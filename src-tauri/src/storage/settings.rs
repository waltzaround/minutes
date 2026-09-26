//! User settings. Stored as one JSON value per section in the `settings`
//! table; every field has a default so older rows keep loading after
//! upgrades. Secrets are never stored here (see `secrets.rs`).

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use ts_rs::TS;

use super::database::{now, Database};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct GeneralSettings {
    pub theme: ThemePreference,
    /// Show a reminder to tell participants the meeting is being recorded.
    pub consent_reminder: bool,
    /// Name of the person using this computer (maps microphone → person).
    pub self_person_id: Option<String>,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        GeneralSettings { theme: ThemePreference::System, consent_reminder: true, self_person_id: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AudioSettings {
    /// Persisted cpal device id; `None` = system default.
    pub microphone_device_id: Option<String>,
    /// Output device whose audio is captured as "meeting audio".
    pub output_device_id: Option<String>,
    pub capture_system_audio: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings { microphone_device_id: None, output_device_id: None, capture_system_audio: true }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AudioRetention {
    #[default]
    DeleteAfterProcessing,
    #[serde(rename = "keep_7_days")]
    Keep7Days,
    KeepForever,
}

impl AudioRetention {
    pub fn as_db(self) -> &'static str {
        match self {
            AudioRetention::DeleteAfterProcessing => "delete_after_processing",
            AudioRetention::Keep7Days => "keep_7_days",
            AudioRetention::KeepForever => "keep_forever",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct PrivacySettings {
    pub audio_retention: AudioRetention,
    /// Allow checking for app updates (the only non-integration network call
    /// besides user-initiated model downloads).
    pub check_for_updates: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum InferenceBackendPreference {
    #[default]
    Auto,
    Metal,
    Cuda,
    Vulkan,
    Cpu,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AdvancedSettings {
    /// Override for the models directory. `None` = app data directory.
    pub models_dir: Option<String>,
    /// LLM chosen by the user; `None` = follow the recommendation.
    pub llm_model_id: Option<String>,
    pub inference_backend: InferenceBackendPreference,
    pub context_tokens: Option<u32>,
    pub gpu_layers: Option<i32>,
    pub asr_threads: Option<u32>,
    /// ASR chunk length target in seconds (VAD segments longer than this are split).
    pub asr_chunk_seconds: f32,
    /// Cosine similarity at or above which a speaker is "known".
    pub speaker_known_threshold: f32,
    /// Cosine similarity at or above which a speaker is "possible".
    pub speaker_possible_threshold: f32,
    /// Debug builds only: evaluate a hardware fixture instead of this machine.
    pub simulate_hardware_fixture: Option<String>,
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        AdvancedSettings {
            models_dir: None,
            llm_model_id: None,
            inference_backend: InferenceBackendPreference::Auto,
            context_tokens: None,
            gpu_layers: None,
            asr_threads: None,
            asr_chunk_seconds: 20.0,
            // Starting points only. These must be calibrated on real
            // recordings for the chosen embedding model; see docs/models.md.
            speaker_known_threshold: 0.6,
            speaker_possible_threshold: 0.45,
            simulate_hardware_fixture: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NotionUploadMode {
    #[default]
    SummaryAndActions,
    SummaryAndTranscript,
    AskEachTime,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct NotionSettings {
    /// A token is stored in the credential store. Tracked here so the UI can
    /// show status without reading the keychain (which may prompt).
    pub connected: bool,
    pub data_source_id: Option<String>,
    pub data_source_name: Option<String>,
    pub upload_mode: NotionUploadMode,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct LinearSettings {
    /// See `NotionSettings::connected`.
    pub connected: bool,
    pub default_team_id: Option<String>,
    pub default_project_id: Option<String>,
    pub workspace_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct OnboardingState {
    pub completed: bool,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AppSettings {
    pub general: GeneralSettings,
    pub audio: AudioSettings,
    pub privacy: PrivacySettings,
    pub advanced: AdvancedSettings,
    pub notion: NotionSettings,
    pub linear: LinearSettings,
    pub onboarding: OnboardingState,
}

fn read_section<T: DeserializeOwned + Default>(db: &Database, key: &str) -> T {
    let raw: Option<String> = db
        .with(|c| {
            c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
                .map(Some)
                .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) })
        })
        .unwrap_or_else(|e| {
            tracing::warn!(key, error = %e, "failed to read settings section");
            None
        });
    raw.and_then(|s| {
        serde_json::from_str(&s)
            .map_err(|e| tracing::warn!(key, error = %e, "settings section is invalid; using defaults"))
            .ok()
    })
    .unwrap_or_default()
}

fn write_section<T: Serialize>(db: &Database, key: &str, value: &T) -> rusqlite::Result<()> {
    let json = serde_json::to_string(value).expect("settings serialize");
    db.with(|c| {
        c.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (key, json, now()),
        )
        .map(|_| ())
    })
}

impl AppSettings {
    pub fn load(db: &Database) -> Self {
        AppSettings {
            general: read_section(db, "general"),
            audio: read_section(db, "audio"),
            privacy: read_section(db, "privacy"),
            advanced: read_section(db, "advanced"),
            notion: read_section(db, "notion"),
            linear: read_section(db, "linear"),
            onboarding: read_section(db, "onboarding"),
        }
    }

    pub fn save(&self, db: &Database) -> rusqlite::Result<()> {
        write_section(db, "general", &self.general)?;
        write_section(db, "audio", &self.audio)?;
        write_section(db, "privacy", &self.privacy)?;
        write_section(db, "advanced", &self.advanced)?;
        write_section(db, "notion", &self.notion)?;
        write_section(db, "linear", &self.linear)?;
        write_section(db, "onboarding", &self.onboarding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_privacy_preserving() {
        let s = AppSettings::default();
        assert_eq!(s.privacy.audio_retention, AudioRetention::DeleteAfterProcessing);
        assert!(s.general.consent_reminder);
    }

    #[test]
    fn round_trip_and_partial_rows() {
        let db = Database::open_in_memory().unwrap();
        let mut s = AppSettings::load(&db);
        s.audio.capture_system_audio = false;
        s.privacy.audio_retention = AudioRetention::Keep7Days;
        s.save(&db).unwrap();
        let loaded = AppSettings::load(&db);
        assert!(!loaded.audio.capture_system_audio);
        assert_eq!(loaded.privacy.audio_retention, AudioRetention::Keep7Days);

        // A row written by an older version with missing fields still loads.
        db.with(|c| c.execute("UPDATE settings SET value = '{}' WHERE key = 'advanced'", [])).unwrap();
        assert_eq!(AppSettings::load(&db).advanced.asr_chunk_seconds, 20.0);
    }
}
