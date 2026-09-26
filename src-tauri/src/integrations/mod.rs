//! Optional integrations. Nothing is sent anywhere until the user connects
//! an integration and explicitly asks to send something. Credentials live in
//! the OS credential store.

pub mod linear;
pub mod notion;

use std::time::Duration;

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, thiserror::Error)]
pub enum IntegrationError {
    #[error("{0} is not connected.")]
    NotConnected(&'static str),
    #[error("{service} rejected the credentials. Reconnect it in Settings.")]
    Unauthorized { service: &'static str },
    #[error("{service} is busy (rate limited). Try again in a minute.")]
    RateLimited { service: &'static str },
    #[error("Could not reach {service}. Check your internet connection. ({detail})")]
    Network { service: &'static str, detail: String },
    #[error("{service} returned an error: {message}")]
    Api { service: &'static str, message: String },
    #[error("{0}")]
    Invalid(String),
}

impl IntegrationError {
    /// True when the request may or may not have been applied remotely.
    pub fn outcome_unknown(&self) -> bool {
        matches!(self, IntegrationError::Network { .. })
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConnectionStatus {
    pub connected: bool,
    /// Workspace / bot name, for display.
    pub account: Option<String>,
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Minutes/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
        .expect("http client")
}

/// Parse a `Retry-After` header (seconds), capped.
pub fn retry_after(resp: &reqwest::Response) -> Duration {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|s| Duration::from_secs(s.min(60)))
        .unwrap_or(Duration::from_secs(2))
}

/// Split text into chunks of at most `max` characters, preferring
/// whitespace boundaries.
pub fn split_text(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while rest.chars().count() > max {
        let byte_max = rest.char_indices().nth(max).map(|(i, _)| i).unwrap_or(rest.len());
        let cut = rest[..byte_max].rfind(char::is_whitespace).filter(|&i| i > byte_max / 2).unwrap_or(byte_max);
        out.push(rest[..cut].to_string());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() || out.is_empty() {
        out.push(rest.to_string());
    }
    out
}

#[cfg(test)]
pub mod test_server {
    //! Tiny scripted HTTP server for integration tests (no external mocks).
    use std::sync::Arc;

    use parking_lot::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Debug, Clone)]
    pub struct Request {
        pub method: String,
        pub path: String,
        pub headers: Vec<(String, String)>,
        pub body: String,
    }

    pub type Handler = Arc<dyn Fn(&Request) -> (u16, Vec<(String, String)>, String) + Send + Sync>;

    pub async fn serve(handler: Handler) -> (String, Arc<Mutex<Vec<Request>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let handler = handler.clone();
                let log = log2.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 8192];
                    let (head, body_start) = loop {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break (String::from_utf8_lossy(&buf[..i]).to_string(), i + 4);
                        }
                    };
                    let mut lines = head.lines();
                    let first = lines.next().unwrap_or_default().to_string();
                    let mut parts = first.split_whitespace();
                    let method = parts.next().unwrap_or_default().to_string();
                    let path = parts.next().unwrap_or_default().to_string();
                    let headers: Vec<(String, String)> = lines
                        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())))
                        .collect();
                    let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
                    let mut body = buf[body_start..].to_vec();
                    while body.len() < len {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        body.extend_from_slice(&tmp[..n]);
                    }
                    let req = Request { method, path, headers, body: String::from_utf8_lossy(&body).to_string() };
                    log.lock().push(req.clone());
                    let (status, extra, resp_body) = handler(&req);
                    let mut head = format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n", resp_body.len());
                    for (k, v) in extra {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(resp_body.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (format!("http://{addr}"), log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_long_text_on_whitespace() {
        let text = "word ".repeat(1000);
        let parts = split_text(&text, 2000);
        assert!(parts.len() >= 3);
        assert!(parts.iter().all(|p| p.chars().count() <= 2000));
        assert_eq!(parts.join(" ").split_whitespace().count(), 1000);
        assert_eq!(split_text("short", 2000), vec!["short"]);
        let unbroken = "x".repeat(4500);
        assert_eq!(split_text(&unbroken, 2000).len(), 3);
    }
}
