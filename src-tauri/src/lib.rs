//! Minutes: private, local-first meeting intelligence.

pub mod audio;
pub mod commands;
pub mod error;
pub mod events;
pub mod integrations;
pub mod llm;
pub mod meetings;
pub mod speech;
pub mod models;
pub mod people;
pub mod state;
pub mod storage;
pub mod system;

use std::sync::Arc;

use tauri::{Emitter, Manager};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use state::{AppPaths, AppState};

fn init_logging(log_dir: &std::path::Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    std::fs::create_dir_all(log_dir).ok()?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("minutes")
        .filename_suffix("log")
        .max_log_files(7)
        .build(log_dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("MINUTES_LOG").unwrap_or_else(|_| EnvFilter::new("info,minutes_lib=debug"));
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(writer).with_ansi(false));
    if cfg!(debug_assertions) {
        registry.with(fmt::layer().with_writer(std::io::stderr)).try_init().ok();
    } else {
        registry.try_init().ok();
    }
    Some(guard)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let path = app.path();
            let data_dir = path.app_data_dir()?;
            let log_dir = path.app_log_dir()?;
            let guard = init_logging(&log_dir);
            tracing::info!(version = env!("CARGO_PKG_VERSION"), data_dir = %data_dir.display(), "starting");
            let paths = AppPaths {
                meetings_dir: data_dir.join("meetings"),
                default_models_dir: data_dir.join("models"),
                data_dir,
                log_dir,
            };
            let handle = app.handle().clone();
            let sink: meetings::recorder::EventSink = Arc::new(move |name, payload| {
                if let Err(e) = handle.emit(name, payload) {
                    tracing::debug!(event = name, error = %e, "emit failed");
                }
            });
            // Bundled llama.cpp runtime; during development it lives in src-tauri/binaries.
            let bundled = path.resource_dir().map(|d| d.join("llama")).unwrap_or_default();
            let runtime_dir = if llm::llama::server_binary(&bundled).exists() || !cfg!(debug_assertions) {
                bundled
            } else {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/llama")
            };
            let state = AppState::new(paths, sink, runtime_dir).map_err(|e| {
                tracing::error!(error = ?e, "failed to initialise application state");
                e
            })?;
            // Bundled NeMo-Speech.cpp diarization runtime (dev: src-tauri/binaries).
            let bundled_nemo = path.resource_dir().map(|d| d.join("nemo-speech")).unwrap_or_default();
            let nemo_dir = if speech::diarization::runtime_binary(&bundled_nemo).exists() || !cfg!(debug_assertions) {
                bundled_nemo
            } else {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/nemo-speech")
            };
            let diar_tmp = state.paths.data_dir.join("tmp");
            let _ = std::fs::remove_dir_all(&diar_tmp); // leftovers from a crash
            state.speech.set_diarization_runtime(nemo_dir, diar_tmp);
            state.start_memory_monitor();
            app.manage(state);
            if let Some(g) = guard {
                app.manage(g);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::get_app_info,
            commands::app::get_settings,
            commands::app::update_settings,
            commands::app::complete_onboarding,
            commands::app::storage_usage,
            commands::app::export_diagnostics,
            commands::system::get_capabilities,
            commands::system::get_recommended_profile,
            commands::system::run_benchmark,
            commands::system::list_audio_devices,
            commands::system::request_microphone_permission,
            commands::system::open_privacy_settings,
            commands::system::list_hardware_fixtures,
            commands::models::list_models,
            commands::models::download_model,
            commands::models::pause_download,
            commands::models::cancel_download,
            commands::models::remove_model,
            commands::models::required_download_space,
            commands::meetings::start_meeting,
            commands::meetings::stop_meeting,
            commands::meetings::get_recording_status,
            commands::meetings::reconnect_source,
            commands::meetings::list_meetings,
            commands::meetings::rename_meeting,
            commands::meetings::delete_meeting,
            commands::meetings::list_interrupted_meetings,
            commands::meetings::recover_meeting,
            commands::meetings::reprocess_meeting,
            commands::meetings::get_meeting,
            commands::meetings::search_meetings,
            commands::meetings::list_speakers,
            commands::meetings::rename_speaker,
            commands::meetings::merge_speakers,
            commands::meetings::split_segment,
            commands::meetings::edit_segment,
            commands::people::list_people,
            commands::people::create_person,
            commands::people::update_person,
            commands::people::delete_person,
            commands::people::export_people,
            commands::people::start_voice_enrollment,
            commands::people::cancel_voice_enrollment,
            commands::people::finish_voice_enrollment,
            commands::people::delete_voice_profile,
            commands::people::clear_all_voice_data,
            commands::people::update_speaker_identity,
            commands::analysis::analyse_meeting,
            commands::analysis::get_analysis,
            commands::analysis::update_action_item,
            commands::analysis::add_action_item,
            commands::analysis::list_llm_choices,
            commands::analysis::import_gguf,
            commands::integrations::integrations_status,
            commands::integrations::notion_connect,
            commands::integrations::notion_disconnect,
            commands::integrations::notion_search_databases,
            commands::integrations::notion_select_database,
            commands::integrations::notion_users,
            commands::integrations::linear_connect,
            commands::integrations::linear_disconnect,
            commands::integrations::linear_directory,
            commands::integrations::set_person_mapping,
            commands::integrations::set_linear_defaults,
            commands::integrations::sync_to_notion,
            commands::integrations::prepare_linear_issues,
            commands::integrations::create_linear_issue,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Minutes")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    state.enrollment.cancel();
                    if state.recorder.is_recording() {
                        // Finalise audio so nothing is lost; processing resumes next launch.
                        let _ = state.recorder.stop();
                    }
                    state.analysis.shutdown_runtime();
                }
            }
        });
}
