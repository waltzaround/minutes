//! Windows probes: DXGI, GlobalMemoryStatusEx, LoadLibrary. No shell commands.

use windows::core::w;
use windows::Win32::Foundation::FreeLibrary;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use crate::system::capabilities::GpuInfo;
use crate::system::memory_budget::{MemoryPressure, GIB};
use crate::system::permissions::PermissionState;

const VENDOR_NVIDIA: u32 = 0x10DE;
const VENDOR_AMD: u32 = 0x1002;
const VENDOR_INTEL: u32 = 0x8086;

pub fn cpu_brand() -> Option<String> {
    None // sysinfo reports the brand reliably on Windows.
}

/// Map the OS memory load percentage to a pressure level.
pub fn memory_pressure() -> Option<MemoryPressure> {
    let mut status = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: dwLength is initialised as the API requires.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(match status.dwMemoryLoad {
        0..=74 => MemoryPressure::Low,
        75..=89 => MemoryPressure::Medium,
        _ => MemoryPressure::High,
    })
}

fn dll_loads(name: windows::core::PCWSTR) -> bool {
    // SAFETY: loading a system DLL by name; freed immediately.
    match unsafe { LoadLibraryW(name) } {
        Ok(h) => {
            let _ = unsafe { FreeLibrary(h) };
            true
        }
        Err(_) => false,
    }
}

fn vulkan_available() -> bool {
    // SAFETY: loads vulkan-1.dll via ash; no instance is created.
    match unsafe { ash::Entry::load() } {
        Ok(entry) => unsafe { entry.try_enumerate_instance_version() }.is_ok(),
        Err(_) => false,
    }
}

pub fn detect_gpus() -> Vec<GpuInfo> {
    let cuda_driver = dll_loads(w!("nvcuda.dll"));
    let vulkan = vulkan_available();
    let mut gpus = Vec::new();
    // SAFETY: standard DXGI factory creation.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return gpus;
    };
    let mut index = 0;
    // SAFETY: EnumAdapters1 returns DXGI_ERROR_NOT_FOUND past the last adapter.
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(index) } {
        index += 1;
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let model = String::from_utf16_lossy(&desc.Description[..len]);
        let dedicated = desc.DedicatedVideoMemory as u64;
        let vendor = match desc.VendorId {
            VENDOR_NVIDIA => "NVIDIA",
            VENDOR_AMD => "AMD",
            VENDOR_INTEL => "Intel",
            _ => "Other",
        };
        // Integrated GPUs report little or no dedicated memory.
        let integrated = dedicated < 2 * GIB;
        if gpus.iter().any(|g: &GpuInfo| g.model.as_deref() == Some(model.as_str())) {
            continue;
        }
        gpus.push(GpuInfo {
            vendor: Some(vendor.into()),
            model: Some(model),
            vram_bytes: (dedicated > 0).then_some(dedicated),
            unified_memory: false,
            integrated,
            metal: false,
            cuda: cuda_driver && desc.VendorId == VENDOR_NVIDIA,
            vulkan,
        });
    }
    gpus
}

/// Windows only gates microphone access through the privacy settings for
/// desktop apps; there is no prompt. Whether access works is verified when
/// the device is opened (see `audio::devices`).
pub fn microphone_permission() -> PermissionState {
    PermissionState::Unknown
}

pub async fn request_microphone_permission() -> PermissionState {
    PermissionState::Unknown
}

/// WASAPI loopback needs no permission.
pub fn system_audio_permission() -> PermissionState {
    PermissionState::NotRequired
}

pub fn privacy_settings_url(kind: &str) -> Option<&'static str> {
    match kind {
        "microphone" => Some("ms-settings:privacy-microphone"),
        _ => None,
    }
}
