//! Real-model checks on a two-speaker recording (not committed; point
//! MINUTES_DIARIZATION_WAV at a file and MINUTES_MODELS_DIR at the models).

use super::diarization::{Diarizer, NemoDiarizer};
use crate::storage::settings::InferenceBackendPreference;

fn diarizer(models: &std::path::Path) -> NemoDiarizer {
    NemoDiarizer::new(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/nemo-speech"),
        &models.join("nemotron-3-diarization/Nemotron-3-Diarization.q8_0.gguf"),
        InferenceBackendPreference::Auto,
        &std::env::temp_dir().join("minutes-diarization-test"),
    )
    .unwrap()
}

#[test]
#[ignore]
fn nemotron_finds_two_speakers() {
    let wav = std::env::var("MINUTES_DIARIZATION_WAV").unwrap();
    let models = std::path::PathBuf::from(std::env::var("MINUTES_MODELS_DIR").unwrap());
    let audio = crate::audio::wav::read_16k_mono(std::path::Path::new(&wav)).unwrap();
    let t = std::time::Instant::now();
    let turns = diarizer(&models).diarize(&audio).unwrap();
    let n = turns.iter().map(|t| t.speaker).max().map(|m| m + 1).unwrap_or(0);
    println!("{n} speakers, {} turns, {:?}", turns.len(), t.elapsed());
    assert_eq!(n, 2);
}

/// Same-speaker vs different-speaker similarity of per-turn embeddings,
/// to place the known/possible thresholds.
#[test]
#[ignore]
fn embedding_similarity_ranges() {
    use super::embeddings::{cosine, EmbeddingModel, SherpaEmbedder};
    let wav = std::env::var("MINUTES_DIARIZATION_WAV").unwrap();
    let models = std::path::PathBuf::from(std::env::var("MINUTES_MODELS_DIR").unwrap());
    let audio = crate::audio::wav::read_16k_mono(std::path::Path::new(&wav)).unwrap();
    let turns = diarizer(&models).diarize(&audio).unwrap();
    let e = SherpaEmbedder::load(&models.join("speaker-embedding/nemo_en_titanet_small.onnx"), 2).unwrap();
    let mut embs = Vec::new();
    for t in &turns {
        let s = (t.start_ms * 16) as usize;
        let en = ((t.end_ms * 16) as usize).min(audio.len());
        if en - s >= 24_000 {
            embs.push((t.speaker, e.embed(&audio[s..en]).unwrap()));
        }
    }
    let (mut same, mut diff) = (Vec::new(), Vec::new());
    for i in 0..embs.len() {
        for j in i + 1..embs.len() {
            let c = cosine(&embs[i].1, &embs[j].1);
            if embs[i].0 == embs[j].0 { same.push(c) } else { diff.push(c) }
        }
    }
    println!("same speaker: {same:?}\ndifferent: {diff:?}");
}
