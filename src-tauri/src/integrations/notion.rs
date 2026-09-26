//! Notion API integration (direct, no third-party automation).
//!
//! Uses the data-source model (Notion-Version 2026-03-11): pages are created
//! under `parent: {type: "data_source_id"}`. Request limits: 2000 characters
//! per rich-text object and 100 blocks per request; longer pages are created
//! with the first 100 blocks and extended with append requests. 429/529
//! responses honour `Retry-After`.

use std::time::Duration;

use rusqlite::params;
use serde::Serialize;
use serde_json::{json, Value};
use ts_rs::TS;

use super::{retry_after, split_text, IntegrationError};
use crate::storage::{new_id, now, Database};

pub const API_BASE: &str = "https://api.notion.com";
pub const NOTION_VERSION: &str = "2026-03-11";
const SERVICE: &str = "Notion";
const MAX_TEXT: usize = 2000;
const MAX_CHILDREN: usize = 100;
const MAX_ATTEMPTS: u32 = 4;

pub struct NotionClient {
    http: reqwest::Client,
    base: String,
    token: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NotionDataSource {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NotionUser {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NotionPage {
    pub id: String,
    pub url: String,
}

impl NotionClient {
    pub fn new(token: String) -> Self {
        Self::with_base(token, API_BASE)
    }

    pub fn with_base(token: String, base: &str) -> Self {
        NotionClient { http: super::http_client(), base: base.trim_end_matches('/').to_string(), token }
    }

    async fn request(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, IntegrationError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut req = self
                .http
                .request(method.clone(), format!("{}{path}", self.base))
                .bearer_auth(&self.token)
                .header("Notion-Version", NOTION_VERSION);
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = req.send().await.map_err(|e| IntegrationError::Network { service: SERVICE, detail: e.to_string() })?;
            let status = resp.status().as_u16();
            if (status == 429 || status == 529 || status == 503) && attempt < MAX_ATTEMPTS {
                let wait = retry_after(&resp);
                tracing::info!(status, wait_s = wait.as_secs(), "Notion asked us to slow down");
                tokio::time::sleep(wait).await;
                continue;
            }
            let v: Value = resp.json().await.unwrap_or(Value::Null);
            return match status {
                200..=299 => Ok(v),
                401 | 403 => Err(IntegrationError::Unauthorized { service: SERVICE }),
                429 | 529 => Err(IntegrationError::RateLimited { service: SERVICE }),
                404 => Err(IntegrationError::Api {
                    service: SERVICE,
                    message: "not found. Make sure the database is shared with the Minutes integration (••• → Connections).".into(),
                }),
                _ => Err(IntegrationError::Api { service: SERVICE, message: v["message"].as_str().unwrap_or("request failed").to_string() }),
            };
        }
    }

    /// Bot name and workspace, used to confirm the token works.
    pub async fn me(&self) -> Result<String, IntegrationError> {
        let v = self.request(reqwest::Method::GET, "/v1/users/me", None).await?;
        let ws = v["bot"]["workspace_name"].as_str().or(v["name"].as_str()).unwrap_or("Notion");
        Ok(ws.to_string())
    }

    pub async fn search_data_sources(&self, query: &str) -> Result<Vec<NotionDataSource>, IntegrationError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..10 {
            let mut body = json!({ "query": query, "filter": { "property": "object", "value": "data_source" }, "page_size": 100 });
            if let Some(c) = &cursor {
                body["start_cursor"] = json!(c);
            }
            let v = self.request(reqwest::Method::POST, "/v1/search", Some(&body)).await?;
            for r in v["results"].as_array().cloned().unwrap_or_default() {
                let name = r["title"].as_array().map(|t| t.iter().filter_map(|x| x["plain_text"].as_str()).collect::<String>()).unwrap_or_default();
                out.push(NotionDataSource { id: r["id"].as_str().unwrap_or_default().into(), name: if name.is_empty() { "Untitled".into() } else { name } });
            }
            if v["has_more"].as_bool() != Some(true) {
                break;
            }
            cursor = v["next_cursor"].as_str().map(String::from);
        }
        Ok(out)
    }

    pub async fn title_property(&self, data_source_id: &str) -> Result<String, IntegrationError> {
        let v = self.request(reqwest::Method::GET, &format!("/v1/data_sources/{data_source_id}"), None).await?;
        v["properties"]
            .as_object()
            .and_then(|props| props.iter().find(|(_, p)| p["type"] == "title").map(|(k, _)| k.clone()))
            .ok_or_else(|| IntegrationError::Api { service: SERVICE, message: "the database has no title property".into() })
    }

    pub async fn users(&self) -> Result<Vec<NotionUser>, IntegrationError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let path = match &cursor {
                Some(c) => format!("/v1/users?page_size=100&start_cursor={c}"),
                None => "/v1/users?page_size=100".into(),
            };
            let v = self.request(reqwest::Method::GET, &path, None).await?;
            for u in v["results"].as_array().cloned().unwrap_or_default() {
                if u["type"] == "person" {
                    out.push(NotionUser {
                        id: u["id"].as_str().unwrap_or_default().into(),
                        name: u["name"].as_str().unwrap_or_default().into(),
                        email: u["person"]["email"].as_str().map(String::from),
                    });
                }
            }
            if v["has_more"].as_bool() != Some(true) {
                break;
            }
            cursor = v["next_cursor"].as_str().map(String::from);
        }
        Ok(out)
    }

    pub async fn create_page(&self, data_source_id: &str, title_prop: &str, title: &str, blocks: &[Value]) -> Result<NotionPage, IntegrationError> {
        let first: Vec<Value> = blocks.iter().take(MAX_CHILDREN).cloned().collect();
        let body = json!({
            "parent": { "type": "data_source_id", "data_source_id": data_source_id },
            "properties": { title_prop: { "title": [{ "type": "text", "text": { "content": title.chars().take(MAX_TEXT).collect::<String>() } }] } },
            "children": first,
        });
        let v = self.request(reqwest::Method::POST, "/v1/pages", Some(&body)).await?;
        let page = NotionPage { id: v["id"].as_str().unwrap_or_default().into(), url: v["url"].as_str().unwrap_or_default().into() };
        for chunk in blocks[first.len()..].chunks(MAX_CHILDREN) {
            let body = json!({ "children": chunk, "position": { "type": "end" } });
            self.request(reqwest::Method::PATCH, &format!("/v1/blocks/{}/children", page.id), Some(&body)).await?;
            tokio::time::sleep(Duration::from_millis(350)).await; // stay under ~3 req/s
        }
        Ok(page)
    }

    pub async fn trash_page(&self, page_id: &str) -> Result<(), IntegrationError> {
        self.request(reqwest::Method::PATCH, &format!("/v1/pages/{page_id}"), Some(&json!({ "in_trash": true }))).await.map(|_| ())
    }
}

// ---- Page content --------------------------------------------------------

/// A piece of rich text: plain text or a Notion user mention.
pub enum Rich {
    Text(String),
    Mention(String),
}

fn rich(parts: &[Rich]) -> Vec<Value> {
    let mut out = Vec::new();
    for p in parts {
        match p {
            Rich::Text(t) => {
                for chunk in split_text(t, MAX_TEXT) {
                    if !chunk.is_empty() {
                        out.push(json!({ "type": "text", "text": { "content": chunk } }));
                    }
                }
            }
            Rich::Mention(id) => out.push(json!({ "type": "mention", "mention": { "type": "user", "user": { "id": id } } })),
        }
    }
    out.truncate(100);
    out
}

fn block(kind: &str, parts: &[Rich]) -> Value {
    json!({ "type": kind, kind: { "rich_text": rich(parts) } })
}

fn text_blocks(kind: &str, text: &str) -> Vec<Value> {
    // A very long paragraph becomes several blocks of ≤100 rich-text items.
    split_text(text, MAX_TEXT * 100).into_iter().map(|t| block(kind, &[Rich::Text(t)])).collect()
}

pub struct PageAction {
    pub title: String,
    pub owner_name: Option<String>,
    pub owner_notion_id: Option<String>,
    pub due: Option<String>,
}

pub struct PageContent {
    pub date_line: String,
    pub summary: Vec<String>,
    pub decisions: Vec<String>,
    pub actions: Vec<PageAction>,
    pub questions: Vec<String>,
    /// (speaker, "00:02:13", text); empty when the transcript is not uploaded.
    pub transcript: Vec<(String, String, String)>,
}

pub fn build_blocks(c: &PageContent) -> Vec<Value> {
    let mut b = vec![block("paragraph", &[Rich::Text(c.date_line.clone())])];
    b.push(block("heading_2", &[Rich::Text("Summary".into())]));
    b.extend(c.summary.iter().flat_map(|s| text_blocks("bulleted_list_item", s)));
    if !c.decisions.is_empty() {
        b.push(block("heading_2", &[Rich::Text("Decisions".into())]));
        b.extend(c.decisions.iter().flat_map(|s| text_blocks("bulleted_list_item", s)));
    }
    if !c.actions.is_empty() {
        b.push(block("heading_2", &[Rich::Text("Action items".into())]));
        for a in &c.actions {
            let mut parts = Vec::new();
            match (&a.owner_notion_id, &a.owner_name) {
                (Some(id), _) => parts.push(Rich::Mention(id.clone())),
                (None, Some(n)) => parts.push(Rich::Text(n.clone())),
                _ => {}
            }
            parts.push(Rich::Text(if parts.is_empty() { a.title.clone() } else { format!(" — {}", a.title) }));
            if let Some(d) = &a.due {
                parts.push(Rich::Text(format!(" (due {d})")));
            }
            b.push(json!({ "type": "to_do", "to_do": { "rich_text": rich(&parts), "checked": false } }));
        }
    }
    if !c.questions.is_empty() {
        b.push(block("heading_2", &[Rich::Text("Open questions".into())]));
        b.extend(c.questions.iter().flat_map(|s| text_blocks("bulleted_list_item", s)));
    }
    if !c.transcript.is_empty() {
        b.push(block("heading_2", &[Rich::Text("Transcript".into())]));
        for (speaker, time, text) in &c.transcript {
            b.push(block("heading_3", &[Rich::Text(format!("{speaker} · {time}"))]));
            b.extend(text_blocks("paragraph", text));
        }
    }
    b
}

/// Record the outcome of a Notion sync on the meeting.
pub fn record_page(db: &Database, meeting_id: &str, page: &NotionPage) -> rusqlite::Result<()> {
    let ts = now();
    db.with(|c| {
        c.execute(
            "UPDATE meetings SET notion_page_id = ?2, notion_page_url = ?3, notion_synced_at = ?4, updated_at = ?4 WHERE id = ?1",
            params![meeting_id, page.id, page.url, ts],
        )?;
        c.execute(
            "INSERT INTO integration_operations (id, provider, kind, meeting_id, status, remote_id, created_at, updated_at)
             VALUES (?1, 'notion', 'page_create', ?2, 'succeeded', ?3, ?4, ?4)",
            params![new_id(), meeting_id, page.id, ts],
        )
        .map(|_| ())
    })
}

pub fn record_failure(db: &Database, meeting_id: &str, error: &str) {
    let ts = now();
    let _ = db.with(|c| {
        c.execute(
            "INSERT INTO integration_operations (id, provider, kind, meeting_id, status, error, created_at, updated_at)
             VALUES (?1, 'notion', 'page_create', ?2, 'failed', ?3, ?4, ?4)",
            params![new_id(), meeting_id, error, ts],
        )
    });
}

#[cfg(test)]
mod tests {
    use super::super::test_server::{serve, Handler};
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn content(transcript_lines: usize) -> PageContent {
        PageContent {
            date_line: "26 September 2026 · 47 minutes".into(),
            summary: vec!["Agreed the release plan.".into()],
            decisions: vec!["Target Friday".into()],
            actions: vec![
                PageAction { title: "Update onboarding API".into(), owner_name: Some("Tom".into()), owner_notion_id: Some("u-tom".into()), due: Some("Fri 2 Oct".into()) },
                PageAction { title: "Review onboarding copy".into(), owner_name: Some("Walter".into()), owner_notion_id: None, due: None },
            ],
            questions: vec!["Does legal need to sign off?".into()],
            transcript: (0..transcript_lines).map(|i| ("Walter".into(), format!("00:00:{:02}", i % 60), "Hello ".repeat(10))).collect(),
        }
    }

    #[test]
    fn page_structure_matches_spec() {
        let b = build_blocks(&content(2));
        let kinds: Vec<&str> = b.iter().map(|x| x["type"].as_str().unwrap()).collect();
        assert_eq!(kinds[0], "paragraph");
        assert!(kinds.contains(&"to_do"));
        let todo = b.iter().find(|x| x["type"] == "to_do").unwrap();
        assert_eq!(todo["to_do"]["rich_text"][0]["type"], "mention");
        assert_eq!(todo["to_do"]["checked"], false);
        let headings: Vec<String> = b
            .iter()
            .filter(|x| x["type"] == "heading_2")
            .map(|x| x["heading_2"]["rich_text"][0]["text"]["content"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(headings, ["Summary", "Decisions", "Action items", "Open questions", "Transcript"]);
    }

    #[test]
    fn long_text_respects_limits() {
        let mut c = content(0);
        c.summary = vec!["x ".repeat(5_000)];
        let b = build_blocks(&c);
        for block in &b {
            let kind = block["type"].as_str().unwrap();
            for rt in block[kind]["rich_text"].as_array().unwrap() {
                if let Some(t) = rt["text"]["content"].as_str() {
                    assert!(t.chars().count() <= MAX_TEXT);
                }
            }
        }
    }

    #[tokio::test]
    async fn large_pages_are_appended_in_batches_and_rate_limits_honoured() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        let handler: Handler = Arc::new(move |req| {
            assert_eq!(req.headers.iter().find(|(k, _)| k == "notion-version").map(|(_, v)| v.as_str()), Some(NOTION_VERSION));
            let n = c2.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return (429, vec![("retry-after".into(), "1".into())], json!({ "code": "rate_limited" }).to_string());
            }
            if req.method == "POST" && req.path == "/v1/pages" {
                let v: Value = serde_json::from_str(&req.body).unwrap();
                assert_eq!(v["parent"]["type"], "data_source_id");
                assert!(v["children"].as_array().unwrap().len() <= MAX_CHILDREN);
                return (200, vec![], json!({ "id": "page-1", "url": "https://notion.so/page-1" }).to_string());
            }
            if req.method == "PATCH" {
                let v: Value = serde_json::from_str(&req.body).unwrap();
                assert!(v["children"].as_array().unwrap().len() <= MAX_CHILDREN);
                assert_eq!(v["position"]["type"], "end");
                return (200, vec![], "{}".into());
            }
            (404, vec![], "{}".into())
        });
        let (base, log) = serve(handler).await;
        let client = NotionClient::with_base("ntn_test".into(), &base);
        let blocks = build_blocks(&content(150)); // > 300 blocks
        assert!(blocks.len() > 300);
        let page = client.create_page("ds-1", "Name", "Design Weekly", &blocks).await.unwrap();
        assert_eq!(page.id, "page-1");
        let reqs = log.lock();
        let appends = reqs.iter().filter(|r| r.method == "PATCH").count();
        assert_eq!(appends, (blocks.len() - MAX_CHILDREN).div_ceil(MAX_CHILDREN));
        assert_eq!(reqs.iter().filter(|r| r.method == "POST").count(), 2, "one 429 then one success");
    }

    #[tokio::test]
    async fn unauthorized_is_reported_plainly() {
        let handler: Handler = Arc::new(|_| (401, vec![], json!({ "message": "API token is invalid." }).to_string()));
        let (base, _) = serve(handler).await;
        let err = NotionClient::with_base("bad".into(), &base).me().await.unwrap_err();
        assert!(matches!(err, IntegrationError::Unauthorized { .. }));
    }
}
