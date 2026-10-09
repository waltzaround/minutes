//! Local media decoding using the executable shipped with Minutes.
use std::path::{Path, PathBuf};
use crate::error::{AppError, AppResult};

pub fn runtime_binary(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" })
}

pub async fn extract_audio(runtime_dir: &Path, input: &Path, output: &Path) -> AppResult<()> {
    let mut command = tokio::process::Command::new(runtime_binary(runtime_dir));
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let result = command
        .args(["-nostdin", "-v", "error", "-y", "-i"]).arg(input)
        .args(["-map", "0:a:0", "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(output).kill_on_drop(true).output().await
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound {
            AppError::user("decoder_missing", "The included media decoder is missing. Reinstall Minutes, then try importing again.")
        } else { AppError::Internal(e.into()) })?;
    if !result.status.success() {
        tracing::warn!(error = %String::from_utf8_lossy(&result.stderr), "media decode failed");
        return Err(AppError::user("decode_failed", "This file could not be read. It may be damaged, unsupported, or have no audio track."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_bundled_decoder_never_uses_system_ffmpeg() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("input.wav");
        std::fs::write(&input, b"input").unwrap();
        let err = extract_audio(&root.path().join("missing"), &input, &root.path().join("output.wav"))
            .await.unwrap_err();
        assert!(matches!(err, AppError::User { code: "decoder_missing", .. }));
    }

    #[tokio::test]
    async fn bundled_decoder_imports_audio_and_first_video_track_without_path() {
        let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/ffmpeg");
        assert!(runtime_binary(&runtime).is_file(), "Prepare the bundled decoder with node scripts/build-ffmpeg.mjs before running tests");
        let root = tempfile::tempdir().unwrap();
        // Put spaces in every path, as in real file-picker imports.
        let runtime_copy = root.path().join("bundled decoder");
        std::fs::create_dir(&runtime_copy).unwrap();
        std::fs::copy(runtime_binary(&runtime), runtime_binary(&runtime_copy)).unwrap();
        let video = root.path().join("two audio tracks.mp4");
        let result = tokio::process::Command::new(runtime_binary(&runtime_copy))
            .env("PATH", "")
            .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=160x120:d=1",
                "-f", "lavfi", "-i", "sine=frequency=440:duration=1",
                "-f", "lavfi", "-i", "anullsrc=r=16000:cl=mono",
                "-map", "0:v", "-map", "1:a", "-map", "2:a", "-c:v", "mpeg4", "-c:a", "aac", "-t", "1"])
            .arg(&video).output().await.unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let original = std::fs::read(&video).unwrap();
        let output = root.path().join("decoded audio.wav");
        extract_audio(&runtime_copy, &video, &output).await.unwrap();
        let mut reader = hound::WavReader::open(&output).unwrap();
        let spec = reader.spec();
        assert_eq!((spec.sample_rate, spec.channels, spec.bits_per_sample), (16000, 1, 16));
        assert!(reader.duration() >= 16000);
        assert!(reader.samples::<i16>().any(|sample| sample.unwrap().abs() > 100), "must select the first audio track, not the silent second track");
        assert_eq!(std::fs::read(&video).unwrap(), original);

        for extension in ["m4a", "wav"] {
            let audio = root.path().join(format!("saved recording.{extension}"));
            let result = tokio::process::Command::new(runtime_binary(&runtime_copy))
                .env("PATH", "").args(["-nostdin", "-v", "error", "-i"]).arg(&video)
                .args(["-map", "0:a:0", "-vn", "-f", if extension == "m4a" { "mp4" } else { "wav" }])
                .arg(&audio).output().await.unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
            let original = std::fs::read(&audio).unwrap();
            extract_audio(&runtime_copy, &audio, &output).await.unwrap();
            assert!(crate::audio::wav::duration_ms(&output).unwrap() >= 1000);
            assert_eq!(std::fs::read(&audio).unwrap(), original);
        }
        // A one-second, synthetic 440 Hz MP3 fixture: the decoder intentionally
        // ships no external MP3 encoder library.
        let mp3 = root.path().join("saved recording.mp3");
        std::fs::write(&mp3, include_bytes!("../../fixtures/audio/import.mp3")).unwrap();
        extract_audio(&runtime_copy, &mp3, &output).await.unwrap();
        assert!(crate::audio::wav::duration_ms(&output).unwrap() >= 1000);
        let bad = root.path().join("broken.mp4");
        std::fs::write(&bad, b"not media").unwrap();
        assert!(matches!(extract_audio(&runtime_copy, &bad, &output).await.unwrap_err(), AppError::User { code: "decode_failed", .. }));
        let silent = root.path().join("no audio.mp4");
        let result = tokio::process::Command::new(runtime_binary(&runtime_copy))
            .env("PATH", "").args(["-nostdin", "-v", "error", "-f", "lavfi", "-i",
                "color=c=black:s=160x120:d=1", "-an", "-c:v", "mpeg4"])
            .arg(&silent).output().await.unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        assert!(matches!(extract_audio(&runtime_copy, &silent, &output).await.unwrap_err(), AppError::User { code: "decode_failed", .. }));
    }
}
