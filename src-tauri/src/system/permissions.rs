//! Privacy permissions (microphone, system audio).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PermissionState {
    Granted,
    Denied,
    /// Blocked by device management / parental controls.
    Restricted,
    /// The OS has not asked the user yet.
    NotDetermined,
    /// The platform does not gate this capability.
    NotRequired,
    /// The OS offers no way to query it; verified by a capture test instead.
    #[default]
    Unknown,
}

impl PermissionState {
    pub fn is_blocking(self) -> bool {
        matches!(self, PermissionState::Denied | PermissionState::Restricted)
    }
}

pub use super::platform::{
    microphone_permission, privacy_settings_url, request_microphone_permission, system_audio_permission,
};
