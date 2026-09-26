//! Prompt construction and context management.
//!
//! The transcript is presented as numbered lines with timestamps and speaker
//! labels. Short aliases (S1, S2, …) keep prompts compact and make evidence
//! checkable. Long meetings are analysed in chunks whose structured results
//! are then merged in a final synthesis pass; nothing is ever truncated.

use super::schemas::{RawAnalysis, TranscriptIndex};

/// A transcript line as shown to the model.
#[derive(Debug, Clone)]
pub struct Line {
    pub alias: String,
    pub segment_id: String,
    pub start_ms: u64,
    pub speaker: String,
    pub text: String,
}

impl Line {
    pub fn render(&self) -> String {
        let s = self.start_ms / 1000;
        format!("{} [{:02}:{:02}:{:02}] {}: {}", self.alias, s / 3600, (s / 60) % 60, s % 60, self.speaker, self.text)
    }
}

pub fn build_lines(segments: impl IntoIterator<Item = (String, u64, String, String)>) -> Vec<Line> {
    segments
        .into_iter()
        .enumerate()
        .map(|(i, (id, start, speaker, text))| Line { alias: format!("S{}", i + 1), segment_id: id, start_ms: start, speaker, text })
        .collect()
}

pub fn index(lines: &[Line]) -> TranscriptIndex {
    let mut speakers: Vec<String> = Vec::new();
    for l in lines {
        if !speakers.contains(&l.speaker) {
            speakers.push(l.speaker.clone());
        }
    }
    TranscriptIndex {
        segments: lines.iter().map(|l| (l.alias.clone(), (l.segment_id.clone(), l.speaker.clone(), l.text.clone()))).collect(),
        speakers,
        meeting_date: None,
    }
}

/// Conservative token estimate (≈3.2 characters per token for English
/// meeting speech plus formatting).
pub fn estimate_tokens(s: &str) -> u32 {
    (s.chars().count() as f32 / 3.2).ceil() as u32
}

pub const SYSTEM_PROMPT: &str = r#"You turn meeting transcripts into structured notes. You are careful and conservative.

Rules:
- Use only what is in the transcript. Never invent people, dates, decisions or tasks.
- Every decision, action item and unresolved question must cite the transcript lines that support it, using their ids (e.g. "S12"). Cite only ids that appear in the transcript.
- Action item owners must be a speaker name exactly as written in the transcript, or null.
- assignmentType:
  - "explicit_acceptance": the owner themselves agreed to do it (e.g. owner says "Yep, I'll take it").
  - "explicit_assignment": someone clearly asked or assigned the owner (e.g. "Tom, can you do that?") and it was not declined.
  - "suggested": only a suggestion or possibility (e.g. "Maybe Tom can look at it").
  - "unclear": no clear owner.
  Do not treat suggestions as assignments.
- Decisions are things the group agreed; they are not action items unless someone must do follow-up work.
- confidence (0 to 1) reflects how certain it is that this is a real, agreed action with this owner.
- dueDate: only when a date can be determined; resolve relative dates ("Friday") using the meeting date. Otherwise null. Put the words used in "due".
- Summary: 3 to 8 short factual bullet points.
- Title: a short, specific meeting title (e.g. "Design weekly: onboarding API").
- Write in the language of the transcript. Output only the JSON object."#;

pub fn user_prompt(meeting_title: &str, meeting_date: &str, lines: &[Line], part: Option<(usize, usize)>) -> String {
    let transcript: Vec<String> = lines.iter().map(Line::render).collect();
    let part_note = match part {
        Some((i, n)) => format!(
            "\nThis is part {i} of {n} of a long meeting. Analyse only this part; the parts will be combined afterwards.\n"
        ),
        None => String::new(),
    };
    format!(
        "Meeting: {meeting_title}\nDate: {meeting_date}\nSpeakers: {}\n{part_note}\nTranscript:\n{}\n",
        index(lines).speakers.join(", "),
        transcript.join("\n")
    )
}

pub fn synthesis_prompt(meeting_title: &str, meeting_date: &str, speakers: &[String], parts: &[RawAnalysis]) -> String {
    let parts_json = serde_json::to_string_pretty(parts).unwrap_or_default();
    format!(
        "Meeting: {meeting_title}\nDate: {meeting_date}\nSpeakers: {}\n\nThe meeting was analysed in {} consecutive parts. \
         Combine them into one analysis of the whole meeting: merge duplicates, keep the evidence ids from the parts \
         (do not invent new ids), keep owners and assignment types unless a later part clearly changes them, and write \
         one summary for the whole meeting.\n\nParts:\n{parts_json}\n",
        speakers.join(", "),
        parts.len()
    )
}

pub fn correction_prompt(problems: &[String]) -> String {
    format!(
        "Your previous answer did not meet the requirements:\n- {}\n\nReturn the complete corrected JSON object. \
         Cite only line ids that appear in the transcript and use speaker names exactly as written.",
        problems.join("\n- ")
    )
}

/// Split lines into chunks whose rendered size fits `budget_tokens`.
pub fn chunk_lines(lines: &[Line], budget_tokens: u32) -> Vec<Vec<Line>> {
    let mut chunks: Vec<Vec<Line>> = Vec::new();
    let mut current: Vec<Line> = Vec::new();
    let mut used = 0u32;
    for l in lines {
        let t = estimate_tokens(&l.render()) + 1;
        if !current.is_empty() && used + t > budget_tokens {
            chunks.push(std::mem::take(&mut current));
            used = 0;
        }
        used += t;
        current.push(l.clone());
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Tokens reserved for the model's JSON answer.
pub fn output_budget(context: u32) -> u32 {
    (context / 4).clamp(1024, 4096)
}

/// Transcript tokens that fit in one request at `context`.
pub fn transcript_budget(context: u32) -> u32 {
    let fixed = estimate_tokens(SYSTEM_PROMPT) + 200;
    let usable = (context as f32 * 0.9) as u32; // keep 10% headroom
    usable.saturating_sub(fixed + output_budget(context))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize, words: usize) -> Vec<Line> {
        build_lines((0..n).map(|i| (format!("id{i}"), i as u64 * 5000, "Tom".to_string(), "word ".repeat(words))))
    }

    #[test]
    fn renders_alias_time_speaker() {
        let l = &build_lines([("x".into(), 3_723_000, "Sarah".into(), "Hi".into())])[0];
        assert_eq!(l.render(), "S1 [01:02:03] Sarah: Hi");
    }

    #[test]
    fn short_meetings_fit_in_one_pass() {
        let l = lines(40, 20);
        let total: u32 = l.iter().map(|x| estimate_tokens(&x.render())).sum();
        assert!(total < transcript_budget(16_384));
        assert_eq!(chunk_lines(&l, transcript_budget(16_384)).len(), 1);
    }

    #[test]
    fn long_meetings_are_chunked_without_losing_lines() {
        let l = lines(2_000, 30);
        let chunks = chunk_lines(&l, transcript_budget(8_192));
        assert!(chunks.len() > 1);
        let flat: Vec<&str> = chunks.iter().flatten().map(|x| x.alias.as_str()).collect();
        assert_eq!(flat.len(), 2_000);
        assert_eq!(flat[0], "S1");
        assert_eq!(flat[1_999], "S2000");
        for c in &chunks {
            let size: u32 = c.iter().map(|x| estimate_tokens(&x.render()) + 1).sum();
            assert!(size <= transcript_budget(8_192) || c.len() == 1);
        }
    }

    #[test]
    fn budgets_leave_room_for_output() {
        for ctx in [4_096u32, 8_192, 16_384, 32_768] {
            assert!(transcript_budget(ctx) + output_budget(ctx) < ctx);
            assert!(transcript_budget(ctx) > 0);
        }
    }
}
