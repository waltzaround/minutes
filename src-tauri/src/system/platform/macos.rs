//! macOS probes: sysctl, Metal and AVFoundation. No shell commands.

use std::ffi::CString;

use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_metal::{MTLCreateSystemDefaultDevice, MTLDevice};

use crate::system::capabilities::GpuInfo;
use crate::system::memory_budget::MemoryPressure;
use crate::system::permissions::PermissionState;

// MTLCreateSystemDefaultDevice needs CoreGraphics linked (objc2-metal docs).
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {}

fn sysctl_string(name: &str) -> Option<String> {
    let cname = CString::new(name).ok()?;
    let mut len: libc::size_t = 0;
    // SAFETY: first call obtains the required buffer length.
    let rc = unsafe { libc::sysctlbyname(cname.as_ptr(), std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) };
    if rc != 0 || len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: buffer is `len` bytes long as reported by the kernel.
    let rc = unsafe {
        libc::sysctlbyname(cname.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0)
    };
    if rc != 0 {
        return None;
    }
    buf.truncate(len);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    String::from_utf8(buf).ok()
}

fn sysctl_i32(name: &str) -> Option<i32> {
    let cname = CString::new(name).ok()?;
    let mut value: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    // SAFETY: value is an i32 and len matches its size.
    let rc = unsafe {
        libc::sysctlbyname(cname.as_ptr(), (&mut value as *mut i32).cast(), &mut len, std::ptr::null_mut(), 0)
    };
    (rc == 0).then_some(value)
}

pub fn cpu_brand() -> Option<String> {
    sysctl_string("machdep.cpu.brand_string")
}

/// Kernel VM pressure level: 1 = normal, 2 = warn, 4 = critical.
pub fn memory_pressure() -> Option<MemoryPressure> {
    match sysctl_i32("kern.memorystatus_vm_pressure_level")? {
        1 => Some(MemoryPressure::Low),
        2 => Some(MemoryPressure::Medium),
        4 => Some(MemoryPressure::High),
        _ => None,
    }
}

pub fn detect_gpus() -> Vec<GpuInfo> {
    let Some(device) = MTLCreateSystemDefaultDevice() else {
        return Vec::new();
    };
    let unified = device.hasUnifiedMemory();
    let name = device.name().to_string();
    vec![GpuInfo {
        vendor: Some(if name.starts_with("Apple") { "Apple".into() } else { "Unknown".into() }),
        model: Some(name),
        vram_bytes: Some(device.recommendedMaxWorkingSetSize()),
        unified_memory: unified,
        integrated: unified,
        metal: true,
        cuda: false,
        vulkan: false,
    }]
}

pub fn microphone_permission() -> PermissionState {
    // SAFETY: AVMediaTypeAudio is a static NSString provided by AVFoundation.
    let Some(media) = (unsafe { AVMediaTypeAudio }) else {
        return PermissionState::Unknown;
    };
    // SAFETY: plain class method call with a valid media type.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) };
    match status {
        AVAuthorizationStatus::Authorized => PermissionState::Granted,
        AVAuthorizationStatus::Denied => PermissionState::Denied,
        AVAuthorizationStatus::Restricted => PermissionState::Restricted,
        AVAuthorizationStatus::NotDetermined => PermissionState::NotDetermined,
        _ => PermissionState::Unknown,
    }
}

/// Show the system microphone prompt (only shown once by macOS; afterwards
/// the user must change it in System Settings). Resolves with the result.
pub async fn request_microphone_permission() -> PermissionState {
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    // The block is not `Send`, so create and hand it over synchronously
    // before awaiting.
    if !start_microphone_request(tx) {
        return PermissionState::Unknown;
    }
    match rx.await {
        Ok(true) => PermissionState::Granted,
        Ok(false) => PermissionState::Denied,
        Err(_) => microphone_permission(),
    }
}

fn start_microphone_request(tx: tokio::sync::oneshot::Sender<bool>) -> bool {
    let tx = std::sync::Mutex::new(Some(tx));
    let block = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
        if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
            let _ = tx.send(granted.as_bool());
        }
    });
    // SAFETY: see microphone_permission.
    let Some(media) = (unsafe { AVMediaTypeAudio }) else {
        return false;
    };
    // SAFETY: AVFoundation copies/retains the block until it is invoked.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &block) };
    true
}

/// macOS exposes no API to query "System Audio Recording" (process tap)
/// permission. We report `Unknown` until a capture test observes real
/// (non-zero) audio, see `audio::capture::probe_system_audio`.
pub fn system_audio_permission() -> PermissionState {
    PermissionState::Unknown
}

/// Deep link to the relevant System Settings privacy pane.
pub fn privacy_settings_url(kind: &str) -> Option<&'static str> {
    match kind {
        "microphone" => Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"),
        "systemAudio" => Some("x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture"),
        _ => None,
    }
}
