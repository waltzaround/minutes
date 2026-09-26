//! Meetings: recording sessions, transcripts, processing and analysis.

pub mod analysis;
pub mod processor;
pub mod recorder;
pub mod speakers;
pub mod store;
pub mod transcript;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MeetingStatus {
    Recording,
    Processing,
    Ready,
    Failed,
    /// The app stopped while recording; audio can be recovered.
    Interrupted,
}

impl MeetingStatus {
    pub fn as_db(self) -> &'static str {
        match self {
            MeetingStatus::Recording => "recording",
            MeetingStatus::Processing => "processing",
            MeetingStatus::Ready => "ready",
            MeetingStatus::Failed => "failed",
            MeetingStatus::Interrupted => "interrupted",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "recording" => MeetingStatus::Recording,
            "processing" => MeetingStatus::Processing,
            "ready" => MeetingStatus::Ready,
            "interrupted" => MeetingStatus::Interrupted,
            _ => MeetingStatus::Failed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingSummary {
    pub id: String,
    pub title: String,
    pub status: MeetingStatus,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub speaker_count: u32,
    pub action_count: u32,
    pub segment_count: u32,
}
