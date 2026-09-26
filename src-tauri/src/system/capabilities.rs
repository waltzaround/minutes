//! Hardware and system capability detection.
//!
//! Detection uses `sysinfo` for the cross-platform baseline and small native
//! modules (`system::macos`, `system::windows`) for GPU, memory-pressure and
//! permission details. No shell commands are parsed.
//!
//! The raw snapshot (`HardwareSnapshot`) is kept separate from the derived
//! assessment (`InferenceSupport`) so fixtures can drive the assessment in
//! tests and in explicit development simulation.

use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};
use ts_rs::TS;

use super::memory_budget::MemoryPressure;
use super::permissions::PermissionState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum OsKind {
    Macos,
    Windows,
    /// Development builds only (Linux CI). Never an official target.
    Other,
}

impl OsKind {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            OsKind::Macos
        } else if cfg!(target_os = "windows") {
            OsKind::Windows
        } else {
            OsKind::Other
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CpuInfo {
    pub model: Option<String>,
    pub physical_cores: Option<u32>,
    pub logical_cores: u32,
    /// x86-64 only; `None` on ARM.
    pub avx2: Option<bool>,
    pub avx512: Option<bool>,
    /// Apple Silicon generation (1 for M1, 2 for M2, ...), parsed from the
    /// brand string.
    pub apple_silicon_generation: Option<u8>,
    /// "Pro", "Max", "Ultra" or `None` for the base chip.
    pub apple_silicon_variant: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub pressure: Option<MemoryPressure>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiskInfo {
    /// Free space on the volume holding application data and models.
    pub available_bytes: u64,
    pub total_bytes: u64,
    pub mount_point: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GpuInfo {
    pub vendor: Option<String>,
    pub model: Option<String>,
    /// Dedicated VRAM for discrete GPUs. For unified memory this is the Metal
    /// recommended working set instead (see `unified_memory`).
    pub vram_bytes: Option<u64>,
    pub unified_memory: bool,
    pub integrated: bool,
    pub metal: bool,
    pub cuda: bool,
    pub vulkan: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioDeviceInfo {
    /// Stable cpal device id (`host:id`), safe to persist.
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub channels: Option<u16>,
    pub sample_rate: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioCapabilities {
    pub microphone_available: bool,
    pub microphone_permission: PermissionState,
    pub system_audio_available: bool,
    pub system_audio_permission: PermissionState,
    /// Why system audio is not available, in plain language.
    pub system_audio_unavailable_reason: Option<String>,
    pub input_devices: Vec<AudioDeviceInfo>,
    pub output_device: Option<AudioDeviceInfo>,
}

/// Raw, measured facts about the machine. Serializable so fixtures can stand
/// in for real hardware in tests.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HardwareSnapshot {
    pub os: Option<OsKind>,
    pub os_version: String,
    /// Parsed (major, minor, patch) for requirement checks.
    pub os_version_parts: Option<(u32, u32, u32)>,
    /// Windows build number (e.g. 19045 for 10 22H2).
    pub os_build: Option<u32>,
    pub architecture: String,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub disk: DiskInfo,
    pub gpus: Vec<GpuInfo>,
    pub audio: AudioCapabilities,
}

impl HardwareSnapshot {
    pub fn os(&self) -> OsKind {
        self.os.unwrap_or(OsKind::Other)
    }

    pub fn is_apple_silicon(&self) -> bool {
        self.os() == OsKind::Macos && (self.architecture == "aarch64" || self.architecture == "arm64")
    }

    /// Metal working-set limit of the unified-memory GPU, if any.
    pub fn unified_gpu_working_set(&self) -> Option<u64> {
        self.gpus
            .iter()
            .filter(|g| g.unified_memory && g.metal)
            .filter_map(|g| g.vram_bytes)
            .max()
    }

    /// Largest discrete GPU that llama.cpp can offload to on this platform.
    pub fn usable_discrete_vram(&self) -> Option<u64> {
        self.gpus
            .iter()
            .filter(|g| !g.unified_memory && !g.integrated && (g.cuda || g.vulkan))
            .filter_map(|g| g.vram_bytes)
            .max()
    }

    pub fn has_gpu_acceleration(&self) -> bool {
        (self.is_apple_silicon() && self.gpus.iter().any(|g| g.metal))
            || self.usable_discrete_vram().is_some_and(|v| v >= 4 * super::memory_budget::GIB)
    }
}

/// Parse "14.6.1" / "10.0.19045" into parts.
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().split(|c: char| c == '.' || c.is_whitespace()).map(|p| p.parse::<u32>());
    let major = it.next()?.ok()?;
    let minor = it.next().and_then(Result::ok).unwrap_or(0);
    let patch = it.next().and_then(Result::ok).unwrap_or(0);
    Some((major, minor, patch))
}

/// Parse "Apple M2 Pro" → (2, Some("Pro")).
pub fn parse_apple_chip(brand: &str) -> Option<(u8, Option<String>)> {
    let rest = brand.trim().strip_prefix("Apple M")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let generation = digits.parse::<u8>().ok()?;
    let variant = rest[digits.len()..].trim();
    let variant = (!variant.is_empty()).then(|| variant.to_string());
    Some((generation, variant))
}

/// Collect the baseline snapshot. Audio and platform GPU details are filled in
/// by their own modules; this function never fails; unknown values stay
/// `None`.
pub fn detect_baseline(data_dir: &std::path::Path) -> HardwareSnapshot {
    let sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_memory(MemoryRefreshKind::nothing().with_ram())
            .with_cpu(CpuRefreshKind::nothing()),
    );

    let os = OsKind::current();
    let os_version = System::long_os_version().unwrap_or_else(|| "Unknown".into());
    let raw_version = System::os_version().unwrap_or_default();
    let os_version_parts = parse_version(&raw_version);
    let os_build = if os == OsKind::Windows {
        System::kernel_version().and_then(|k| k.split('.').nth(2).and_then(|b| b.parse().ok()))
            .or_else(|| raw_version.split(|c: char| !c.is_ascii_digit()).filter(|s| s.len() >= 5).find_map(|s| s.parse().ok()))
    } else {
        None
    };

    let brand = sys.cpus().first().map(|c| c.brand().trim().to_string()).filter(|b| !b.is_empty());
    let brand = brand.or_else(super::platform::cpu_brand);
    let (gen, variant) = brand
        .as_deref()
        .and_then(parse_apple_chip)
        .map(|(g, v)| (Some(g), v))
        .unwrap_or((None, None));

    let cpu = CpuInfo {
        model: brand,
        physical_cores: System::physical_core_count().map(|c| c as u32),
        logical_cores: sys.cpus().len().max(1) as u32,
        avx2: x86_feature("avx2"),
        avx512: x86_feature("avx512f"),
        apple_silicon_generation: gen,
        apple_silicon_variant: variant,
    };

    let memory = MemoryInfo {
        total_bytes: sys.total_memory(),
        available_bytes: sys.available_memory(),
        pressure: super::platform::memory_pressure(),
    };

    HardwareSnapshot {
        os: Some(os),
        os_version,
        os_version_parts,
        os_build,
        architecture: std::env::consts::ARCH.to_string(),
        cpu,
        memory,
        disk: detect_disk(data_dir),
        gpus: super::platform::detect_gpus(),
        audio: AudioCapabilities::default(),
    }
}

#[allow(unused_variables)]
fn x86_feature(name: &str) -> Option<bool> {
    #[cfg(target_arch = "x86_64")]
    {
        return Some(match name {
            "avx2" => std::is_x86_feature_detected!("avx2"),
            "avx512f" => std::is_x86_feature_detected!("avx512f"),
            _ => false,
        });
    }
    #[allow(unreachable_code)]
    None
}

/// Free space on the volume that contains `path` (the longest matching mount
/// point wins, which also de-duplicates `/` vs `/System/Volumes/Data`).
pub fn detect_disk(path: &std::path::Path) -> DiskInfo {
    let disks = Disks::new_with_refreshed_list();
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    disks
        .list()
        .iter()
        .filter(|d| target.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| DiskInfo {
            available_bytes: d.available_space(),
            total_bytes: d.total_space(),
            mount_point: Some(d.mount_point().to_string_lossy().into_owned()),
        })
        .unwrap_or_default()
}

/// Refresh only the fast-changing values (memory, pressure, disk). Used for
/// the lightweight check on subsequent launches and before loading models.
pub fn refresh_dynamic(snapshot: &mut HardwareSnapshot, data_dir: &std::path::Path) {
    let mut sys = System::new();
    sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    snapshot.memory.total_bytes = sys.total_memory();
    snapshot.memory.available_bytes = sys.available_memory();
    snapshot.memory.pressure = super::platform::memory_pressure();
    snapshot.disk = detect_disk(data_dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("14.6.1"), Some((14, 6, 1)));
        assert_eq!(parse_version("15"), Some((15, 0, 0)));
        assert_eq!(parse_version("10.0.19045"), Some((10, 0, 19045)));
        assert_eq!(parse_version("garbage"), None);
    }

    #[test]
    fn parses_apple_chips() {
        assert_eq!(parse_apple_chip("Apple M1"), Some((1, None)));
        assert_eq!(parse_apple_chip("Apple M2 Pro"), Some((2, Some("Pro".into()))));
        assert_eq!(parse_apple_chip("Apple M4 Max"), Some((4, Some("Max".into()))));
        assert_eq!(parse_apple_chip("Intel(R) Core(TM) i7"), None);
    }

    #[test]
    fn baseline_detection_returns_sane_values() {
        let dir = std::env::temp_dir();
        let snap = detect_baseline(&dir);
        assert!(snap.memory.total_bytes > 0);
        assert!(snap.cpu.logical_cores >= 1);
        assert!(snap.memory.available_bytes <= snap.memory.total_bytes);
        assert!(snap.disk.total_bytes > 0, "disk for temp dir should be found");
    }
}
