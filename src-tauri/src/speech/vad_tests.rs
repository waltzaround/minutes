//! Tests against the real Silero model (ignored unless models are installed).

use super::vad::{SpeechDetector, VadConfig};

fn models() -> Option<std::path::PathBuf> {
    std::env::var("MINUTES_MODELS_DIR").ok().map(std::path::PathBuf::from)
}

#[test]
#[ignore]
fn vad_finds_speech_and_keeps_absolute_offsets() {
    let dir = models().expect("MINUTES_MODELS_DIR");
    let audio = crate::audio::wav::read_16k_mono(&dir.join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav")).unwrap();
    let mut vad = SpeechDetector::new(&dir.join("silero-vad/silero_vad.onnx"), &VadConfig::default()).unwrap();
    // 2 s silence, speech, 1 s silence, fed in irregular blocks starting at 5 s.
    let mut stream = vec![0.0f32; 32_000];
    stream.extend_from_slice(&audio);
    stream.extend(std::iter::repeat_n(0.0, 16_000));
    let base = 80_000u64;
    let mut spans = Vec::new();
    let mut pos = 0usize;
    for size in [3000usize, 7000, 1234, 8000].iter().cycle() {
        if pos >= stream.len() {
            break;
        }
        let end = (pos + size).min(stream.len());
        spans.extend(vad.accept(base + pos as u64, &stream[pos..end]));
        pos = end;
    }
    spans.extend(vad.flush());
    assert!(!spans.is_empty());
    let first = spans[0].start_sample;
    assert!(first >= base + 30_000 && first <= base + 36_000, "speech starts at {first}");
}
