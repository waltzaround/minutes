//! Identity directory: one registry of people that bridges voice →
//! person → transcript → action item → Linear assignee / Notion user.
//!
//! Identity resolution is deterministic and lives here; the LLM is never
//! responsible for it. Voice embeddings are sensitive: they are stored
//! encrypted (key in the OS credential store) and never leave the machine.

pub mod enrollment;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::storage::secrets::{SecretKey, SecretStore, VoiceprintCipher};
use crate::storage::{new_id, now, Database};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceProfileSummary {
    pub embedding_count: u32,
    pub enrolled_count: u32,
    pub from_meetings_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Person {
    pub id: String,
    pub display_name: String,
    pub email: Option<String>,
    pub avatar_path: Option<String>,
    pub is_self: bool,
    pub voice_profile: Option<VoiceProfileSummary>,
    pub notion_user_id: Option<String>,
    pub linear_user_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PersonInput {
    pub display_name: String,
    pub email: Option<String>,
    pub is_self: Option<bool>,
}

/// Export format: directory metadata without any voice data.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PersonExport {
    pub display_name: String,
    pub email: Option<String>,
    pub has_voice_profile: bool,
    pub notion_user_id: Option<String>,
    pub linear_user_id: Option<String>,
}

pub const EMBEDDING_MODEL_ID: &str = crate::models::catalog::EMBEDDING_ID;

fn mapping(c: &rusqlite::Connection, provider: &str, person_id: &str) -> rusqlite::Result<Option<String>> {
    c.query_row(
        "SELECT remote_id FROM integration_mappings WHERE provider = ?1 AND entity_type = 'person' AND local_id = ?2",
        params![provider, person_id],
        |r| r.get(0),
    )
    .optional()
}

fn load(c: &rusqlite::Connection, where_sql: &str, arg: Option<&str>) -> rusqlite::Result<Vec<Person>> {
    let sql = format!(
        "SELECT p.id, p.display_name, p.email, p.avatar_path, p.is_self,
                sp.created_at, sp.updated_at,
                (SELECT count(*) FROM speaker_embeddings e WHERE e.profile_id = sp.id),
                (SELECT count(*) FROM speaker_embeddings e WHERE e.profile_id = sp.id AND e.source = 'enrollment')
         FROM people p LEFT JOIN speaker_profiles sp ON sp.person_id = p.id
         {where_sql} ORDER BY p.is_self DESC, p.display_name COLLATE NOCASE"
    );
    let mut stmt = c.prepare(&sql)?;
    let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Person> {
        let created: Option<String> = r.get(5)?;
        let count: u32 = r.get(7)?;
        let enrolled: u32 = r.get(8)?;
        Ok(Person {
            id: r.get(0)?,
            display_name: r.get(1)?,
            email: r.get(2)?,
            avatar_path: r.get(3)?,
            is_self: r.get::<_, i64>(4)? != 0,
            voice_profile: created.filter(|_| count > 0).map(|created_at| VoiceProfileSummary {
                embedding_count: count,
                enrolled_count: enrolled,
                from_meetings_count: count - enrolled,
                created_at,
                updated_at: r.get(6).unwrap_or_default(),
            }),
            notion_user_id: None,
            linear_user_id: None,
        })
    };
    let mut people: Vec<Person> = match arg {
        Some(a) => stmt.query_map([a], map)?.collect::<rusqlite::Result<_>>()?,
        None => stmt.query_map([], map)?.collect::<rusqlite::Result<_>>()?,
    };
    for p in &mut people {
        p.notion_user_id = mapping(c, "notion", &p.id)?;
        p.linear_user_id = mapping(c, "linear", &p.id)?;
    }
    Ok(people)
}

pub fn list(db: &Database) -> rusqlite::Result<Vec<Person>> {
    db.with(|c| load(c, "", None))
}

pub fn get(db: &Database, id: &str) -> rusqlite::Result<Option<Person>> {
    db.with(|c| load(c, "WHERE p.id = ?1", Some(id))).map(|v| v.into_iter().next())
}

pub fn create(db: &Database, input: &PersonInput) -> anyhow::Result<Person> {
    let name = input.display_name.trim();
    anyhow::ensure!(!name.is_empty(), "A name is required.");
    let id = new_id();
    let ts = now();
    db.transaction(|tx| {
        if input.is_self == Some(true) {
            tx.execute("UPDATE people SET is_self = 0 WHERE is_self = 1", [])?;
        }
        tx.execute(
            "INSERT INTO people (id, display_name, email, is_self, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![id, name, input.email.as_deref().map(str::trim).filter(|e| !e.is_empty()), input.is_self.unwrap_or(false) as i32, ts],
        )?;
        Ok(())
    })?;
    Ok(get(db, &id)?.expect("just created"))
}

pub fn update(db: &Database, id: &str, input: &PersonInput) -> anyhow::Result<Person> {
    let name = input.display_name.trim();
    anyhow::ensure!(!name.is_empty(), "A name is required.");
    db.transaction(|tx| {
        if input.is_self == Some(true) {
            tx.execute("UPDATE people SET is_self = 0 WHERE is_self = 1 AND id != ?1", [id])?;
        }
        tx.execute(
            "UPDATE people SET display_name = ?2, email = ?3, is_self = COALESCE(?4, is_self), updated_at = ?5 WHERE id = ?1",
            params![id, name, input.email.as_deref().map(str::trim).filter(|e| !e.is_empty()), input.is_self.map(|b| b as i32), now()],
        )?;
        Ok(())
    })?;
    get(db, id)?.ok_or_else(|| anyhow::anyhow!("That person could not be found."))
}

/// Delete a person. Transcript segments and action items keep their text;
/// their person references are cleared by foreign keys (ON DELETE SET NULL),
/// and voice data is removed by cascade.
pub fn delete(db: &Database, id: &str) -> rusqlite::Result<()> {
    db.transaction(|tx| {
        tx.execute("DELETE FROM integration_mappings WHERE entity_type = 'person' AND local_id = ?1", [id])?;
        tx.execute("DELETE FROM people WHERE id = ?1", [id])?;
        Ok(())
    })
}

pub fn export(db: &Database) -> rusqlite::Result<Vec<PersonExport>> {
    Ok(list(db)?
        .into_iter()
        .map(|p| PersonExport {
            display_name: p.display_name,
            email: p.email,
            has_voice_profile: p.voice_profile.is_some(),
            notion_user_id: p.notion_user_id,
            linear_user_id: p.linear_user_id,
        })
        .collect())
}

/// Store embeddings (encrypted) for a person, creating the profile if needed.
pub fn add_embeddings(
    db: &Database,
    cipher: &VoiceprintCipher,
    person_id: &str,
    embeddings: &[Vec<f32>],
    source: &str,
    meeting_id: Option<&str>,
    quality: Option<f32>,
) -> rusqlite::Result<()> {
    let ts = now();
    db.transaction(|tx| {
        let profile_id: String = match tx
            .query_row("SELECT id FROM speaker_profiles WHERE person_id = ?1", [person_id], |r| r.get(0))
            .optional()?
        {
            Some(id) => {
                tx.execute("UPDATE speaker_profiles SET updated_at = ?2 WHERE id = ?1", params![id, ts])?;
                id
            }
            None => {
                let id = new_id();
                tx.execute(
                    "INSERT INTO speaker_profiles (id, person_id, embedding_model, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![id, person_id, EMBEDDING_MODEL_ID, ts],
                )?;
                id
            }
        };
        for e in embeddings {
            let sealed = cipher.seal_embedding(e);
            tx.execute(
                "INSERT INTO speaker_embeddings (id, profile_id, source, meeting_id, dimensions, ciphertext, nonce, quality, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![new_id(), profile_id, source, meeting_id, e.len() as i64, sealed.ciphertext, sealed.nonce, quality, ts],
            )?;
        }
        Ok(())
    })
}

/// Decrypt all voice profiles for matching. People whose data can't be
/// decrypted (e.g. the key was reset) are skipped with a warning.
pub fn load_voiceprints(db: &Database, cipher: &VoiceprintCipher) -> rusqlite::Result<Vec<(String, Vec<Vec<f32>>)>> {
    let rows: Vec<(String, Vec<u8>, Vec<u8>)> = db.with(|c| {
        let mut stmt = c.prepare(
            "SELECT sp.person_id, e.ciphertext, e.nonce FROM speaker_embeddings e
             JOIN speaker_profiles sp ON sp.id = e.profile_id
             WHERE sp.embedding_model = ?1",
        )?;
        let rows = stmt.query_map([EMBEDDING_MODEL_ID], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    })?;
    let mut out: Vec<(String, Vec<Vec<f32>>)> = Vec::new();
    for (person, ct, nonce) in rows {
        match cipher.open_embedding(&ct, &nonce) {
            Ok(v) => match out.iter_mut().find(|(p, _)| *p == person) {
                Some((_, list)) => list.push(v),
                None => out.push((person, vec![v])),
            },
            Err(_) => tracing::warn!(person, "could not decrypt a voice embedding; skipping"),
        }
    }
    Ok(out)
}

pub fn delete_voice_profile(db: &Database, person_id: &str) -> rusqlite::Result<()> {
    db.with(|c| c.execute("DELETE FROM speaker_profiles WHERE person_id = ?1", [person_id]).map(|_| ()))
}

/// Remove every voice profile and the encryption key itself.
pub fn clear_all_voice_data(db: &Database, secrets: &dyn SecretStore) -> anyhow::Result<()> {
    db.with(|c| c.execute("DELETE FROM speaker_profiles", []))?;
    secrets.delete(SecretKey::VoiceprintKey)?;
    Ok(())
}

pub fn set_mapping(db: &Database, provider: &str, person_id: &str, remote_id: Option<&str>, remote_name: Option<&str>) -> rusqlite::Result<()> {
    db.with(|c| match remote_id {
        Some(r) => c
            .execute(
                "INSERT INTO integration_mappings (id, provider, entity_type, local_id, remote_id, remote_name, updated_at)
                 VALUES (?1, ?2, 'person', ?3, ?4, ?5, ?6)
                 ON CONFLICT(provider, entity_type, local_id) DO UPDATE SET remote_id = excluded.remote_id,
                   remote_name = excluded.remote_name, updated_at = excluded.updated_at",
                params![new_id(), provider, person_id, r, remote_name, now()],
            )
            .map(|_| ()),
        None => c
            .execute(
                "DELETE FROM integration_mappings WHERE provider = ?1 AND entity_type = 'person' AND local_id = ?2",
                params![provider, person_id],
            )
            .map(|_| ()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::secrets::MemorySecretStore;

    #[test]
    fn crud_self_and_voice_profiles() {
        let db = Database::open_in_memory().unwrap();
        let store = MemorySecretStore::default();
        let cipher = VoiceprintCipher::load_or_create(&store).unwrap();
        let walter = create(&db, &PersonInput { display_name: "Walter".into(), email: None, is_self: Some(true) }).unwrap();
        let tom = create(&db, &PersonInput { display_name: "Tom".into(), email: Some("tom@x.co".into()), is_self: Some(true) }).unwrap();
        // Only one "self".
        let all = list(&db).unwrap();
        assert!(all.iter().find(|p| p.id == tom.id).unwrap().is_self);
        assert!(!all.iter().find(|p| p.id == walter.id).unwrap().is_self);

        add_embeddings(&db, &cipher, &tom.id, &[vec![0.1, 0.2], vec![0.3, 0.4]], "enrollment", None, Some(0.8)).unwrap();
        let t = get(&db, &tom.id).unwrap().unwrap();
        assert_eq!(t.voice_profile.as_ref().unwrap().embedding_count, 2);
        let prints = load_voiceprints(&db, &cipher).unwrap();
        assert_eq!(prints[0].1[1], vec![0.3, 0.4]);

        // Raw database never contains the plaintext floats.
        let raw: Vec<u8> = db.with(|c| c.query_row("SELECT ciphertext FROM speaker_embeddings LIMIT 1", [], |r| r.get(0))).unwrap();
        assert_ne!(raw, [0.1f32, 0.2].iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>());

        set_mapping(&db, "linear", &tom.id, Some("lin-1"), Some("Tom H")).unwrap();
        let ex = export(&db).unwrap();
        let tom_ex = ex.iter().find(|p| p.display_name == "Tom").unwrap();
        assert!(tom_ex.has_voice_profile);
        assert_eq!(tom_ex.linear_user_id.as_deref(), Some("lin-1"));
        assert!(!serde_json::to_string(&ex).unwrap().contains("embedding\""));

        delete_voice_profile(&db, &tom.id).unwrap();
        assert!(get(&db, &tom.id).unwrap().unwrap().voice_profile.is_none());

        add_embeddings(&db, &cipher, &walter.id, &[vec![1.0]], "enrollment", None, None).unwrap();
        clear_all_voice_data(&db, &store).unwrap();
        assert!(load_voiceprints(&db, &VoiceprintCipher::load_or_create(&store).unwrap()).unwrap().is_empty());

        delete(&db, &tom.id).unwrap();
        assert!(get(&db, &tom.id).unwrap().is_none());
    }
}
