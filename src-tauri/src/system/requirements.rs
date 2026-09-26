//! Published minimum/recommended system requirements and the check that
//! compares a machine against them.
//!
//! This answers "is this machine officially supported?". It is separate from
//! the capability tier (`profile.rs`), which answers "what can this machine
//! comfortably run?".

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::capabilities::{HardwareSnapshot, OsKind};
use super::memory_budget::GIB;

/// Physical memory below this is "limited mode". 16 GB machines report
/// between ~15.7 and 16 GiB depending on firmware reservations.
pub const SUPPORTED_MEMORY_BYTES: u64 = 15 * GIB;
/// Below this even the speech models are unlikely to run reliably.
pub const LIMITED_MEMORY_BYTES: u64 = 7 * GIB;
pub const MIN_FREE_DISK_BYTES: u64 = 10 * GIB;
pub const RECOMMENDED_FREE_DISK_FULL_BYTES: u64 = 30 * GIB;
/// Enough for the speech models and some meeting audio, but not an LLM.
pub const SPEECH_ONLY_DISK_BYTES: u64 = 4 * GIB;
pub const MIN_MACOS: (u32, u32, u32) = (14, 6, 0);
/// Windows 10 RTM.
pub const MIN_WINDOWS_BUILD: u32 = 10240;
pub const MIN_PHYSICAL_CORES: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RequirementCheck {
    pub id: String,
    pub label: String,
    pub status: CheckStatus,
    /// Plain-language explanation suitable for office staff.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SupportLevel {
    /// Meets the published minimum.
    Supported,
    /// Usable, but below the published minimum (e.g. 8 GB memory).
    Limited,
    /// Fails a hard requirement. The user may still continue unless the
    /// speech benchmark shows transcription cannot work.
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RequirementReport {
    pub level: SupportLevel,
    pub meets_official_minimum: bool,
    pub checks: Vec<RequirementCheck>,
}

fn check(id: &str, label: &str, status: CheckStatus, detail: impl Into<String>) -> RequirementCheck {
    RequirementCheck { id: id.into(), label: label.into(), status, detail: detail.into() }
}

fn gb(bytes: u64) -> String {
    format!("{:.0} GB", bytes as f64 / GIB as f64)
}

pub fn evaluate(hw: &HardwareSnapshot) -> RequirementReport {
    let mut checks = Vec::new();

    // Operating system and architecture.
    match hw.os() {
        OsKind::Macos => {
            let ok = hw.os_version_parts.is_some_and(|v| v >= MIN_MACOS);
            checks.push(check(
                "os",
                "Operating system",
                if ok { CheckStatus::Pass } else { CheckStatus::Fail },
                if ok {
                    format!("{} is supported.", hw.os_version)
                } else {
                    format!("{} is too old. macOS 14.6 or newer is needed to capture meeting audio.", hw.os_version)
                },
            ));
            let arm = hw.is_apple_silicon();
            checks.push(check(
                "architecture",
                "Processor",
                if arm { CheckStatus::Pass } else { CheckStatus::Warn },
                if arm {
                    "Apple Silicon.".to_string()
                } else {
                    "Intel Macs are not officially supported. Local AI will be slow.".to_string()
                },
            ));
        }
        OsKind::Windows => {
            let ok = hw.os_build.is_none_or(|b| b >= MIN_WINDOWS_BUILD) && hw.architecture == "x86_64";
            checks.push(check(
                "os",
                "Operating system",
                if ok { CheckStatus::Pass } else { CheckStatus::Fail },
                if ok {
                    format!("{} is supported.", hw.os_version)
                } else {
                    "Windows 10 64-bit or newer on an Intel or AMD processor is required.".to_string()
                },
            ));
        }
        OsKind::Other => checks.push(check(
            "os",
            "Operating system",
            CheckStatus::Warn,
            "This operating system is only supported for development.",
        )),
    }

    // CPU.
    if hw.os() == OsKind::Windows || hw.architecture == "x86_64" {
        match hw.cpu.physical_cores {
            Some(c) if c < MIN_PHYSICAL_CORES => checks.push(check(
                "cpu",
                "Processor cores",
                CheckStatus::Warn,
                format!("{c} cores detected. At least {MIN_PHYSICAL_CORES} are recommended for live transcription."),
            )),
            _ => checks.push(check("cpu", "Processor cores", CheckStatus::Pass, "Enough processor cores.")),
        }
        if hw.cpu.avx2 == Some(false) {
            checks.push(check(
                "cpu_features",
                "Processor features",
                CheckStatus::Warn,
                "This processor lacks AVX2, so local AI will run much more slowly.",
            ));
        }
    }

    // Memory.
    let total = hw.memory.total_bytes;
    let mem_status = if total >= SUPPORTED_MEMORY_BYTES {
        CheckStatus::Pass
    } else if total >= LIMITED_MEMORY_BYTES {
        CheckStatus::Warn
    } else {
        CheckStatus::Fail
    };
    checks.push(check(
        "memory",
        "Memory",
        mem_status,
        match mem_status {
            CheckStatus::Pass => format!("{} of memory.", gb(total)),
            CheckStatus::Warn => format!(
                "Your computer has {} of memory. Recording and transcription may work, but local AI summaries can be \
                 slower and other applications may affect reliability. 16 GB or more is recommended.",
                gb(total)
            ),
            CheckStatus::Fail => format!("Your computer has {} of memory, which is not enough for local AI.", gb(total)),
        },
    ));

    // Disk.
    let free = hw.disk.available_bytes;
    let disk_status = if free >= MIN_FREE_DISK_BYTES {
        CheckStatus::Pass
    } else if free >= SPEECH_ONLY_DISK_BYTES {
        CheckStatus::Warn
    } else {
        CheckStatus::Fail
    };
    checks.push(check(
        "disk",
        "Free disk space",
        disk_status,
        match disk_status {
            CheckStatus::Pass => format!("{} free.", gb(free)),
            CheckStatus::Warn => format!("{} free. 10 GB is needed for meeting summaries.", gb(free)),
            CheckStatus::Fail => format!("{} free. Free up at least 10 GB to continue.", gb(free)),
        },
    ));

    // Audio.
    checks.push(check(
        "microphone",
        "Microphone",
        if hw.audio.microphone_available { CheckStatus::Pass } else { CheckStatus::Warn },
        if hw.audio.microphone_available {
            "A microphone is available.".to_string()
        } else {
            "No microphone was found. Connect one to record your own voice.".to_string()
        },
    ));
    checks.push(check(
        "system_audio",
        "Meeting audio",
        if hw.audio.system_audio_available { CheckStatus::Pass } else { CheckStatus::Warn },
        if hw.audio.system_audio_available {
            "Meeting audio can be captured.".to_string()
        } else {
            hw.audio
                .system_audio_unavailable_reason
                .clone()
                .unwrap_or_else(|| "Meeting audio capture is not available.".into())
        },
    ));

    let worst = checks.iter().map(|c| c.status).max().unwrap_or(CheckStatus::Pass);
    let hard_ids = ["os", "memory", "disk"];
    let hard_fail = checks.iter().any(|c| c.status == CheckStatus::Fail && hard_ids.contains(&c.id.as_str()));
    let meets_official_minimum = !checks
        .iter()
        .any(|c| c.status != CheckStatus::Pass && ["os", "architecture", "memory", "disk"].contains(&c.id.as_str()));

    let level = if hard_fail {
        SupportLevel::Unsupported
    } else if meets_official_minimum {
        SupportLevel::Supported
    } else if worst >= CheckStatus::Warn {
        SupportLevel::Limited
    } else {
        SupportLevel::Supported
    };

    RequirementReport { level, meets_official_minimum, checks }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::fixtures;

    #[test]
    fn m1_16gb_is_supported() {
        let r = evaluate(&fixtures::load("mac-m1-16gb").hardware);
        assert_eq!(r.level, SupportLevel::Supported);
        assert!(r.meets_official_minimum);
    }

    #[test]
    fn eight_gb_windows_is_limited_not_blocked() {
        let r = evaluate(&fixtures::load("win-intel-8gb").hardware);
        assert_eq!(r.level, SupportLevel::Limited);
        assert!(!r.meets_official_minimum);
        let mem = r.checks.iter().find(|c| c.id == "memory").unwrap();
        assert_eq!(mem.status, CheckStatus::Warn);
        assert!(mem.detail.contains("16 GB or more is recommended"));
    }

    #[test]
    fn old_macos_fails() {
        let mut hw = fixtures::load("mac-m1-16gb").hardware;
        hw.os_version_parts = Some((14, 5, 0));
        let r = evaluate(&hw);
        assert_eq!(r.level, SupportLevel::Unsupported);
    }

    #[test]
    fn low_disk_fails_and_medium_disk_warns() {
        let mut hw = fixtures::load("mac-m1-16gb").hardware;
        hw.disk.available_bytes = 2 * GIB;
        assert_eq!(evaluate(&hw).level, SupportLevel::Unsupported);
        hw.disk.available_bytes = 6 * GIB;
        let r = evaluate(&hw);
        assert_eq!(r.level, SupportLevel::Limited);
    }

    #[test]
    fn missing_system_audio_is_a_warning_only() {
        let mut hw = fixtures::load("mac-m1-16gb").hardware;
        hw.audio.system_audio_available = false;
        let r = evaluate(&hw);
        assert!(r.meets_official_minimum);
        assert_eq!(r.checks.iter().find(|c| c.id == "system_audio").unwrap().status, CheckStatus::Warn);
    }

    #[test]
    fn all_fixtures_evaluate() {
        for f in fixtures::all() {
            let r = evaluate(&f.hardware);
            assert!(!r.checks.is_empty(), "{}", f.id);
        }
    }
}
