//! SQLite persistence for meetings and their audio tracks.

use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use ts_rs::TS;

use super::{MeetingStatus, MeetingSummary};
use crate::audio::capture::{AudioSource, TrackInfo};
use crate::storage::{new_id, now, Database};

pub fn default_title() -> String {
    chrono::Local::now().format("Meeting %-d %b, %-I:%M %P").to_string()
}

pub fn create_meeting(
    db: &Database,
    title: &str,
    meetings_root: &std::path::Path,
    capture_mic: bool,
    capture_system: bool,
    retention: &str,
) -> rusqlite::Result<(String, String, PathBuf)> {
    let id = new_id();
    let ts = now();
    let audio_dir = meetings_root.join(&id).join("audio");
    db.with(|c| {
        c.execute(
            "INSERT INTO meetings (id, title, status, started_at, capture_microphone, capture_system, audio_dir,
                                   audio_retention, created_at, updated_at)
             VALUES (?1, ?2, 'recording', ?3, ?4, ?5, ?6, ?7, ?3, ?3)",
            params![id, title, ts, capture_mic as i32, capture_system as i32, audio_dir.display().to_string(), retention],
        )
    })?;
    Ok((id, ts, audio_dir))
}

pub fn insert_track(db: &Database, meeting_id: &str, info: &TrackInfo, segment: u32) -> rusqlite::Result<String> {
    let id = new_id();
    db.with(|c| {
        c.execute(
            "INSERT INTO audio_tracks (id, meeting_id, source, segment_index, start_offset_ms, device_id, device_name,
                                       path, sample_rate, channels, status, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'recording', ?11)",
            params![
                id,
                meeting_id,
                info.source.as_str(),
                segment,
                info.start_offset_ms as i64,
                info.device_id,
                info.device_name,
                info.path.display().to_string(),
                info.sample_rate,
                info.channels,
                now()
            ],
        )
    })?;
    Ok(id)
}

pub fn update_track(
    db: &Database,
    track_id: &str,
    frames: Option<u64>,
    status: Option<&str>,
    error: Option<&str>,
) -> rusqlite::Result<()> {
    db.with(|c| {
        c.execute(
            "UPDATE audio_tracks SET frames_written = COALESCE(?2, frames_written), status = COALESCE(?3, status),
                    error = COALESCE(?4, error), updated_at = ?5 WHERE id = ?1",
            params![track_id, frames.map(|f| f as i64), status, error, now()],
        )
        .map(|_| ())
    })
}

pub fn touch(db: &Database, meeting_id: &str) -> rusqlite::Result<()> {
    db.with(|c| c.execute("UPDATE meetings SET updated_at = ?2 WHERE id = ?1", params![meeting_id, now()]).map(|_| ()))
}

pub fn finish_recording(db: &Database, meeting_id: &str, duration_ms: u64, status: MeetingStatus) -> rusqlite::Result<()> {
    let ts = now();
    db.with(|c| {
        c.execute(
            "UPDATE meetings SET status = ?2, ended_at = ?3, duration_ms = ?4, updated_at = ?3, paused = ?5, needs_full_transcription = MAX(needs_full_transcription, ?5) WHERE id = ?1",
            params![meeting_id, status.as_db(), ts, duration_ms as i64, (status == MeetingStatus::Paused) as i32],
        )
        .map(|_| ())
    })
}

pub fn set_status(db: &Database, meeting_id: &str, status: MeetingStatus) -> rusqlite::Result<()> {
    db.with(|c| {
        c.execute("UPDATE meetings SET status = ?2, updated_at = ?3 WHERE id = ?1", params![meeting_id, status.as_db(), now()])
            .map(|_| ())
    })
}

pub fn rename(db: &Database, meeting_id: &str, title: &str) -> rusqlite::Result<()> {
    db.with(|c| {
        c.execute("UPDATE meetings SET title = ?2, updated_at = ?3 WHERE id = ?1", params![meeting_id, title, now()])
            .map(|_| ())
    })
}

const SUMMARY_SQL: &str = "
    SELECT m.id, m.title, CASE WHEN m.paused = 1 THEN 'paused' ELSE m.status END, m.started_at, m.ended_at, m.duration_ms,
           (SELECT count(*) FROM speaker_clusters sc WHERE sc.meeting_id = m.id AND sc.merged_into IS NULL
              AND EXISTS (SELECT 1 FROM transcript_segments t WHERE t.speaker_cluster_id = sc.id)),
           (SELECT count(*) FROM action_items a WHERE a.meeting_id = m.id AND a.dismissed = 0),
           (SELECT count(*) FROM transcript_segments t WHERE t.meeting_id = m.id)
    FROM meetings m";

fn summary_row(r: &Row<'_>) -> rusqlite::Result<MeetingSummary> {
    Ok(MeetingSummary {
        id: r.get(0)?,
        title: r.get(1)?,
        status: MeetingStatus::from_db(&r.get::<_, String>(2)?),
        started_at: r.get(3)?,
        ended_at: r.get(4)?,
        duration_ms: r.get::<_, Option<i64>>(5)?.map(|d| d as u64),
        speaker_count: r.get(6)?,
        action_count: r.get(7)?,
        segment_count: r.get(8)?,
    })
}

pub fn list_recent(db: &Database, limit: u32) -> rusqlite::Result<Vec<MeetingSummary>> {
    db.with(|c| {
        let mut stmt = c.prepare(&format!("{SUMMARY_SQL} ORDER BY m.started_at DESC LIMIT ?1"))?;
        let rows = stmt.query_map([limit], summary_row)?;
        rows.collect()
    })
}

pub fn get_summary(db: &Database, id: &str) -> rusqlite::Result<Option<MeetingSummary>> {
    db.with(|c| c.query_row(&format!("{SUMMARY_SQL} WHERE m.id = ?1"), [id], summary_row).optional())
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioTrackRow {
    pub id: String,
    pub source: AudioSource,
    pub segment_index: u32,
    pub start_offset_ms: u64,
    pub device_name: Option<String>,
    pub path: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames_written: u64,
    pub status: String,
    pub error: Option<String>,
}

pub fn tracks(conn: &Connection, meeting_id: &str) -> rusqlite::Result<Vec<AudioTrackRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, source, segment_index, start_offset_ms, device_name, path, sample_rate, channels, frames_written,
                status, error
         FROM audio_tracks WHERE meeting_id = ?1 ORDER BY source, segment_index",
    )?;
    let rows = stmt.query_map([meeting_id], |r| {
        Ok(AudioTrackRow {
            id: r.get(0)?,
            source: if r.get::<_, String>(1)? == "microphone" { AudioSource::Microphone } else { AudioSource::System },
            segment_index: r.get(2)?,
            start_offset_ms: r.get::<_, i64>(3)? as u64,
            device_name: r.get(4)?,
            path: r.get(5)?,
            sample_rate: r.get(6)?,
            channels: r.get(7)?,
            frames_written: r.get::<_, i64>(8)? as u64,
            status: r.get(9)?,
            error: r.get(10)?,
        })
    })?;
    rows.collect()
}

pub fn audio_dir(db: &Database, meeting_id: &str) -> rusqlite::Result<Option<PathBuf>> {
    db.with(|c| {
        c.query_row("SELECT audio_dir FROM meetings WHERE id = ?1", [meeting_id], |r| r.get::<_, Option<String>>(0))
            .optional()
            .map(|o| o.flatten().map(PathBuf::from))
    })
}

/// Meetings left in `recording` state by a crash become `interrupted`.
/// Returns their ids.
pub fn mark_interrupted(db: &Database) -> rusqlite::Result<Vec<String>> {
    db.transaction(|tx| {
        let ids: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM meetings WHERE status = 'recording'")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        tx.execute("UPDATE meetings SET status = 'interrupted', updated_at = ?1 WHERE status = 'recording'", [now()])?;
        tx.execute("UPDATE audio_tracks SET status = 'failed' WHERE status = 'recording'", [])?;
        Ok(ids)
    })
}

/// Delete a meeting and everything that belongs to it (cascades), plus its
/// audio directory.
pub fn delete_meeting(db: &Database, meeting_id: &str, meetings_root: &std::path::Path) -> anyhow::Result<()> {
    let dir = audio_dir(db, meeting_id)?;
    db.with(|c| c.execute("DELETE FROM meetings WHERE id = ?1", [meeting_id]))?;
    if let Some(dir) = dir {
        // The audio directory is <meetings_root>/<id>/...; remove the meeting folder.
        let meeting_folder = meetings_root.join(meeting_id);
        if meeting_folder.exists() && dir.starts_with(&meeting_folder) {
            std::fs::remove_dir_all(&meeting_folder)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_survives_reopening_and_is_not_crash_recovery() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("sessions.db");
        let db = Database::open(&path).unwrap();
        let (id, _, _) = create_meeting(&db, "Class", root.path(), true, false, "delete_after_processing").unwrap();
        finish_recording(&db, &id, 60_000, MeetingStatus::Paused).unwrap();
        drop(db);
        let db = Database::open(&path).unwrap();
        assert!(mark_interrupted(&db).unwrap().is_empty());
        let m = get_summary(&db, &id).unwrap().unwrap();
        assert_eq!(m.status, MeetingStatus::Paused);
        assert_eq!(m.duration_ms, Some(60_000));
        assert!(db.with(|c| c.query_row("SELECT needs_full_transcription FROM meetings WHERE id = ?1", [&id], |r| r.get::<_, bool>(0))).unwrap());
        finish_recording(&db, &id, 60_000, MeetingStatus::Processing).unwrap();
        assert_eq!(get_summary(&db, &id).unwrap().unwrap().status, MeetingStatus::Processing);
    }

    #[test]
    fn create_list_interrupt_delete() {
        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (id, _, dir) = create_meeting(&db, "Design Weekly", root.path(), true, true, "delete_after_processing").unwrap();
        let info = TrackInfo {
            source: AudioSource::Microphone,
            device_id: None,
            device_name: "Mic".into(),
            sample_rate: 48_000,
            channels: 1,
            path: dir.join("microphone-0.wav"),
            start_offset_ms: 0,
        };
        let track = insert_track(&db, &id, &info, 0).unwrap();
        update_track(&db, &track, Some(480_000), None, None).unwrap();

        let list = list_recent(&db, 10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].status, MeetingStatus::Recording);

        assert_eq!(mark_interrupted(&db).unwrap(), vec![id.clone()]);
        assert_eq!(get_summary(&db, &id).unwrap().unwrap().status, MeetingStatus::Interrupted);
        let t = db.with(|c| tracks(c, &id)).unwrap();
        assert_eq!(t[0].frames_written, 480_000);

        delete_meeting(&db, &id, root.path()).unwrap();
        assert!(get_summary(&db, &id).unwrap().is_none());
        assert!(db.with(|c| tracks(c, &id)).unwrap().is_empty());
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingDetail {
    pub summary: MeetingSummary,
    pub processing_error: Option<String>,
    pub audio_available: bool,
    pub audio_retention: String,
    pub transcript_revision: u32,
    pub notion_page_url: Option<String>,
    pub tracks: Vec<AudioTrackRow>,
    pub segments: Vec<crate::meetings::transcript::TranscriptSegment>,
}

pub fn get_detail(db: &Database, id: &str) -> rusqlite::Result<Option<MeetingDetail>> {
    let Some(summary) = get_summary(db, id)? else { return Ok(None) };
    db.with(|c| {
        let (processing_error, deleted, retention, revision, notion_url): (Option<String>, Option<String>, String, u32, Option<String>) = c.query_row(
            "SELECT processing_error, audio_deleted_at, audio_retention, transcript_revision, notion_page_url FROM meetings WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        let tracks = tracks(c, id)?;
        let audio_available = deleted.is_none() && tracks.iter().any(|t| std::path::Path::new(&t.path).exists());
        let segments = crate::meetings::transcript::list_segments(c, id)?;
        Ok(Some(MeetingDetail {
            summary,
            processing_error,
            audio_available,
            audio_retention: retention,
            transcript_revision: revision,
            notion_page_url: notion_url,
            tracks,
            segments,
        }))
    })
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchHit {
    pub meeting_id: String,
    pub meeting_title: String,
    pub started_at: String,
    /// "title", "summary", "decision", "action" or "transcript".
    pub kind: String,
    pub text: String,
    pub segment_id: Option<String>,
    pub start_ms: Option<u64>,
}

fn escape_like(q: &str) -> String {
    q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// Case-insensitive search across meeting titles, notes and transcripts.
pub fn search(db: &Database, query: &str, limit: u32) -> rusqlite::Result<Vec<SearchHit>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!("%{}%", escape_like(q));
    db.with(|c| {
        let sql = "
            SELECT * FROM (
              SELECT m.id, m.title, m.started_at, 'title' AS kind, m.title AS text, NULL AS seg, NULL AS ms, 0 AS rank
                FROM meetings m WHERE m.title LIKE ?1 ESCAPE '\\' AND m.status != 'recording'
              UNION ALL
              SELECT m.id, m.title, m.started_at, 'summary', a.summary_json, NULL, NULL, 1
                FROM meeting_analysis a JOIN meetings m ON m.id = a.meeting_id WHERE a.summary_json LIKE ?1 ESCAPE '\\'
              UNION ALL
              SELECT m.id, m.title, m.started_at, 'decision', d.text, NULL, NULL, 2
                FROM decisions d JOIN meeting_analysis a ON a.id = d.analysis_id JOIN meetings m ON m.id = a.meeting_id
                WHERE d.text LIKE ?1 ESCAPE '\\'
              UNION ALL
              SELECT m.id, m.title, m.started_at, 'action', ai.title, NULL, NULL, 3
                FROM action_items ai JOIN meetings m ON m.id = ai.meeting_id
                WHERE ai.dismissed = 0 AND (ai.title LIKE ?1 ESCAPE '\\' OR ai.description LIKE ?1 ESCAPE '\\')
              UNION ALL
              SELECT m.id, m.title, m.started_at, 'transcript', t.text, t.id, t.start_ms, 4
                FROM transcript_segments t JOIN meetings m ON m.id = t.meeting_id WHERE t.text LIKE ?1 ESCAPE '\\'
            ) ORDER BY rank, started_at DESC LIMIT ?2";
        let mut stmt = c.prepare(sql)?;
        let rows = stmt.query_map(params![pattern, limit], |r| {
            let kind: String = r.get(3)?;
            let mut text: String = r.get(4)?;
            if kind == "summary" {
                // Show the matching bullet, not the whole JSON list.
                let lower = q.to_lowercase();
                text = serde_json::from_str::<Vec<String>>(&text)
                    .ok()
                    .and_then(|v| v.into_iter().find(|s| s.to_lowercase().contains(&lower)))
                    .unwrap_or(text);
            }
            Ok(SearchHit {
                meeting_id: r.get(0)?,
                meeting_title: r.get(1)?,
                started_at: r.get(2)?,
                kind,
                text,
                segment_id: r.get(5)?,
                start_ms: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
            })
        })?;
        rows.collect()
    })
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use crate::meetings::transcript::{insert_segment, NewSegment, SegmentSource};

    #[test]
    fn finds_titles_and_transcript_lines() {
        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (id, _, _) = create_meeting(&db, "Design Weekly", root.path(), true, true, "keep_forever").unwrap();
        finish_recording(&db, &id, 1000, MeetingStatus::Ready).unwrap();
        insert_segment(&db, &NewSegment { meeting_id: &id, source: SegmentSource::System, start_ms: 5000, end_ms: 6000, text: "The onboarding API ships Friday", words: &[], person_id: None, speaker_confidence: None }).unwrap();
        let hits = search(&db, "onboarding", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "transcript");
        assert_eq!(hits[0].start_ms, Some(5000));
        let hits = search(&db, "design", 20).unwrap();
        assert_eq!(hits[0].kind, "title");
        assert!(search(&db, "100%", 20).unwrap().is_empty(), "wildcards are escaped");
        assert!(search(&db, "  ", 20).unwrap().is_empty());
    }
}
