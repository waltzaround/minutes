use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::audio::devices::{self, AudioDeviceList};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::system::benchmark::{self, BenchmarkSummary};
use crate::system::fixtures::{self, HardwareFixture};
use crate::system::permissions::{self, PermissionState};
use crate::system::profile::CapabilityAssessment;
use crate::system::service::SystemCapabilities;

fn simulate(state: &AppState) -> Option<String> {
    state.settings.read().advanced.simulate_hardware_fixture.clone()
}

/// Full detection when `refresh` is true (first run / "Check again"),
/// otherwise the lightweight check.
#[tauri::command]
pub async fn get_capabilities(state: State<'_, AppState>, refresh: bool) -> AppResult<SystemCapabilities> {
    let system = state.system.clone();
    let sim = simulate(&state);
    tauri::async_runtime::spawn_blocking(move || system.capabilities(refresh, sim.as_deref()))
        .await
        .map_err(|e| AppError::Internal(e.into()))
}

#[tauri::command]
pub async fn get_recommended_profile(state: State<'_, AppState>) -> AppResult<CapabilityAssessment> {
    Ok(get_capabilities(state, false).await?.assessment)
}

/// Run one benchmark stage. `synthetic` needs no models. `asr` and `llm`
/// require installed models and are reported as unavailable until the
/// corresponding runtime is installed.
#[tauri::command]
pub async fn run_benchmark(state: State<'_, AppState>, stage: String) -> AppResult<BenchmarkSummary> {
    let system = state.system.clone();
    match stage.as_str() {
        "synthetic" => {
            tauri::async_runtime::spawn_blocking(move || -> AppResult<BenchmarkSummary> {
                let hw = system.cached_hardware().unwrap_or_else(|| system.detect_full());
                let threads = hw.cpu.physical_cores.unwrap_or(hw.cpu.logical_cores).max(1) as usize;
                let result = benchmark::run_synthetic(threads);
                system.save_benchmark(&hw, "synthetic", None, true, &result)?;
                Ok(system.load_benchmarks(&hw))
            })
            .await
            .map_err(|e| AppError::Internal(e.into()))?
        }
        "asr" => {
            let engine = state.speech.clone();
            let recorder = state.recorder.clone();
            let models = state.models.clone();
            tauri::async_runtime::spawn_blocking(move || -> AppResult<BenchmarkSummary> {
                let hw = system.cached_hardware().unwrap_or_else(|| system.detect_full());
                let result = benchmark::run_asr(&engine, &models);
                if !recorder.is_recording() {
                    engine.unload_asr();
                }
                let result = result.map_err(|e| AppError::Unavailable(e.to_string()))?;
                let passed = result.error.is_none() && result.output_plausible;
                system.save_benchmark(&hw, "asr", Some(&result.model_id), passed, &result)?;
                Ok(system.load_benchmarks(&hw))
            })
            .await
            .map_err(|e| AppError::Internal(e.into()))?
        }
        "llm" => {
            let analysis = state.analysis.clone();
            let models = state.models.clone();
            let db = state.db.clone();
            tauri::async_runtime::spawn_blocking(move || -> AppResult<BenchmarkSummary> {
                let hw = system.cached_hardware().unwrap_or_else(|| system.detect_full());
                let mut ran = false;
                for id in [crate::models::catalog::LLM_SMALL_ID, crate::models::catalog::LLM_LARGE_ID] {
                    if crate::llm::model_manager::model_path(&db, &models, id).is_none() {
                        continue;
                    }
                    let result = analysis.benchmark(id).map_err(|e| AppError::Unavailable(e))?;
                    let passed = result.error.is_none() && result.stable && result.structured_output_valid;
                    system.save_benchmark(&hw, "llm", Some(id), passed, &result)?;
                    ran = true;
                }
                if !ran {
                    return Err(AppError::Unavailable("Install the meeting summary model to run this test.".into()));
                }
                Ok(system.load_benchmarks(&hw))
            })
            .await
            .map_err(|e| AppError::Internal(e.into()))?
        }
        _ => Err(AppError::user("invalid_stage", "Unknown benchmark stage.")),
    }
}

#[tauri::command]
pub fn list_audio_devices() -> AudioDeviceList {
    devices::list_devices()
}

#[tauri::command]
pub async fn request_microphone_permission() -> PermissionState {
    permissions::request_microphone_permission().await
}

/// Open the OS privacy settings pane for `kind` ("microphone" | "systemAudio").
#[tauri::command]
pub fn open_privacy_settings(app: AppHandle, kind: String) -> AppResult<()> {
    let url = permissions::privacy_settings_url(&kind)
        .ok_or_else(|| AppError::Unavailable("There is no settings page for this on your system.".into()))?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("open settings: {e}")))
}

/// Development fixtures (debug builds only).
#[tauri::command]
pub fn list_hardware_fixtures() -> Vec<HardwareFixture> {
    if cfg!(debug_assertions) {
        fixtures::all()
    } else {
        Vec::new()
    }
}
