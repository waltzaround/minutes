//! Audio device enumeration and system-audio availability.

use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::system::capabilities::{AudioCapabilities, AudioDeviceInfo, HardwareSnapshot, OsKind};
use crate::system::permissions::{self, PermissionState};

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioDeviceList {
    pub host: String,
    pub inputs: Vec<AudioDeviceInfo>,
    pub outputs: Vec<AudioDeviceInfo>,
}

fn describe(device: &cpal::Device, default_id: Option<&str>, input: bool) -> Option<AudioDeviceInfo> {
    let id = device.id().ok()?.to_string();
    let name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| device.to_string());
    let config = if input { device.default_input_config() } else { device.default_output_config() };
    let (channels, sample_rate) = match config {
        Ok(c) => (Some(c.channels()), Some(c.sample_rate())),
        Err(_) => (None, None),
    };
    Some(AudioDeviceInfo { is_default: default_id == Some(id.as_str()), id, name, channels, sample_rate })
}

pub fn list_devices() -> AudioDeviceList {
    let host = cpal::default_host();
    let default_in = host.default_input_device().and_then(|d| d.id().ok()).map(|i| i.to_string());
    let default_out = host.default_output_device().and_then(|d| d.id().ok()).map(|i| i.to_string());

    let inputs = host
        .input_devices()
        .map(|it| it.filter_map(|d| describe(&d, default_in.as_deref(), true)).collect())
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to enumerate input devices");
            Vec::new()
        });
    let outputs = host
        .output_devices()
        .map(|it| it.filter_map(|d| describe(&d, default_out.as_deref(), false)).collect())
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to enumerate output devices");
            Vec::new()
        });

    AudioDeviceList { host: host.id().name().to_string(), inputs, outputs }
}

/// Find a device by persisted id, falling back to the default.
pub fn resolve_device(id: Option<&str>, input: bool) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if let Some(id) = id {
        if let Ok(parsed) = id.parse::<cpal::DeviceId>() {
            if let Some(d) = host.device_by_id(&parsed) {
                return Some(d);
            }
        }
        tracing::warn!(id, "saved audio device not found; using default");
    }
    if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    }
}

/// Why meeting-audio capture is unavailable on this machine, if it is.
pub fn system_audio_unavailable_reason(hw: &HardwareSnapshot, outputs: &[AudioDeviceInfo]) -> Option<String> {
    match hw.os() {
        OsKind::Macos => {
            if !hw.os_version_parts.is_some_and(|v| v >= (14, 6, 0)) {
                return Some("Capturing meeting audio needs macOS 14.6 or newer.".into());
            }
        }
        OsKind::Windows => {}
        OsKind::Other => return Some("Meeting audio capture is only supported on macOS and Windows.".into()),
    }
    if outputs.is_empty() {
        return Some("No speakers or headphones were found to capture meeting audio from.".into());
    }
    None
}

/// Fill the audio part of a hardware snapshot.
pub fn detect_audio(hw: &HardwareSnapshot) -> AudioCapabilities {
    let list = list_devices();
    let reason = system_audio_unavailable_reason(hw, &list.outputs);
    let mic_perm = permissions::microphone_permission();
    AudioCapabilities {
        microphone_available: !list.inputs.is_empty(),
        microphone_permission: mic_perm,
        system_audio_available: reason.is_none(),
        system_audio_permission: if reason.is_some() {
            PermissionState::Unknown
        } else {
            permissions::system_audio_permission()
        },
        system_audio_unavailable_reason: reason,
        output_device: list.outputs.iter().find(|d| d.is_default).cloned().or_else(|| list.outputs.first().cloned()),
        input_devices: list.inputs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::fixtures;

    #[test]
    fn old_macos_has_no_system_audio() {
        let mut hw = fixtures::load("mac-m1-16gb").hardware;
        hw.os_version_parts = Some((14, 2, 0));
        let out = vec![AudioDeviceInfo { id: "x".into(), name: "Speakers".into(), ..Default::default() }];
        assert!(system_audio_unavailable_reason(&hw, &out).unwrap().contains("14.6"));
        hw.os_version_parts = Some((14, 6, 0));
        assert!(system_audio_unavailable_reason(&hw, &out).is_none());
        assert!(system_audio_unavailable_reason(&hw, &[]).is_some());
    }

    #[test]
    fn enumeration_does_not_panic() {
        let list = list_devices();
        assert!(!list.host.is_empty());
    }
}
