//! Managed `llama-server` sidecar.
//!
//! - Binds only to 127.0.0.1 on an ephemeral port chosen by us.
//! - Protected by a random per-process API key (other local processes can't
//!   use it).
//! - Started only when needed, health-checked, stopped cleanly; a PID file
//!   lets the next launch terminate an orphan left by a crash.
//! - Requests have timeouts; raw subprocess output goes to a log file, never
//!   to the user.

use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::json;

use super::{JsonRequest, JsonResponse, LlmError, LocalLlm};
use crate::storage::settings::InferenceBackendPreference;

#[derive(Debug, Clone)]
pub struct LlamaConfig {
    /// Directory containing `llama-server` and its libraries.
    pub runtime_dir: PathBuf,
    pub model_id: String,
    pub model_path: PathBuf,
    pub context_tokens: u32,
    /// `None` = let llama.cpp fit layers to device memory.
    pub gpu_layers: Option<i32>,
    pub backend: InferenceBackendPreference,
    pub threads: Option<u32>,
    pub log_path: PathBuf,
    pub pid_path: PathBuf,
    pub load_timeout: Duration,
    pub request_timeout: Duration,
}

pub fn server_binary(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(if cfg!(windows) { "llama-server.exe" } else { "llama-server" })
}

pub struct LlamaServer {
    child: Mutex<Child>,
    base: String,
    api_key: String,
    client: reqwest::blocking::Client,
    cfg: LlamaConfig,
}

fn free_port() -> std::io::Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

fn random_key() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// Kill a `llama-server` left behind by a previous crash.
pub fn kill_orphan(pid_path: &Path) {
    let Ok(raw) = std::fs::read_to_string(pid_path) else { return };
    let _ = std::fs::remove_file(pid_path);
    let Ok(pid) = raw.trim().parse::<u32>() else { return };
    let mut sys = sysinfo::System::new();
    let spid = sysinfo::Pid::from_u32(pid);
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[spid]), true);
    if let Some(p) = sys.process(spid) {
        if p.name().to_string_lossy().contains("llama-server") {
            tracing::warn!(pid, "terminating orphaned llama-server");
            p.kill();
        }
    }
}

fn log_tail(path: &Path, max: usize) -> String {
    let mut s = String::new();
    if let Ok(mut f) = std::fs::File::open(path) {
        let _ = f.read_to_string(&mut s);
    }
    let start = s.len().saturating_sub(max);
    s[s.floor_char_boundary(start)..].to_string()
}

impl LlamaServer {
    pub fn start(cfg: LlamaConfig) -> Result<Self, LlmError> {
        let bin = server_binary(&cfg.runtime_dir);
        if !bin.exists() {
            return Err(LlmError::RuntimeMissing);
        }
        kill_orphan(&cfg.pid_path);
        let port = free_port().map_err(|e| LlmError::StartFailed(e.to_string()))?;
        let api_key = random_key();
        if let Some(parent) = cfg.log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let log = std::fs::File::create(&cfg.log_path).map_err(|e| LlmError::StartFailed(e.to_string()))?;
        let log_err = log.try_clone().map_err(|e| LlmError::StartFailed(e.to_string()))?;

        let mut cmd = Command::new(&bin);
        cmd.current_dir(&cfg.runtime_dir)
            .arg("-m")
            .arg(&cfg.model_path)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["-c", &cfg.context_tokens.to_string()])
            .args(["-np", "1", "--no-webui", "--reasoning", "off", "--cache-ram", "0"])
            .args(["--api-key", &api_key])
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        let layers = match (cfg.backend, cfg.gpu_layers) {
            (InferenceBackendPreference::Cpu, _) => Some(0),
            (_, l) => l,
        };
        match layers {
            Some(n) => cmd.args(["-ngl", &n.to_string()]),
            None => cmd.args(["-ngl", "auto"]),
        };
        if let Some(t) = cfg.threads {
            cmd.args(["-t", &t.to_string()]);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().map_err(|e| LlmError::StartFailed(e.to_string()))?;
        let _ = std::fs::write(&cfg.pid_path, child.id().to_string());
        tracing::info!(model = cfg.model_id, port, context = cfg.context_tokens, "llama-server starting");

        let client = reqwest::blocking::Client::builder()
            .timeout(cfg.request_timeout)
            .connect_timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
            .map_err(|e| LlmError::StartFailed(e.to_string()))?;
        let server = LlamaServer { child: Mutex::new(child), base: format!("http://127.0.0.1:{port}"), api_key, client, cfg };
        server.wait_healthy()?;
        Ok(server)
    }

    fn alive(&self) -> bool {
        matches!(self.child.lock().try_wait(), Ok(None))
    }

    fn wait_healthy(&self) -> Result<(), LlmError> {
        let started = Instant::now();
        loop {
            if !self.alive() {
                let tail = log_tail(&self.cfg.log_path, 2000);
                tracing::error!(log = tail, "llama-server exited during startup");
                return Err(LlmError::StartFailed("the model could not be loaded (see log for details)".into()));
            }
            match self.client.get(format!("{}/health", self.base)).timeout(Duration::from_secs(2)).send() {
                Ok(r) if r.status().is_success() => {
                    tracing::info!(ms = started.elapsed().as_millis() as u64, "llama-server ready");
                    return Ok(());
                }
                _ => {}
            }
            if started.elapsed() > self.cfg.load_timeout {
                self.stop();
                return Err(LlmError::Timeout);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    pub fn stop(&self) {
        let mut child = self.child.lock();
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!(model = self.cfg.model_id, "llama-server stopped");
        }
        let _ = std::fs::remove_file(&self.cfg.pid_path);
    }

    pub fn config(&self) -> &LlamaConfig {
        &self.cfg
    }
}

impl Drop for LlamaServer {
    fn drop(&mut self) {
        self.stop();
    }
}

impl LocalLlm for LlamaServer {
    fn model_id(&self) -> &str {
        &self.cfg.model_id
    }

    fn context_tokens(&self) -> u32 {
        self.cfg.context_tokens
    }

    fn complete_json(&self, req: &JsonRequest) -> Result<JsonResponse, LlmError> {
        if !self.alive() {
            return Err(LlmError::Crashed);
        }
        let body = json!({
            "messages": [
                { "role": "system", "content": req.system },
                { "role": "user", "content": req.user }
            ],
            "response_format": { "type": "json_schema", "json_schema": { "name": "result", "schema": req.schema } },
            "chat_template_kwargs": { "enable_thinking": false },
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
            "stream": false
        });
        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.base))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::Timeout
                } else if !self.alive() {
                    LlmError::Crashed
                } else {
                    LlmError::Request(e.to_string())
                }
            })?;
        let status = resp.status();
        let v: serde_json::Value = resp.json().map_err(|e| LlmError::Request(e.to_string()))?;
        if !status.is_success() {
            let msg = v["error"]["message"].as_str().unwrap_or("request failed").to_string();
            return Err(LlmError::Request(msg));
        }
        let choice = &v["choices"][0];
        let t = &v["timings"];
        Ok(JsonResponse {
            content: choice["message"]["content"].as_str().unwrap_or_default().to_string(),
            prompt_tokens: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            completion_tokens: v["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
            prompt_tokens_per_second: t["prompt_per_second"].as_f64().unwrap_or(0.0),
            generation_tokens_per_second: t["predicted_per_second"].as_f64().unwrap_or(0.0),
            truncated: choice["finish_reason"].as_str() == Some("length"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_ephemeral_and_local() {
        let p = free_port().unwrap();
        assert!(p > 1024);
        assert!(TcpListener::bind(("127.0.0.1", p)).is_ok());
    }

    #[test]
    fn keys_are_random() {
        assert_ne!(random_key(), random_key());
        assert_eq!(random_key().len(), 64);
    }

    #[test]
    fn missing_runtime_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = LlamaConfig {
            runtime_dir: dir.path().to_path_buf(),
            model_id: "m".into(),
            model_path: dir.path().join("m.gguf"),
            context_tokens: 4096,
            gpu_layers: None,
            backend: InferenceBackendPreference::Auto,
            threads: None,
            log_path: dir.path().join("l.log"),
            pid_path: dir.path().join("p.pid"),
            load_timeout: Duration::from_secs(1),
            request_timeout: Duration::from_secs(1),
        };
        assert!(matches!(LlamaServer::start(cfg), Err(LlmError::RuntimeMissing)));
    }

    #[test]
    fn orphan_pid_file_for_unrelated_process_is_harmless() {
        let dir = tempfile::tempdir().unwrap();
        let pid = dir.path().join("x.pid");
        std::fs::write(&pid, std::process::id().to_string()).unwrap();
        kill_orphan(&pid); // our own process is not llama-server: must not be killed
        assert!(!pid.exists());
    }
}
