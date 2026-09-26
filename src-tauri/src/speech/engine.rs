//! Loads speech models on demand and releases them when no longer needed,
//! so ML models are not kept resident for the lifetime of the app.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use super::asr::ParakeetRecognizer;
use super::SpeechRecognizer;
use crate::models::catalog::{ASR_ID, DIARIZATION_ID, EMBEDDING_ID, VAD_ID};
use crate::models::manager::ModelManager;

pub struct SpeechEngine {
    models: Arc<ModelManager>,
    asr: Mutex<Option<Arc<ParakeetRecognizer>>>,
    /// Folder holding the bundled `nemo-speech` diarization runtime.
    diarization_runtime: Mutex<PathBuf>,
    /// Private scratch folder for temporary diarization audio.
    work_dir: Mutex<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum SpeechUnavailable {
    #[error("The transcription models are not installed. Download them in Settings → Models.")]
    NotInstalled,
    #[error("The transcription model could not be loaded: {0}")]
    LoadFailed(String),
}

impl SpeechEngine {
    pub fn new(models: Arc<ModelManager>) -> Self {
        SpeechEngine {
            models,
            asr: Mutex::new(None),
            diarization_runtime: Mutex::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/nemo-speech")),
            work_dir: Mutex::new(std::env::temp_dir().join("minutes-diarization")),
        }
    }

    pub fn set_diarization_runtime(&self, runtime_dir: PathBuf, work_dir: PathBuf) {
        *self.diarization_runtime.lock() = runtime_dir;
        *self.work_dir.lock() = work_dir;
    }

    pub fn diarization_runtime(&self) -> (PathBuf, PathBuf) {
        (self.diarization_runtime.lock().clone(), self.work_dir.lock().clone())
    }

    pub fn vad_model(&self) -> Option<PathBuf> {
        self.models.installed_dir(VAD_ID).map(|d| d.join("silero_vad.onnx"))
    }

    pub fn asr_installed(&self) -> bool {
        self.models.installed_dir(ASR_ID).is_some() && self.vad_model().is_some()
    }

    /// Nemotron 3 Diarization GGUF, if installed.
    pub fn diarization_model(&self) -> Option<PathBuf> {
        self.models.installed_dir(DIARIZATION_ID).map(|d| d.join("Nemotron-3-Diarization.q8_0.gguf"))
    }

    pub fn embedding_model(&self) -> Option<PathBuf> {
        self.models.installed_dir(EMBEDDING_ID).map(|d| d.join("nemo_en_titanet_small.onnx"))
    }

    /// Get (loading if necessary) the ASR model. Blocking; call off the UI thread.
    pub fn asr(&self, threads: u32) -> Result<Arc<dyn SpeechRecognizer>, SpeechUnavailable> {
        let mut guard = self.asr.lock();
        if let Some(a) = guard.as_ref() {
            return Ok(a.clone());
        }
        let dir = self.models.installed_dir(ASR_ID).ok_or(SpeechUnavailable::NotInstalled)?;
        let started = std::time::Instant::now();
        let asr = Arc::new(ParakeetRecognizer::load(&dir, threads).map_err(|e| SpeechUnavailable::LoadFailed(e.to_string()))?);
        tracing::info!(ms = started.elapsed().as_millis() as u64, "transcription model loaded");
        *guard = Some(asr.clone());
        Ok(asr)
    }

    /// Release the ASR model. In-flight users keep their `Arc` until done.
    pub fn unload_asr(&self) {
        if self.asr.lock().take().is_some() {
            tracing::info!("transcription model unloaded");
        }
    }

    pub fn asr_loaded(&self) -> bool {
        self.asr.lock().is_some()
    }
}
