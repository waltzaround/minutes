//! Shared application state managed by Tauri.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::audio::capture::SpeechBlock;
use crate::meetings::analysis::{AnalysisDeps, AnalysisService};
use crate::meetings::processor::{Job, Processor, ProcessorDeps};
use crate::meetings::recorder::{EventSink, Recorder};
use crate::meetings::transcript::{self, LiveTranscriber};
use crate::meetings::speakers::SpeakerRefinement;
use crate::people::enrollment::EnrollmentManager;
use crate::speech::engine::SpeechEngine;
use crate::models::download::Downloader;
use crate::models::manager::ModelManager;
use crate::storage::secrets::{OsSecretStore, SecretStore};
use crate::storage::settings::AppSettings;
use crate::storage::Database;
use crate::system::service::SystemService;

pub struct AppPaths {
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub meetings_dir: PathBuf,
    pub default_models_dir: PathBuf,
}

pub struct AppState {
    pub paths: AppPaths,
    pub db: Database,
    pub settings: Arc<RwLock<AppSettings>>,
    pub system: Arc<SystemService>,
    pub secrets: Arc<dyn SecretStore>,
    pub models: Arc<ModelManager>,
    pub downloader: Arc<Downloader>,
    pub recorder: Arc<Recorder>,
    pub speech: Arc<SpeechEngine>,
    pub processor: Processor,
    pub sink: EventSink,
    pub enrollment: EnrollmentManager,
    pub analysis: Arc<AnalysisService>,
    pub llama_runtime_dir: PathBuf,
    live: parking_lot::Mutex<Option<(String, LiveTranscriber)>>,
}

impl AppState {
    pub fn new(paths: AppPaths, sink: EventSink, llama_runtime_dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&paths.data_dir)?;
        std::fs::create_dir_all(&paths.meetings_dir)?;
        let db = Database::open(&paths.data_dir.join("minutes.db"))?;
        let settings = AppSettings::load(&db);
        let system = Arc::new(SystemService::new(paths.data_dir.clone(), db.clone()));
        let models_dir = settings
            .advanced
            .models_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| paths.default_models_dir.clone());
        let models = Arc::new(ModelManager::new(db.clone(), models_dir));
        let downloader = Arc::new(Downloader::new(models.clone()));
        let recorder = Arc::new(Recorder::new(db.clone(), sink.clone()));
        let speech = Arc::new(SpeechEngine::new(models.clone()));
        let settings = Arc::new(RwLock::new(settings));
        let interrupted = crate::meetings::store::mark_interrupted(&db)?;
        if !interrupted.is_empty() {
            tracing::warn!(count = interrupted.len(), "found meetings interrupted by a previous crash");
        }
        let rec_for_analysis = recorder.clone();
        let analysis = Arc::new(AnalysisService::start(AnalysisDeps {
            db: db.clone(),
            settings: settings.clone(),
            system: system.clone(),
            models: models.clone(),
            speech: speech.clone(),
            sink: sink.clone(),
            runtime_dir: llama_runtime_dir.clone(),
            log_dir: paths.log_dir.clone(),
            data_dir: paths.data_dir.clone(),
            is_recording: Arc::new(move || rec_for_analysis.is_recording()),
        }));
        let rec = recorder.clone();
        let secrets: Arc<dyn SecretStore> = Arc::new(OsSecretStore);
        let speaker_step = Arc::new(SpeakerRefinement {
            db: db.clone(),
            engine: speech.clone(),
            settings: settings.clone(),
            secrets: secrets.clone(),
        });
        let processor = Processor::start(ProcessorDeps {
            db: db.clone(),
            engine: speech.clone(),
            sink: sink.clone(),
            settings: settings.clone(),
            is_recording: Arc::new(move || rec.is_recording()),
            speaker_step: Some(speaker_step),
            on_ready: Some({
                let analysis = analysis.clone();
                Arc::new(move |id: &str| analysis.enqueue(id))
            }),
        });
        // Meetings whose processing was cut short by a quit or crash.
        let pending: Vec<String> = db.with(|c| {
            let mut stmt = c.prepare("SELECT id FROM meetings WHERE status = 'processing' ORDER BY started_at")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect()
        })?;
        for id in pending {
            processor.enqueue(Job { meeting_id: id, live: None });
        }
        crate::meetings::processor::apply_retention_all(&db);
        Ok(AppState {
            paths,
            db,
            settings,
            system,
            secrets,
            models,
            downloader,
            recorder,
            speech,
            processor,
            sink,
            enrollment: EnrollmentManager::default(),
            analysis,
            llama_runtime_dir,
            live: parking_lot::Mutex::new(None),
        })
    }

    pub fn models_dir(&self) -> PathBuf {
        self.settings
            .read()
            .advanced
            .models_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.paths.default_models_dir.clone())
    }

    /// Watch memory pressure. Under high pressure: release idle models and
    /// warn the user (once per episode) rather than risking swap or a crash.
    pub fn start_memory_monitor(&self) {
        struct Handles {
            system: Arc<SystemService>,
            recorder: Arc<Recorder>,
            speech: Arc<SpeechEngine>,
            analysis: Arc<AnalysisService>,
            sink: EventSink,
        }
        let state = Handles {
            system: self.system.clone(),
            recorder: self.recorder.clone(),
            speech: self.speech.clone(),
            analysis: self.analysis.clone(),
            sink: self.sink.clone(),
        };
        std::thread::Builder::new()
            .name("memory-monitor".into())
            .spawn(move || {
                let mut warned = false;
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(20));
                    let hw = state.system.detect_light();
                    let high = hw.memory.pressure == Some(crate::system::memory_budget::MemoryPressure::High);
                    if high && !warned {
                        warned = true;
                        if !state.recorder.is_recording() {
                            state.speech.unload_asr();
                        }
                        state.analysis.shutdown_runtime();
                        let msg = if state.recorder.is_recording() {
                            "Your computer is running low on memory. Recording continues; closing other apps will keep things smooth."
                        } else {
                            "Your computer is running low on memory, so Minutes released its AI models. They reload when needed."
                        };
                        let payload = crate::events::MemoryWarning { message: msg.into(), available_bytes: hw.memory.available_bytes };
                        (state.sink)(crate::events::MEMORY_WARNING, serde_json::to_value(&payload).unwrap_or_default());
                    } else if !high {
                        warned = false;
                    }
                }
            })
            .expect("spawn memory monitor");
    }

    /// Start live transcription for a meeting about to be recorded, if the
    /// models are installed and the capability profile allows it.
    pub fn begin_live_transcription(&self, realtime_allowed: bool) -> Option<std::sync::mpsc::SyncSender<SpeechBlock>> {
        if !realtime_allowed || !self.speech.asr_installed() {
            return None;
        }
        let (tx, live) = transcript::start_live(
            {
                let db = self.db.clone();
                let engine = self.speech.clone();
                let settings = self.settings.read().clone();
                let sink = self.sink.clone();
                let recorder = self.recorder.clone();
                move || {
                    // The meeting id is only known once recording starts.
                    let meeting_id = wait_for_meeting_id(&recorder)?;
                    let deps = ProcessorDeps {
                        db,
                        engine,
                        sink,
                        settings: Arc::new(RwLock::new(settings.clone())),
                        is_recording: Arc::new(|| true),
                        speaker_step: None,
                        on_ready: None,
                    };
                    crate::meetings::processor::transcribe_ctx(&deps, &meeting_id, &settings)
                }
            },
            self.sink.clone(),
        );
        *self.live.lock() = Some((String::new(), live));
        Some(tx)
    }

    /// Drop a live session that never got a recording (start failed).
    pub fn abandon_live_transcription(&self) {
        self.live.lock().take();
    }

    /// Queue post-recording processing, handing over the live transcriber.
    pub fn on_recording_stopped(&self, meeting_id: &str) {
        let live = self.live.lock().take().map(|(_, l)| l);
        self.processor.enqueue(Job { meeting_id: meeting_id.to_string(), live });
    }
}

fn wait_for_meeting_id(recorder: &Recorder) -> anyhow::Result<String> {
    for _ in 0..200 {
        if let Some(s) = recorder.status() {
            return Ok(s.meeting_id);
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    anyhow::bail!("recording did not start")
}
