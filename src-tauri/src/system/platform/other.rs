//! Fallback probes for development on unsupported platforms (e.g. Linux CI).

use crate::system::capabilities::GpuInfo;
use crate::system::memory_budget::MemoryPressure;
use crate::system::permissions::PermissionState;

pub fn cpu_brand() -> Option<String> {
    None
}

pub fn memory_pressure() -> Option<MemoryPressure> {
    None
}

pub fn detect_gpus() -> Vec<GpuInfo> {
    Vec::new()
}

pub fn microphone_permission() -> PermissionState {
    PermissionState::Unknown
}

pub async fn request_microphone_permission() -> PermissionState {
    PermissionState::Unknown
}

pub fn system_audio_permission() -> PermissionState {
    PermissionState::Unknown
}

pub fn privacy_settings_url(_kind: &str) -> Option<&'static str> {
    None
}
