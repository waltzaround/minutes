//! Linear GraphQL integration (API-key auth; the auth header is isolated so
//! OAuth can be added later).
//!
//! Duplicate protection: each action item gets a client-generated UUID v4
//! (`linear_idempotency_key`), persisted *before* the first request and sent
//! as `IssueCreateInput.id`. A retry after an unknown outcome first looks the
//! issue up by that id, and re-sending the same id can never create a second
//! issue.
//!
//! The model never supplies Linear ids: assignee, team and project are
//! resolved from deterministic mappings and chosen by the user in review.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ts_rs::TS;

use super::IntegrationError;
use crate::storage::{new_id, now, Database};

pub const ENDPOINT: &str = "https://api.linear.app/graphql";
const SERVICE: &str = "Linear";

#[derive(Debug, Clone)]
pub enum LinearAuth {
    /// Personal API key: sent as-is (no "Bearer").
    ApiKey(String),
    /// OAuth access token (future).
    #[allow(dead_code)]
    OAuth(String),
}

impl LinearAuth {
    fn header(&self) -> String {
        match self {
            LinearAuth::ApiKey(k) => k.clone(),
            LinearAuth::OAuth(t) => format!("Bearer {t}"),
        }
    }
}

pub struct LinearClient {
    http: reqwest::Client,
    endpoint: String,
    auth: LinearAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LinearEntity {
    pub id: String,
    pub name: String,
    /// Team key (e.g. "ENG"), user email, or project team ids joined.
    pub detail: Option<String>,
    #[serde(default)]
    pub team_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LinearDirectory {
    pub workspace: String,
    pub teams: Vec<LinearEntity>,
    pub users: Vec<LinearEntity>,
    pub projects: Vec<LinearEntity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IssueDraft {
    pub title: String,
    pub description: String,
    pub team_id: String,
    pub assignee_id: Option<String>,
    pub project_id: Option<String>,
    /// 0 none, 1 urgent, 2 high, 3 medium, 4 low.
    pub priority: Option<u8>,
    pub due_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IssueRef {
    pub id: String,
    pub identifier: String,
    pub url: String,
}

impl LinearClient {
    pub fn new(auth: LinearAuth) -> Self {
        Self::with_endpoint(auth, ENDPOINT)
    }

    pub fn with_endpoint(auth: LinearAuth, endpoint: &str) -> Self {
        LinearClient { http: super::http_client(), endpoint: endpoint.to_string(), auth }
    }

    async fn gql(&self, query: &str, variables: Value) -> Result<Value, IntegrationError> {
        let resp = self
            .http
            .post(&self.endpoint)
            .header(reqwest::header::AUTHORIZATION, self.auth.header())
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(|e| IntegrationError::Network { service: SERVICE, detail: e.to_string() })?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(IntegrationError::Unauthorized { service: SERVICE });
        }
        let body: Value = resp.json().await.map_err(|e| IntegrationError::Network { service: SERVICE, detail: e.to_string() })?;
        if let Some(errors) = body["errors"].as_array().filter(|e| !e.is_empty()) {
            let code = errors[0]["extensions"]["code"].as_str().unwrap_or_default();
            let message = errors[0]["message"].as_str().unwrap_or("request failed").to_string();
            return Err(match code {
                "RATELIMITED" => IntegrationError::RateLimited { service: SERVICE },
                "AUTHENTICATION_ERROR" => IntegrationError::Unauthorized { service: SERVICE },
                _ => IntegrationError::Api { service: SERVICE, message },
            });
        }
        if !status.is_success() {
            return Err(IntegrationError::Api { service: SERVICE, message: format!("HTTP {status}") });
        }
        Ok(body["data"].clone())
    }

    pub async fn viewer(&self) -> Result<(String, String), IntegrationError> {
        let d = self.gql("query { viewer { id name organization { name urlKey } } }", json!({})).await?;
        Ok((
            d["viewer"]["name"].as_str().unwrap_or_default().to_string(),
            d["viewer"]["organization"]["name"].as_str().unwrap_or_default().to_string(),
        ))
    }

    async fn paginate(&self, field: &str, selection: &str, map: impl Fn(&Value) -> LinearEntity) -> Result<Vec<LinearEntity>, IntegrationError> {
        let mut out = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..50 {
            let q = format!("query($after: String) {{ {field}(first: 100, after: $after) {{ nodes {{ {selection} }} pageInfo {{ hasNextPage endCursor }} }} }}");
            let d = self.gql(&q, json!({ "after": after })).await?;
            let conn = &d[field];
            for n in conn["nodes"].as_array().cloned().unwrap_or_default() {
                out.push(map(&n));
            }
            if conn["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
                break;
            }
            after = conn["pageInfo"]["endCursor"].as_str().map(String::from);
        }
        Ok(out)
    }

    pub async fn directory(&self) -> Result<LinearDirectory, IntegrationError> {
        let (_, workspace) = self.viewer().await?;
        let s = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
        let teams = self
            .paginate("teams", "id key name", |n| LinearEntity { id: s(n, "id"), name: s(n, "name"), detail: Some(s(n, "key")), team_ids: vec![] })
            .await?;
        let users = self
            .paginate("users", "id name displayName email active", |n| LinearEntity {
                id: s(n, "id"),
                name: s(n, "name"),
                detail: n["email"].as_str().map(String::from),
                team_ids: vec![],
            })
            .await?;
        let projects = self
            .paginate("projects", "id name teams { nodes { id } }", |n| LinearEntity {
                id: s(n, "id"),
                name: s(n, "name"),
                detail: None,
                team_ids: n["teams"]["nodes"].as_array().map(|a| a.iter().filter_map(|t| t["id"].as_str().map(String::from)).collect()).unwrap_or_default(),
            })
            .await?;
        Ok(LinearDirectory { workspace, teams, users, projects })
    }

    pub async fn issue_by_id(&self, id: &str) -> Result<Option<IssueRef>, IntegrationError> {
        match self.gql("query($id: String!) { issue(id: $id) { id identifier url } }", json!({ "id": id })).await {
            Ok(d) if d["issue"].is_object() => Ok(Some(IssueRef {
                id: d["issue"]["id"].as_str().unwrap_or_default().into(),
                identifier: d["issue"]["identifier"].as_str().unwrap_or_default().into(),
                url: d["issue"]["url"].as_str().unwrap_or_default().into(),
            })),
            Ok(_) => Ok(None),
            // Linear reports a missing entity as a GraphQL error.
            Err(IntegrationError::Api { message, .. }) if message.to_lowercase().contains("not found") || message.to_lowercase().contains("entity") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn create_issue(&self, key: &str, d: &IssueDraft) -> Result<IssueRef, IntegrationError> {
        let mut input = json!({ "id": key, "teamId": d.team_id, "title": d.title, "description": d.description });
        if let Some(a) = &d.assignee_id {
            input["assigneeId"] = json!(a);
        }
        if let Some(p) = &d.project_id {
            input["projectId"] = json!(p);
        }
        if let Some(p) = d.priority {
            input["priority"] = json!(p);
        }
        if let Some(due) = &d.due_date {
            input["dueDate"] = json!(due);
        }
        let data = self
            .gql(
                "mutation($input: IssueCreateInput!) { issueCreate(input: $input) { success issue { id identifier url } } }",
                json!({ "input": input }),
            )
            .await?;
        let r = &data["issueCreate"];
        if r["success"].as_bool() != Some(true) || !r["issue"].is_object() {
            return Err(IntegrationError::Api { service: SERVICE, message: "the issue was not created".into() });
        }
        Ok(IssueRef {
            id: r["issue"]["id"].as_str().unwrap_or_default().into(),
            identifier: r["issue"]["identifier"].as_str().unwrap_or_default().into(),
            url: r["issue"]["url"].as_str().unwrap_or_default().into(),
        })
    }
}

/// Issue description in Linear markdown (spec format).
pub fn issue_description(meeting_title: &str, meeting_date: &str, context: Option<&str>, evidence: &[(String, u64, String)], notion_url: Option<&str>) -> String {
    let mut s = format!("Created from {meeting_title} — {meeting_date}\n");
    if let Some(c) = context.filter(|c| !c.trim().is_empty()) {
        s.push_str(&format!("\n## Context\n\n{}\n", c.trim()));
    }
    if !evidence.is_empty() {
        s.push_str("\n## Evidence\n");
        for (speaker, ms, text) in evidence {
            let secs = ms / 1000;
            s.push_str(&format!("\n{speaker} · {}:{:02}\n\n> {}\n", secs / 60, secs % 60, text.trim()));
        }
    }
    if let Some(url) = notion_url {
        s.push_str(&format!("\nMeeting notes: {url}\n"));
    }
    s
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateIssueResult {
    pub state: String,
    pub issue: Option<IssueRef>,
    pub error: Option<String>,
}

fn record_op(db: &Database, meeting_id: &str, action_id: &str, key: &str, status: &str, remote: Option<&str>, error: Option<&str>) {
    let ts = now();
    let _ = db.with(|c| {
        c.execute(
            "INSERT INTO integration_operations (id, provider, kind, meeting_id, action_item_id, idempotency_key, status, remote_id, error, created_at, updated_at)
             VALUES (?1, 'linear', 'issue_create', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![new_id(), meeting_id, action_id, key, status, remote, error, ts],
        )
    });
}

/// Create (or confirm) the Linear issue for one reviewed action item.
pub async fn sync_action(db: &Database, client: &LinearClient, action_id: &str, draft: &IssueDraft) -> Result<CreateIssueResult, IntegrationError> {
    if draft.title.trim().is_empty() || draft.team_id.trim().is_empty() {
        return Err(IntegrationError::Invalid("Choose a team and give the issue a title.".into()));
    }
    let row: Option<(String, Option<String>, String, Option<String>, Option<String>, Option<String>)> = db
        .with(|c| {
            c.query_row(
                "SELECT meeting_id, linear_idempotency_key, linear_sync_state, linear_issue_id, linear_issue_identifier, linear_issue_url
                 FROM action_items WHERE id = ?1",
                [action_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()
        })
        .map_err(|e| IntegrationError::Invalid(e.to_string()))?;
    let Some((meeting_id, key, state, issue_id, identifier, url)) = row else {
        return Err(IntegrationError::Invalid("That action item no longer exists.".into()));
    };
    if state == "created" {
        return Ok(CreateIssueResult {
            state,
            issue: issue_id.map(|id| IssueRef { id, identifier: identifier.unwrap_or_default(), url: url.unwrap_or_default() }),
            error: None,
        });
    }
    let previous_attempt_uncertain = state == "pending" || state == "unknown";
    let key = key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    // Persist the key and reviewed draft before any request.
    db.with(|c| {
        c.execute(
            "UPDATE action_items SET linear_idempotency_key = ?2, linear_sync_state = 'pending', linear_team_id = ?3,
                    linear_project_id = ?4, linear_assignee_id = ?5, linear_priority = ?6, linear_error = NULL, updated_at = ?7
             WHERE id = ?1",
            params![action_id, key, draft.team_id, draft.project_id, draft.assignee_id, draft.priority, now()],
        )
    })
    .map_err(|e| IntegrationError::Invalid(e.to_string()))?;

    let mark_created = |issue: &IssueRef| {
        let _ = db.with(|c| {
            c.execute(
                "UPDATE action_items SET linear_sync_state = 'created', linear_issue_id = ?2, linear_issue_identifier = ?3,
                        linear_issue_url = ?4, linear_error = NULL, selected = 0, updated_at = ?5 WHERE id = ?1",
                params![action_id, issue.id, issue.identifier, issue.url, now()],
            )
        });
        record_op(db, &meeting_id, action_id, &key, "succeeded", Some(&issue.id), None);
    };

    if previous_attempt_uncertain {
        if let Some(existing) = client.issue_by_id(&key).await? {
            mark_created(&existing);
            return Ok(CreateIssueResult { state: "created".into(), issue: Some(existing), error: None });
        }
    }

    match client.create_issue(&key, draft).await {
        Ok(issue) => {
            mark_created(&issue);
            Ok(CreateIssueResult { state: "created".into(), issue: Some(issue), error: None })
        }
        Err(e) => {
            // A conflict on our own id means an earlier attempt did succeed.
            if let IntegrationError::Api { .. } = &e {
                if let Ok(Some(existing)) = client.issue_by_id(&key).await {
                    mark_created(&existing);
                    return Ok(CreateIssueResult { state: "created".into(), issue: Some(existing), error: None });
                }
            }
            let state = if e.outcome_unknown() { "unknown" } else { "failed" };
            let msg = e.to_string();
            let _ = db.with(|c| {
                c.execute(
                    "UPDATE action_items SET linear_sync_state = ?2, linear_error = ?3, updated_at = ?4 WHERE id = ?1",
                    params![action_id, state, msg, now()],
                )
            });
            record_op(db, &meeting_id, action_id, &key, if state == "unknown" { "unknown" } else { "failed" }, None, Some(&msg));
            Ok(CreateIssueResult { state: state.into(), issue: None, error: Some(msg) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_server::{serve, Handler};
    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn setup_action(db: &Database) -> String {
        let root = tempfile::tempdir().unwrap();
        let (mid, _, _) = crate::meetings::store::create_meeting(db, "m", root.path(), true, true, "keep_forever").unwrap();
        crate::meetings::analysis::add_manual_action(db, &mid, "Update onboarding API").unwrap()
    }

    fn draft() -> IssueDraft {
        IssueDraft {
            title: "Update onboarding API".into(),
            description: "desc".into(),
            team_id: "team-1".into(),
            assignee_id: Some("user-tom".into()),
            project_id: None,
            priority: Some(3),
            due_date: Some("2026-10-02".into()),
        }
    }

    /// Fake Linear: stores issues by client id; can fail the first create
    /// *after* storing it (simulating a lost response).
    fn fake_linear(drop_first_response: bool) -> (Handler, Arc<AtomicUsize>) {
        let issues: Arc<Mutex<HashMap<String, Value>>> = Arc::new(Mutex::new(HashMap::new()));
        let creates = Arc::new(AtomicUsize::new(0));
        let c2 = creates.clone();
        let handler: Handler = Arc::new(move |req| {
            assert_eq!(req.headers.iter().find(|(k, _)| k == "authorization").map(|(_, v)| v.as_str()), Some("lin_api_test"));
            let body: Value = serde_json::from_str(&req.body).unwrap();
            let q = body["query"].as_str().unwrap();
            if q.contains("issueCreate") {
                let input = &body["variables"]["input"];
                let id = input["id"].as_str().unwrap().to_string();
                let n = c2.fetch_add(1, Ordering::SeqCst);
                let mut store = issues.lock();
                if store.contains_key(&id) {
                    return (200, vec![], json!({ "errors": [{ "message": "Entity with this id already exists", "extensions": { "code": "INVALID_INPUT" } }] }).to_string());
                }
                store.insert(id.clone(), input.clone());
                if drop_first_response && n == 0 {
                    return (502, vec![], "not json".into());
                }
                return (200, vec![], json!({ "data": { "issueCreate": { "success": true, "issue": { "id": id, "identifier": "ENG-42", "url": "https://linear.app/x/issue/ENG-42" } } } }).to_string());
            }
            if q.contains("issue(id") {
                let id = body["variables"]["id"].as_str().unwrap();
                return match issues.lock().get(id) {
                    Some(_) => (200, vec![], json!({ "data": { "issue": { "id": id, "identifier": "ENG-42", "url": "https://linear.app/x/issue/ENG-42" } } }).to_string()),
                    None => (200, vec![], json!({ "errors": [{ "message": "Entity not found", "extensions": { "code": "INVALID_INPUT" } }] }).to_string()),
                };
            }
            (200, vec![], json!({ "data": {} }).to_string())
        });
        (handler, creates)
    }

    #[tokio::test]
    async fn creates_issue_and_stores_links() {
        let (handler, creates) = fake_linear(false);
        let (base, _) = serve(handler).await;
        let client = LinearClient::with_endpoint(LinearAuth::ApiKey("lin_api_test".into()), &base);
        let db = Database::open_in_memory().unwrap();
        let action = setup_action(&db);
        let r = sync_action(&db, &client, &action, &draft()).await.unwrap();
        assert_eq!(r.state, "created");
        assert_eq!(r.issue.as_ref().unwrap().identifier, "ENG-42");
        // Syncing again is a no-op.
        let r2 = sync_action(&db, &client, &action, &draft()).await.unwrap();
        assert_eq!(r2.state, "created");
        assert_eq!(creates.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_after_lost_response_never_duplicates() {
        let (handler, creates) = fake_linear(true);
        let (base, _) = serve(handler).await;
        let client = LinearClient::with_endpoint(LinearAuth::ApiKey("lin_api_test".into()), &base);
        let db = Database::open_in_memory().unwrap();
        let action = setup_action(&db);
        // First attempt: the issue was created but the response was lost.
        let r = sync_action(&db, &client, &action, &draft()).await.unwrap();
        assert_ne!(r.state, "created");
        // Retry: finds the existing issue by our id instead of creating another.
        let r2 = sync_action(&db, &client, &action, &draft()).await.unwrap();
        assert_eq!(r2.state, "created");
        assert_eq!(creates.load(Ordering::SeqCst), 1, "exactly one create request reached Linear");
        let ops: i64 = db.with(|c| c.query_row("SELECT count(*) FROM integration_operations WHERE status = 'succeeded'", [], |r| r.get(0))).unwrap();
        assert_eq!(ops, 1);
    }

    #[tokio::test]
    async fn rate_limit_is_reported_and_retryable() {
        let handler: Handler = Arc::new(|_| (400, vec![], json!({ "errors": [{ "message": "Rate limit exceeded", "extensions": { "code": "RATELIMITED" } }] }).to_string()));
        let (base, _) = serve(handler).await;
        let client = LinearClient::with_endpoint(LinearAuth::ApiKey("k".into()), &base);
        let db = Database::open_in_memory().unwrap();
        let action = setup_action(&db);
        let r = sync_action(&db, &client, &action, &draft()).await.unwrap();
        assert_eq!(r.state, "failed");
        assert!(r.error.unwrap().contains("rate limited"));
    }

    #[test]
    fn description_matches_spec_format() {
        let d = issue_description(
            "Design Weekly",
            "26 Sep 2026",
            Some("The team agreed the API changes come first."),
            &[("Tom".into(), 28 * 60_000 + 43_000, "I'll take the API changes and have them ready Friday.".into())],
            Some("https://notion.so/page"),
        );
        assert!(d.starts_with("Created from Design Weekly — 26 Sep 2026"));
        assert!(d.contains("## Context"));
        assert!(d.contains("Tom · 28:43"));
        assert!(d.contains("> I'll take the API changes"));
        assert!(d.contains("Meeting notes: https://notion.so/page"));
    }

    #[test]
    fn api_key_is_not_sent_as_bearer() {
        assert_eq!(LinearAuth::ApiKey("lin_api_x".into()).header(), "lin_api_x");
        assert_eq!(LinearAuth::OAuth("tok".into()).header(), "Bearer tok");
    }
}
