use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::integrations::linear::{self, CreateIssueResult, IssueDraft, LinearAuth, LinearClient, LinearDirectory};
use crate::integrations::notion::{self, NotionClient, NotionDataSource, NotionPage, NotionUser, PageAction, PageContent};
use crate::integrations::{ConnectionStatus, IntegrationError};
use crate::meetings::analysis;
use crate::people;
use crate::state::AppState;
use crate::storage::secrets::SecretKey;

impl From<IntegrationError> for AppError {
    fn from(e: IntegrationError) -> Self {
        AppError::user("integration", e.to_string())
    }
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IntegrationsStatus {
    pub notion: ConnectionStatus,
    pub linear: ConnectionStatus,
}

fn notion_client(state: &AppState) -> Result<NotionClient, AppError> {
    let token = state.secrets.get(SecretKey::NotionToken)?.ok_or(IntegrationError::NotConnected("Notion"))?;
    Ok(NotionClient::new(token))
}

fn linear_client(state: &AppState) -> Result<LinearClient, AppError> {
    let key = state.secrets.get(SecretKey::LinearApiKey)?.ok_or(IntegrationError::NotConnected("Linear"))?;
    Ok(LinearClient::new(LinearAuth::ApiKey(key)))
}

/// Local status only: no network request and no keychain access.
#[tauri::command]
pub fn integrations_status(state: State<'_, AppState>) -> AppResult<IntegrationsStatus> {
    let s = state.settings.read().clone();
    Ok(IntegrationsStatus {
        notion: ConnectionStatus {
            connected: s.notion.connected,
            account: s.notion.data_source_name.clone(),
        },
        linear: ConnectionStatus {
            connected: s.linear.connected,
            account: s.linear.workspace_name.clone(),
        },
    })
}

#[tauri::command]
pub async fn notion_connect(state: State<'_, AppState>, token: String) -> AppResult<String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(AppError::user("invalid", "Paste the integration secret from Notion."));
    }
    let workspace = NotionClient::new(token.clone()).me().await?;
    state.secrets.set(SecretKey::NotionToken, &token)?;
    let mut s = state.settings.read().clone();
    s.notion.connected = true;
    s.save(&state.db)?;
    *state.settings.write() = s;
    Ok(workspace)
}

#[tauri::command]
pub fn notion_disconnect(state: State<'_, AppState>) -> AppResult<()> {
    state.secrets.delete(SecretKey::NotionToken)?;
    let mut s = state.settings.read().clone();
    s.notion.connected = false;
    s.notion.data_source_id = None;
    s.notion.data_source_name = None;
    s.save(&state.db)?;
    *state.settings.write() = s;
    Ok(())
}

#[tauri::command]
pub async fn notion_search_databases(state: State<'_, AppState>, query: String) -> AppResult<Vec<NotionDataSource>> {
    Ok(notion_client(&state)?.search_data_sources(&query).await?)
}

#[tauri::command]
pub async fn notion_select_database(state: State<'_, AppState>, data_source_id: String, name: String) -> AppResult<()> {
    notion_client(&state)?.title_property(&data_source_id).await?;
    let mut s = state.settings.read().clone();
    s.notion.data_source_id = Some(data_source_id);
    s.notion.data_source_name = Some(name);
    s.save(&state.db)?;
    *state.settings.write() = s;
    Ok(())
}

#[tauri::command]
pub async fn notion_users(state: State<'_, AppState>) -> AppResult<Vec<NotionUser>> {
    Ok(notion_client(&state)?.users().await?)
}

#[tauri::command]
pub async fn linear_connect(state: State<'_, AppState>, api_key: String) -> AppResult<String> {
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return Err(AppError::user("invalid", "Paste a personal API key from Linear."));
    }
    let (_, workspace) = LinearClient::new(LinearAuth::ApiKey(key.clone())).viewer().await?;
    state.secrets.set(SecretKey::LinearApiKey, &key)?;
    let mut s = state.settings.read().clone();
    s.linear.workspace_name = Some(workspace.clone());
    s.linear.connected = true;
    s.save(&state.db)?;
    *state.settings.write() = s;
    Ok(workspace)
}

#[tauri::command]
pub fn linear_disconnect(state: State<'_, AppState>) -> AppResult<()> {
    state.secrets.delete(SecretKey::LinearApiKey)?;
    let mut s = state.settings.read().clone();
    s.linear = Default::default();
    s.save(&state.db)?;
    *state.settings.write() = s;
    Ok(())
}

#[tauri::command]
pub async fn linear_directory(state: State<'_, AppState>) -> AppResult<LinearDirectory> {
    Ok(linear_client(&state)?.directory().await?)
}

#[tauri::command]
pub fn set_person_mapping(
    state: State<'_, AppState>,
    person_id: String,
    provider: String,
    remote_id: Option<String>,
    remote_name: Option<String>,
) -> AppResult<()> {
    if provider != "notion" && provider != "linear" {
        return Err(AppError::user("invalid", "Unknown integration."));
    }
    Ok(people::set_mapping(&state.db, &provider, &person_id, remote_id.as_deref(), remote_name.as_deref())?)
}

#[tauri::command]
pub fn set_linear_defaults(state: State<'_, AppState>, team_id: Option<String>, project_id: Option<String>) -> AppResult<()> {
    let mut s = state.settings.read().clone();
    s.linear.default_team_id = team_id;
    s.linear.default_project_id = project_id;
    s.save(&state.db)?;
    *state.settings.write() = s;
    Ok(())
}

struct MeetingMeta {
    title: String,
    date_long: String,
    date_short: String,
    duration: String,
    notion_url: Option<String>,
    old_page: Option<String>,
}

fn meeting_meta(state: &AppState, meeting_id: &str) -> AppResult<MeetingMeta> {
    let (title, started, duration_ms, url, page): (String, String, Option<i64>, Option<String>, Option<String>) = state.db.with(|c| {
        c.query_row(
            "SELECT title, started_at, duration_ms, notion_page_url, notion_page_id FROM meetings WHERE id = ?1",
            [meeting_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
    })?;
    let local = chrono::DateTime::parse_from_rfc3339(&started).ok().map(|d| d.with_timezone(&chrono::Local));
    let minutes = duration_ms.map(|d| (d / 60_000).max(1)).unwrap_or(0);
    Ok(MeetingMeta {
        title,
        date_long: local.map(|d| d.format("%-d %B %Y").to_string()).unwrap_or_default(),
        date_short: local.map(|d| d.format("%-d %b %Y").to_string()).unwrap_or_default(),
        duration: format!("{minutes} minutes"),
        notion_url: url,
        old_page: page,
    })
}

/// Create the meeting page in Notion. Re-sending replaces the previous page
/// (the new page is created first; the old one is moved to Notion's trash
/// only after that succeeds).
#[tauri::command]
pub async fn sync_to_notion(state: State<'_, AppState>, meeting_id: String, include_transcript: bool) -> AppResult<NotionPage> {
    let settings = state.settings.read().clone();
    let ds = settings
        .notion
        .data_source_id
        .clone()
        .ok_or_else(|| AppError::user("notion_database", "Choose a Notion database in Settings → Notion first."))?;
    let client = notion_client(&state)?;
    let view = analysis::load_view(&state.db, &settings, &meeting_id)?;
    let meta = meeting_meta(&state, &meeting_id)?;
    let t = analysis::transcript_for_model(&state.db, &settings, &meeting_id)?;
    let people_list = people::list(&state.db)?;
    let content = PageContent {
        date_line: format!("{} · {}", meta.date_long, meta.duration),
        summary: view.as_ref().map(|v| v.summary.clone()).unwrap_or_default(),
        decisions: view.as_ref().map(|v| v.decisions.iter().map(|d| d.text.clone()).collect()).unwrap_or_default(),
        actions: view
            .as_ref()
            .map(|v| {
                v.action_items
                    .iter()
                    .filter(|a| !a.dismissed)
                    .map(|a| {
                        let person = a.owner_person_id.as_ref().and_then(|id| people_list.iter().find(|p| &p.id == id));
                        PageAction {
                            title: a.title.clone(),
                            owner_name: person.map(|p| p.display_name.clone()).or_else(|| a.owner_label.clone()),
                            owner_notion_id: person.and_then(|p| p.notion_user_id.clone()),
                            due: a.due_date.clone().or(a.due_text.clone()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        questions: view.as_ref().map(|v| v.unresolved_questions.iter().map(|q| q.text.clone()).collect()).unwrap_or_default(),
        transcript: if include_transcript {
            t.lines
                .iter()
                .map(|l| {
                    let s = l.start_ms / 1000;
                    (l.speaker.clone(), format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60), l.text.clone())
                })
                .collect()
        } else {
            Vec::new()
        },
    };
    let title_prop = client.title_property(&ds).await?;
    let blocks = notion::build_blocks(&content);
    match client.create_page(&ds, &title_prop, &meta.title, &blocks).await {
        Ok(page) => {
            notion::record_page(&state.db, &meeting_id, &page)?;
            if let Some(old) = meta.old_page.filter(|o| *o != page.id) {
                if let Err(e) = client.trash_page(&old).await {
                    tracing::warn!(error = %e, "could not move the previous Notion page to trash");
                }
            }
            Ok(page)
        }
        Err(e) => {
            notion::record_failure(&state.db, &meeting_id, &e.to_string());
            Err(e.into())
        }
    }
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IssueProposal {
    pub action_item_id: String,
    pub draft: IssueDraft,
    /// Why fields were pre-filled (all resolved against real Linear entities).
    pub notes: Vec<String>,
}

/// Build review drafts for the selected action items. Every id comes from a
/// deterministic mapping or the saved defaults — never from the model.
#[tauri::command]
pub async fn prepare_linear_issues(state: State<'_, AppState>, meeting_id: String) -> AppResult<Vec<IssueProposal>> {
    let settings = state.settings.read().clone();
    let dir = linear_client(&state)?.directory().await?;
    let view = analysis::load_view(&state.db, &settings, &meeting_id)?.ok_or_else(|| AppError::user("no_notes", "There are no meeting notes yet."))?;
    let meta = meeting_meta(&state, &meeting_id)?;
    let people_list = people::list(&state.db)?;
    let valid_team = |id: &Option<String>| id.as_ref().filter(|t| dir.teams.iter().any(|x| &x.id == *t)).cloned();
    let default_team = valid_team(&settings.linear.default_team_id).or_else(|| (dir.teams.len() == 1).then(|| dir.teams[0].id.clone()));
    let default_project = settings.linear.default_project_id.clone().filter(|p| dir.projects.iter().any(|x| &x.id == p));
    let mut out = Vec::new();
    for a in view.action_items.iter().filter(|a| a.selected && !a.dismissed && a.linear.state != "created") {
        let mut notes = Vec::new();
        let person = a.owner_person_id.as_ref().and_then(|id| people_list.iter().find(|p| &p.id == id));
        let assignee = person.and_then(|p| p.linear_user_id.clone()).filter(|u| dir.users.iter().any(|x| &x.id == u));
        if let (Some(p), None) = (person, &assignee) {
            notes.push(format!("{} isn't linked to a Linear user yet.", p.display_name));
        }
        // Keep reviewed choices from a previous attempt.
        let team = valid_team(&a.linear.team_id).or(default_team.clone());
        let project = a.linear.project_id.clone().or(default_project.clone()).filter(|p| dir.projects.iter().any(|x| &x.id == p));
        let evidence: Vec<(String, u64, String)> = a.evidence.iter().map(|e| (e.speaker.clone(), e.start_ms, e.text.clone())).collect();
        out.push(IssueProposal {
            action_item_id: a.id.clone(),
            draft: IssueDraft {
                title: a.title.clone(),
                description: linear::issue_description(&meta.title, &meta.date_short, a.description.as_deref(), &evidence, meta.notion_url.as_deref()),
                team_id: team.unwrap_or_default(),
                assignee_id: a.linear.assignee_id.clone().or(assignee),
                project_id: project,
                priority: a.linear.priority.or(Some(0)),
                due_date: a.due_date.clone(),
            },
            notes,
        });
    }
    Ok(out)
}

/// Create one reviewed issue. Safe to retry: never creates duplicates.
#[tauri::command]
pub async fn create_linear_issue(state: State<'_, AppState>, action_item_id: String, draft: IssueDraft) -> AppResult<CreateIssueResult> {
    let client = linear_client(&state)?;
    Ok(linear::sync_action(&state.db, &client, &action_item_id, &draft).await?)
}
