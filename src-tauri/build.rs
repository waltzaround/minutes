fn main() {
    // Refuse release builds without the decoder for the actual compilation
    // target. Resource directories alone do not guarantee a working import.
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        let target = std::env::var("TARGET").unwrap();
        let (binary, platform) = match target.as_str() {
            "aarch64-apple-darwin" => ("ffmpeg", "macos-arm64"),
            "x86_64-pc-windows-msvc" => ("ffmpeg.exe", "win-x64"),
            _ => panic!("No bundled FFmpeg configured for {target}"),
        };
        let dir = std::path::Path::new("binaries/ffmpeg");
        for file in [binary, "LICENSE", "LICENSE.upstream", "README.upstream", "NOTICE", "VERSION", "ffmpeg-8.0.1-source.tar.xz", "build-ffmpeg.mjs"] {
            println!("cargo:rerun-if-changed=binaries/ffmpeg/{file}");
            assert!(dir.join(file).is_file(), "Missing bundled decoder file {file}; run node scripts/build-ffmpeg.mjs {platform} from the project root");
        }
        let version = std::fs::read_to_string(dir.join("VERSION")).unwrap();
        assert_eq!(version.trim(), format!("8.0.1 {platform}"), "Bundled FFmpeg target mismatch; run node scripts/build-ffmpeg.mjs {platform}");
    }
    tauri_build::build()
}
