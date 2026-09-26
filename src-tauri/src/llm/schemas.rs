//! Structured meeting analysis: what the model must return, the JSON Schema
//! used to constrain generation, and strict validation.
//!
//! The model never sees database ids. Transcript segments are presented with
//! short aliases ("S12"); evidence must reference those aliases and is mapped
//! back to real segment ids here. Owners are speaker names exactly as shown
//! in the transcript; identity resolution to people happens deterministically
//! afterwards, never inside the model.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AssignmentType {
    /// The owner said they would do it ("Yep, I'll take it").
    ExplicitAcceptance,
    /// Someone clearly assigned it to the owner ("Tom, can you do that?").
    ExplicitAssignment,
    /// Only suggested ("Maybe Tom can look at it").
    Suggested,
    /// Ownership is unclear.
    Unclear,
}

impl AssignmentType {
    pub fn as_db(self) -> &'static str {
        match self {
            AssignmentType::ExplicitAcceptance => "explicit_acceptance",
            AssignmentType::ExplicitAssignment => "explicit_assignment",
            AssignmentType::Suggested => "suggested",
            AssignmentType::Unclear => "unclear",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "explicit_acceptance" => AssignmentType::ExplicitAcceptance,
            "explicit_assignment" => AssignmentType::ExplicitAssignment,
            "suggested" => AssignmentType::Suggested,
            _ => AssignmentType::Unclear,
        }
    }

    pub fn is_strong(self) -> bool {
        matches!(self, AssignmentType::ExplicitAcceptance | AssignmentType::ExplicitAssignment)
    }
}

// ---- Raw model output --------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawEvidenceItem {
    pub text: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawActionItem {
    pub title: String,
    pub description: Option<String>,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub due_date: Option<String>,
    pub assignment_type: AssignmentType,
    pub evidence: Vec<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawAnalysis {
    pub title: String,
    pub summary: Vec<String>,
    pub decisions: Vec<RawEvidenceItem>,
    pub action_items: Vec<RawActionItem>,
    pub unresolved_questions: Vec<RawEvidenceItem>,
}

/// JSON Schema passed to llama.cpp (grammar-constrained decoding).
pub fn analysis_schema() -> serde_json::Value {
    let evidence = json!({ "type": "array", "items": { "type": "string", "pattern": "^S[0-9]+$" }, "minItems": 1, "maxItems": 6 });
    let evidence_item = json!({
        "type": "object",
        "properties": { "text": { "type": "string", "minLength": 1 }, "evidence": evidence },
        "required": ["text", "evidence"],
        "additionalProperties": false
    });
    let nullable_string = json!({ "type": ["string", "null"] });
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string", "minLength": 1, "maxLength": 120 },
            "summary": { "type": "array", "items": { "type": "string", "minLength": 1 }, "minItems": 1, "maxItems": 12 },
            "decisions": { "type": "array", "items": evidence_item, "maxItems": 20 },
            "actionItems": {
                "type": "array",
                "maxItems": 30,
                "items": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "minLength": 1, "maxLength": 140 },
                        "description": nullable_string,
                        "owner": nullable_string,
                        "due": nullable_string,
                        "dueDate": { "type": ["string", "null"], "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$" },
                        "assignmentType": { "enum": ["explicit_acceptance", "explicit_assignment", "suggested", "unclear"] },
                        "evidence": evidence,
                        "confidence": { "type": "number", "minimum": 0, "maximum": 1 }
                    },
                    "required": ["title", "description", "owner", "due", "dueDate", "assignmentType", "evidence", "confidence"],
                    "additionalProperties": false
                }
            },
            "unresolvedQuestions": { "type": "array", "items": evidence_item, "maxItems": 20 }
        },
        "required": ["title", "summary", "decisions", "actionItems", "unresolvedQuestions"],
        "additionalProperties": false
    })
}

// ---- Validated analysis ------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EvidenceItem {
    pub text: String,
    pub evidence_segment_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ActionItem {
    pub title: String,
    pub description: Option<String>,
    /// Speaker label as it appeared in the transcript (resolved to a person
    /// later, deterministically).
    pub owner_label: Option<String>,
    pub owner_person_id: Option<String>,
    pub due_text: Option<String>,
    pub due_date: Option<String>,
    pub assignment_type: AssignmentType,
    pub evidence_segment_ids: Vec<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingAnalysis {
    pub title: String,
    pub summary: Vec<String>,
    pub decisions: Vec<EvidenceItem>,
    pub action_items: Vec<ActionItem>,
    pub unresolved_questions: Vec<EvidenceItem>,
}

/// What the validator knows about the transcript.
pub struct TranscriptIndex {
    /// Alias ("S12") → (segment id, speaker label, text).
    pub segments: HashMap<String, (String, String, String)>,
    /// Speaker labels present in the transcript (as shown to the model).
    pub speakers: Vec<String>,
    /// Meeting date, used to resolve and sanity-check due dates.
    pub meeting_date: Option<chrono::NaiveDate>,
}

/// Resolve spoken due dates deterministically instead of trusting the model:
/// weekday names mean the next such day after the meeting (same day counts
/// only for "today"), plus "today", "tomorrow" and "end of (the) week".
pub fn resolve_due(text: &str, meeting: chrono::NaiveDate) -> Option<chrono::NaiveDate> {
    use chrono::{Datelike, Duration, Weekday};
    let t = text.to_lowercase();
    if t.contains("today") || t.contains("end of day") || t.contains("eod") {
        return Some(meeting);
    }
    if t.contains("tomorrow") {
        return Some(meeting + Duration::days(1));
    }
    let next = |wd: Weekday| {
        let mut d = meeting + Duration::days(1);
        while d.weekday() != wd {
            d += Duration::days(1);
        }
        d
    };
    if t.contains("end of the week") || t.contains("end of week") || t.contains("end of this week") {
        return Some(if meeting.weekday() == Weekday::Fri { meeting } else { next(Weekday::Fri) });
    }
    let days = [
        ("monday", Weekday::Mon),
        ("tuesday", Weekday::Tue),
        ("wednesday", Weekday::Wed),
        ("thursday", Weekday::Thu),
        ("friday", Weekday::Fri),
        ("saturday", Weekday::Sat),
        ("sunday", Weekday::Sun),
    ];
    let hits: Vec<Weekday> = days.iter().filter(|(n, _)| t.contains(n)).map(|(_, w)| *w).collect();
    if hits.len() == 1 && !t.contains("next week") {
        return Some(next(hits[0]));
    }
    None
}

/// A model-supplied date is only trusted within a plausible window.
fn plausible_due(d: chrono::NaiveDate, meeting: chrono::NaiveDate) -> bool {
    d >= meeting - chrono::Duration::days(1) && d <= meeting + chrono::Duration::days(730)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationReport {
    pub problems: Vec<String>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Upper bounds on confidence by assignment strength (conservative).
fn cap_confidence(t: AssignmentType, c: f64) -> f32 {
    let c = c.clamp(0.0, 1.0) as f32;
    match t {
        AssignmentType::ExplicitAcceptance | AssignmentType::ExplicitAssignment => c,
        AssignmentType::Suggested => c.min(0.6),
        AssignmentType::Unclear => c.min(0.4),
    }
}

fn first_name(label: &str) -> &str {
    label.split_whitespace().next().unwrap_or(label)
}

/// Enforce the ownership rules the model is asked to follow:
/// - explicit acceptance must be supported by evidence spoken by the owner;
/// - explicit assignment must be supported by evidence that names the owner
///   (by first name) or is spoken by someone else addressing them;
/// - otherwise the claim is downgraded to "suggested";
/// - no owner → "unclear".
pub fn enforce_assignment(
    t: AssignmentType,
    owner: Option<&str>,
    evidence: &[(String, String, String)],
) -> AssignmentType {
    let Some(owner) = owner else { return AssignmentType::Unclear };
    let owner_lc = owner.to_lowercase();
    let first = first_name(owner).to_lowercase();
    match t {
        AssignmentType::ExplicitAcceptance => {
            if evidence.iter().any(|(_, spk, _)| spk.to_lowercase() == owner_lc) {
                t
            } else {
                AssignmentType::Suggested
            }
        }
        AssignmentType::ExplicitAssignment => {
            let named = evidence.iter().any(|(_, spk, text)| {
                spk.to_lowercase() != owner_lc && text.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|w| w == first)
            });
            let accepted = evidence.iter().any(|(_, spk, _)| spk.to_lowercase() == owner_lc);
            if named || accepted {
                t
            } else {
                AssignmentType::Suggested
            }
        }
        other => other,
    }
}

/// Validate raw output. Returns the cleaned analysis and a report of
/// problems. Problems cause one retry with correction instructions; items
/// that still lack valid evidence after the retry are dropped (never
/// guessed).
pub fn validate(raw: &RawAnalysis, index: &TranscriptIndex) -> (MeetingAnalysis, ValidationReport) {
    let mut problems = Vec::new();
    let resolve = |aliases: &[String], what: &str, problems: &mut Vec<String>| -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for a in aliases {
            match index.segments.get(a.trim()) {
                Some(seg) => {
                    if !out.iter().any(|(id, _, _): &(String, String, String)| *id == seg.0) {
                        out.push(seg.clone());
                    }
                }
                None => problems.push(format!("{what} cites {a}, which is not a transcript line")),
            }
        }
        if out.is_empty() {
            problems.push(format!("{what} has no valid evidence"));
        }
        out
    };

    let title = raw.title.trim().chars().take(120).collect::<String>();
    if title.is_empty() {
        problems.push("title is empty".into());
    }
    let summary: Vec<String> = raw.summary.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if summary.is_empty() {
        problems.push("summary is empty".into());
    }

    let mut decisions = Vec::new();
    for (i, d) in raw.decisions.iter().enumerate() {
        let ev = resolve(&d.evidence, &format!("decision {}", i + 1), &mut problems);
        if !ev.is_empty() && !d.text.trim().is_empty() {
            decisions.push(EvidenceItem { text: d.text.trim().into(), evidence_segment_ids: ev.into_iter().map(|e| e.0).collect() });
        }
    }
    let mut questions = Vec::new();
    for (i, q) in raw.unresolved_questions.iter().enumerate() {
        let ev = resolve(&q.evidence, &format!("question {}", i + 1), &mut problems);
        if !ev.is_empty() && !q.text.trim().is_empty() {
            questions.push(EvidenceItem { text: q.text.trim().into(), evidence_segment_ids: ev.into_iter().map(|e| e.0).collect() });
        }
    }

    let mut actions = Vec::new();
    for (i, a) in raw.action_items.iter().enumerate() {
        let what = format!("action {}", i + 1);
        let ev = resolve(&a.evidence, &what, &mut problems);
        if ev.is_empty() || a.title.trim().is_empty() {
            continue;
        }
        let owner = a.owner.as_deref().map(str::trim).filter(|o| !o.is_empty()).and_then(|o| {
            let found = index.speakers.iter().find(|s| s.eq_ignore_ascii_case(o) || first_name(s).eq_ignore_ascii_case(o));
            if found.is_none() {
                problems.push(format!("{what} owner \"{o}\" is not a speaker in the transcript"));
            }
            found.cloned()
        });
        let spoken = a.due.as_deref().and_then(|t| index.meeting_date.and_then(|m| resolve_due(t, m)));
        let due_date: Option<String> = match (spoken, a.due_date.as_deref()) {
            (Some(d), _) => Some(d.format("%Y-%m-%d").to_string()),
            (None, Some(d)) => {
                let parsed = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok();
                match (parsed, index.meeting_date) {
                    (None, _) => {
                        problems.push(format!("{what} dueDate \"{d}\" is not a valid YYYY-MM-DD date"));
                        None
                    }
                    (Some(p), Some(m)) if !plausible_due(p, m) => {
                        problems.push(format!("{what} dueDate \"{d}\" is not plausible for a meeting on {m}"));
                        None
                    }
                    (Some(_), _) => Some(d.to_string()),
                }
            }
            (None, None) => None,
        };
        let assignment = enforce_assignment(a.assignment_type, owner.as_deref(), &ev);
        actions.push(ActionItem {
            title: a.title.trim().chars().take(140).collect(),
            description: a.description.as_deref().map(str::trim).filter(|d| !d.is_empty()).map(String::from),
            owner_label: owner,
            owner_person_id: None,
            due_text: a.due.as_deref().map(str::trim).filter(|d| !d.is_empty()).map(String::from),
            due_date,
            assignment_type: assignment,
            evidence_segment_ids: ev.into_iter().map(|e| e.0).collect(),
            confidence: cap_confidence(assignment, a.confidence),
        });
    }

    (
        MeetingAnalysis { title, summary, decisions, action_items: actions, unresolved_questions: questions },
        ValidationReport { problems },
    )
}

/// Parse model text into the raw structure (strict: unknown fields fail).
pub fn parse(content: &str) -> Result<RawAnalysis, String> {
    let trimmed = content.trim();
    // Tolerate a fenced block but nothing else around the JSON.
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(trimmed);
    serde_json::from_str::<RawAnalysis>(body).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> TranscriptIndex {
        let mut segments = HashMap::new();
        segments.insert("S1".into(), ("seg-1".into(), "Walter".into(), "Tom, can you do the API changes?".into()));
        segments.insert("S2".into(), ("seg-2".into(), "Tom Hutchison".into(), "Yep, I'll take the API changes by Friday.".into()));
        segments.insert("S3".into(), ("seg-3".into(), "Sarah".into(), "Maybe Tom can look at the analytics thing.".into()));
        TranscriptIndex {
            segments,
            speakers: vec!["Walter".into(), "Tom Hutchison".into(), "Sarah".into()],
            meeting_date: chrono::NaiveDate::from_ymd_opt(2026, 9, 26),
        }
    }

    fn action(owner: Option<&str>, t: AssignmentType, ev: &[&str], conf: f64) -> RawActionItem {
        RawActionItem {
            title: "Do it".into(),
            description: None,
            owner: owner.map(String::from),
            due: None,
            due_date: None,
            assignment_type: t,
            evidence: ev.iter().map(|s| s.to_string()).collect(),
            confidence: conf,
        }
    }

    fn raw(actions: Vec<RawActionItem>) -> RawAnalysis {
        RawAnalysis {
            title: "Design Weekly".into(),
            summary: vec!["Talked about the API.".into()],
            decisions: vec![RawEvidenceItem { text: "Target Friday".into(), evidence: vec!["S1".into()] }],
            action_items: actions,
            unresolved_questions: vec![],
        }
    }

    #[test]
    fn schema_is_valid_json_schema_shape() {
        let s = analysis_schema();
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], false);
        assert!(s["properties"]["actionItems"]["items"]["properties"]["assignmentType"]["enum"].is_array());
    }

    #[test]
    fn parses_strictly() {
        let good = serde_json::to_string(&raw(vec![])).unwrap();
        assert!(parse(&good).is_ok());
        assert!(parse(&format!("```json\n{good}\n```")).is_ok());
        assert!(parse("Here is the analysis: {}").is_err());
        let extra = good.replacen("{", "{\"reasoning\":\"...\",", 1);
        assert!(parse(&extra).is_err(), "unknown fields are rejected");
    }

    #[test]
    fn maps_aliases_to_segment_ids() {
        let (a, r) = validate(&raw(vec![action(Some("Tom Hutchison"), AssignmentType::ExplicitAcceptance, &["S2"], 0.9)]), &index());
        assert!(r.is_ok(), "{:?}", r.problems);
        assert_eq!(a.decisions[0].evidence_segment_ids, vec!["seg-1"]);
        assert_eq!(a.action_items[0].evidence_segment_ids, vec!["seg-2"]);
        assert_eq!(a.action_items[0].owner_label.as_deref(), Some("Tom Hutchison"));
    }

    #[test]
    fn hallucinated_evidence_is_reported_and_item_dropped() {
        let (a, r) = validate(&raw(vec![action(Some("Tom"), AssignmentType::Suggested, &["S99"], 0.5)]), &index());
        assert!(!r.is_ok());
        assert!(a.action_items.is_empty());
    }

    #[test]
    fn unknown_owner_is_reported_and_cleared() {
        let (a, r) = validate(&raw(vec![action(Some("Bob"), AssignmentType::ExplicitAssignment, &["S1"], 0.9)]), &index());
        assert!(r.problems.iter().any(|p| p.contains("Bob")));
        assert_eq!(a.action_items[0].owner_label, None);
        assert_eq!(a.action_items[0].assignment_type, AssignmentType::Unclear);
        assert!(a.action_items[0].confidence <= 0.4);
    }

    #[test]
    fn distinguishes_assignment_acceptance_and_suggestion() {
        let ix = index();
        let ev = |a: &str| vec![ix.segments[a].clone()];
        // "Tom, can you do that?" (said by Walter) → explicit assignment holds.
        assert_eq!(enforce_assignment(AssignmentType::ExplicitAssignment, Some("Tom Hutchison"), &ev("S1")), AssignmentType::ExplicitAssignment);
        // "Yep, I'll take it." (said by Tom) → explicit acceptance holds.
        assert_eq!(enforce_assignment(AssignmentType::ExplicitAcceptance, Some("Tom Hutchison"), &ev("S2")), AssignmentType::ExplicitAcceptance);
        // Claimed acceptance but Tom never spoke in the evidence → suggested.
        assert_eq!(enforce_assignment(AssignmentType::ExplicitAcceptance, Some("Tom Hutchison"), &ev("S3")), AssignmentType::Suggested);
        // "Maybe Tom can look at it" claimed as assignment: the name appears,
        // but the model must mark it suggested; the validator does not upgrade.
        assert_eq!(enforce_assignment(AssignmentType::Suggested, Some("Tom Hutchison"), &ev("S3")), AssignmentType::Suggested);
        // Assignment claimed with evidence that never names or involves the owner.
        assert_eq!(enforce_assignment(AssignmentType::ExplicitAssignment, Some("Sarah"), &ev("S2")), AssignmentType::Suggested);
        assert_eq!(enforce_assignment(AssignmentType::ExplicitAssignment, None, &ev("S1")), AssignmentType::Unclear);
    }

    #[test]
    fn confidence_is_capped_by_strength() {
        let (a, _) = validate(&raw(vec![action(Some("Sarah"), AssignmentType::Suggested, &["S3"], 0.95)]), &index());
        assert!(a.action_items[0].confidence <= 0.6);
    }

    #[test]
    fn spoken_due_dates_resolve_deterministically() {
        let fri = chrono::NaiveDate::from_ymd_opt(2026, 9, 25).unwrap(); // a Friday
        let d = |t: &str| resolve_due(t, fri).map(|d| d.to_string());
        assert_eq!(d("by Friday").as_deref(), Some("2026-10-02"));
        assert_eq!(d("Thursday").as_deref(), Some("2026-10-01"));
        assert_eq!(d("tomorrow").as_deref(), Some("2026-09-26"));
        assert_eq!(d("end of the week").as_deref(), Some("2026-09-25"));
        assert_eq!(d("next week sometime"), None);
        assert_eq!(d("Monday or Tuesday"), None);
    }

    #[test]
    fn implausible_model_dates_are_rejected() {
        let mut act = action(Some("Tom Hutchison"), AssignmentType::ExplicitAcceptance, &["S2"], 0.9);
        act.due_date = Some("5000-09-26".into());
        let (a, r) = validate(&raw(vec![act.clone()]), &index());
        assert!(!r.is_ok());
        assert_eq!(a.action_items[0].due_date, None);
        // The spoken text wins over the model's date.
        act.due = Some("by Friday".into());
        let (a, _) = validate(&raw(vec![act]), &index());
        assert_eq!(a.action_items[0].due_date.as_deref(), Some("2026-10-02"));
    }

    #[test]
    fn invalid_dates_are_reported() {
        let mut act = action(Some("Tom Hutchison"), AssignmentType::ExplicitAcceptance, &["S2"], 0.9);
        act.due_date = Some("next Friday".into());
        act.due = Some("sometime soon".into());
        let (a, r) = validate(&raw(vec![act]), &index());
        assert!(!r.is_ok());
        assert_eq!(a.action_items[0].due_date, None);
        assert_eq!(a.action_items[0].due_text.as_deref(), Some("sometime soon"));
    }
}
