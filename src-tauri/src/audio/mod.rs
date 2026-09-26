//! Audio capture layer (CPAL). Microphone and system/meeting audio are
//! captured as two separate logical streams and never pre-mixed.

pub mod capture;
pub mod devices;
pub mod resample;
pub mod wav;
