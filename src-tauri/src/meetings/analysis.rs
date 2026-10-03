//! Meeting analysis with the local LLM.
//!
//! ```text
//! queue ─► memory plan (live budget; smaller context/model if needed)
//!       ─► unload speech models when loading must be sequential
//!       ─► start llama-server (or reuse a resident one)
//!       ─► single pass, or chunked analysis + synthesis for long meetings
//!       ─► strict validation; one retry with correction instructions
//!       ─► store with evidence; resolve owners deterministically
//!       ─► stop llama-server unless the profile keeps it resident
//! ```
//!
//! Failures never touch the transcript: the analysis row is marked
//! `unavailable`/`failed` with a user-safe message and can be retried.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use ts_rs::TS;

use super::recorder::EventSink;
use super::speakers::list_clusters;
use super::transcript::list_segments;
use crate::llm::llama::{LlamaConfig, LlamaServer};
use crate::llm::prompts::{self, Line};
use crate::llm::schemas::{self, AssignmentType, MeetingAnalysis, RawAnalysis, TranscriptIndex};
use crate::llm::{model_manager, JsonRequest, JsonResponse, LlmError, LocalLlm};
use crate::models::manager::ModelManager;
use crate::people;
use crate::speech::engine::SpeechEngine;
use crate::speech::speaker_registry::SpeakerIdentity;
use crate::storage::settings::AppSettings;
use crate::storage::{new_id, now, Database};
use crate::system::benchmark::LlmBenchmark;
use crate::system::profile;
use crate::system::service::SystemService;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AnalysisStatus {
    Pending,
    Running,
    Ready,
    /// Could not produce a valid analysis; shown as unavailable, never guessed.
    Unavailable,
    Failed,
}

impl AnalysisStatus {
    fn from_db(s: &str) -> Self {
        match s {
            "pending" => AnalysisStatus::Pending,
            "running" => AnalysisStatus::Running,
            "ready" => AnalysisStatus::Ready,
            "unavailable" => AnalysisStatus::Unavailable,
            _ => AnalysisStatus::Failed,
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EvidenceRef {
    pub segment_id: String,
    pub start_ms: u64,
    pub speaker: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EvidenceItemView {
    pub id: String,
    pub text: String,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LinearLink {
    pub state: String,
    pub issue_id: Option<String>,
    pub identifier: Option<String>,
    pub url: Option<String>,
    pub error: Option<String>,
    pub team_id: Option<String>,
    pub project_id: Option<String>,
    pub assignee_id: Option<String>,
    pub priority: Option<u8>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ActionItemView {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub owner_person_id: Option<String>,
    pub owner_label: Option<String>,
    pub due_date: Option<String>,
    pub due_text: Option<String>,
    pub assignment_type: AssignmentType,
    pub confidence: f32,
    pub selected: bool,
    pub dismissed: bool,
    pub user_edited: bool,
    pub evidence: Vec<EvidenceRef>,
    pub linear: LinearLink,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AnalysisView {
    pub status: AnalysisStatus,
    pub error: Option<String>,
    pub model_id: Option<String>,
    pub strategy: Option<String>,
    pub updated_at: Option<String>,
    /// The transcript was edited after this analysis was produced.
    pub stale: bool,
    pub title: Option<String>,
    pub summary: Vec<String>,
    pub decisions: Vec<EvidenceItemView>,
    pub action_items: Vec<ActionItemView>,
    pub unresolved_questions: Vec<EvidenceItemView>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AnalysisProgress {
    pub meeting_id: String,
    pub status: AnalysisStatus,
    pub message: Option<String>,
    pub fraction: Option<f32>,
}

pub struct AnalysisDeps {
    pub db: Database,
    pub settings: Arc<RwLock<AppSettings>>,
    pub system: Arc<SystemService>,
    pub models: Arc<ModelManager>,
    pub speech: Arc<SpeechEngine>,
    pub sink: EventSink,
    pub runtime_dir: PathBuf,
    pub log_dir: PathBuf,
    pub data_dir: PathBuf,
    pub is_recording: Arc<dyn Fn() -> bool + Send + Sync>,
}

enum Job {
    Analyse(String),
    Benchmark(String, Sender<Result<LlmBenchmark, String>>),
}

pub struct AnalysisService {
    tx: Sender<Job>,
    server: Arc<Mutex<Option<(Arc<LlamaServer>, Instant)>>>,
}

const IDLE_SHUTDOWN: Duration = Duration::from_secs(300);

impl AnalysisService {
    pub fn start(deps: AnalysisDeps) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let server: Arc<Mutex<Option<(Arc<LlamaServer>, Instant)>>> = Arc::new(Mutex::new(None));
        crate::llm::llama::kill_orphan(&deps.data_dir.join("llama-server.pid"));
        {
            let server = server.clone();
            std::thread::Builder::new()
                .name("analysis".into())
                .spawn(move || run(deps, rx, server))
                .expect("spawn analysis worker");
        }
        {
            // Idle reaper for a resident server.
            let server = server.clone();
            std::thread::Builder::new()
                .name("llm-idle-reaper".into())
                .spawn(move || loop {
                    std::thread::sleep(Duration::from_secs(30));
                    let mut s = server.lock();
                    if s.as_ref().is_some_and(|(_, last)| last.elapsed() > IDLE_SHUTDOWN) {
                        *s = None; // Drop stops the process.
                    }
                })
                .expect("spawn reaper");
        }
        AnalysisService { tx, server }
    }

    pub fn enqueue(&self, meeting_id: &str) {
        let _ = self.tx.send(Job::Analyse(meeting_id.to_string()));
    }

    /// Run the LLM benchmark on the analysis thread (serialised with jobs).
    pub fn benchmark(&self, model_id: &str) -> Result<LlmBenchmark, String> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Job::Benchmark(model_id.to_string(), tx)).map_err(|_| "analysis worker stopped".to_string())?;
        rx.recv().map_err(|_| "analysis worker stopped".to_string())?
    }

    /// Stop the resident server (app exit, memory pressure).
    pub fn shutdown_runtime(&self) {
        self.server.lock().take();
    }
}

fn emit(deps: &AnalysisDeps, meeting_id: &str, status: AnalysisStatus, message: Option<String>, fraction: Option<f32>) {
    let p = AnalysisProgress { meeting_id: meeting_id.into(), status, message, fraction };
    (deps.sink)(crate::events::ANALYSIS_PROGRESS, serde_json::to_value(&p).unwrap_or_default());
}

fn run(deps: AnalysisDeps, rx: Receiver<Job>, server: Arc<Mutex<Option<(Arc<LlamaServer>, Instant)>>>) {
    while let Ok(job) = rx.recv() {
        match job {
            Job::Analyse(id) => {
                set_row(&deps.db, &id, AnalysisStatus::Running, None, None, None);
                emit(&deps, &id, AnalysisStatus::Running, None, None);
                match analyse(&deps, &id, &server) {
                    Ok(()) => emit(&deps, &id, AnalysisStatus::Ready, None, Some(1.0)),
                    Err(AnalysisError::Unavailable(msg)) => {
                        set_row(&deps.db, &id, AnalysisStatus::Unavailable, Some(&msg), None, None);
                        emit(&deps, &id, AnalysisStatus::Unavailable, Some(msg), None);
                    }
                    Err(AnalysisError::Failed(msg)) => {
                        tracing::warn!(meeting_id = id, error = msg, "analysis failed");
                        set_row(&deps.db, &id, AnalysisStatus::Failed, Some(&msg), None, None);
                        emit(&deps, &id, AnalysisStatus::Failed, Some(msg), None);
                    }
                }
            }
            Job::Benchmark(model, reply) => {
                let _ = reply.send(benchmark(&deps, &model, &server));
            }
        }
    }
}

#[derive(Debug)]
pub enum AnalysisError {
    Unavailable(String),
    Failed(String),
}

impl From<LlmError> for AnalysisError {
    fn from(e: LlmError) -> Self {
        AnalysisError::Failed(e.to_string())
    }
}

impl From<rusqlite::Error> for AnalysisError {
    fn from(e: rusqlite::Error) -> Self {
        AnalysisError::Failed(format!("database error: {e}"))
    }
}

fn set_row(db: &Database, meeting_id: &str, status: AnalysisStatus, error: Option<&str>, model: Option<&str>, strategy: Option<&str>) {
    let s = match status {
        AnalysisStatus::Pending => "pending",
        AnalysisStatus::Running => "running",
        AnalysisStatus::Ready => "ready",
        AnalysisStatus::Unavailable => "unavailable",
        AnalysisStatus::Failed => "failed",
    };
    let ts = now();
    let _ = db.with(|c| {
        c.execute(
            "INSERT INTO meeting_analysis (id, meeting_id, status, error, model_id, strategy, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(meeting_id) DO UPDATE SET status = excluded.status, error = excluded.error,
               model_id = COALESCE(excluded.model_id, meeting_analysis.model_id),
               strategy = COALESCE(excluded.strategy, meeting_analysis.strategy), updated_at = excluded.updated_at",
            params![new_id(), meeting_id, s, error, model, strategy, ts],
        )
    });
}

/// Speaker name for each segment plus deterministic label → person mapping.
pub struct TranscriptForModel {
    pub lines: Vec<Line>,
    pub meeting_date: Option<chrono::NaiveDate>,
    pub label_to_person: HashMap<String, String>,
    pub revision: u32,
    pub title: String,
    pub date: String,
}

pub fn transcript_for_model(db: &Database, settings: &AppSettings, meeting_id: &str) -> rusqlite::Result<TranscriptForModel> {
    let segments = db.with(|c| list_segments(c, meeting_id))?;
    let clusters = list_clusters(db, meeting_id)?;
    let people_list = people::list(db)?;
    let (self_id, self_label) = super::processor::self_identity(db, settings);
    let name_of = |pid: &str| people_list.iter().find(|p| p.id == pid).map(|p| p.display_name.clone());
    let mut label_to_person = HashMap::new();
    let mut rows = Vec::new();
    for s in &segments {
        let cluster = clusters.iter().find(|c| Some(&c.id) == s.speaker_cluster_id.as_ref());
        // Only confirmed/known identities give a person name; "possible" stays anonymous.
        let person = s.person_id.clone().or_else(|| match cluster.map(|c| &c.identity) {
            Some(SpeakerIdentity::Known { person_id, .. }) => Some(person_id.clone()),
            _ => None,
        });
        let label = match (&person, cluster) {
            (Some(p), _) => name_of(p).unwrap_or_else(|| "Unknown".into()),
            (None, Some(c)) => c.label.clone(),
            (None, None) if s.source == super::transcript::SegmentSource::Microphone => self_label.clone(),
            _ => "Meeting audio".into(),
        };
        if let Some(p) = person.clone().or_else(|| (s.source == super::transcript::SegmentSource::Microphone).then(|| self_id.clone()).flatten()) {
            label_to_person.insert(label.clone(), p);
        }
        rows.push((s.id.clone(), s.start_ms, label, s.text.clone()));
    }
    let (title, started, revision): (String, String, u32) = db.with(|c| {
        c.query_row("SELECT title, started_at, transcript_revision FROM meetings WHERE id = ?1", [meeting_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
    })?;
    let local = chrono::DateTime::parse_from_rfc3339(&started).ok().map(|d| d.with_timezone(&chrono::Local));
    let date = local.map(|d| d.format("%A %-d %B %Y").to_string()).unwrap_or(started);
    Ok(TranscriptForModel {
        lines: prompts::build_lines(rows),
        meeting_date: local.map(|d| d.date_naive()),
        label_to_person,
        revision,
        title,
        date,
    })
}

/// Ask for an analysis, validate, retry once with corrections.
pub fn request_validated(llm: &dyn LocalLlm, user: &str, index: &TranscriptIndex) -> Result<MeetingAnalysis, AnalysisError> {
    request_validated_with_metrics(llm, user, index).map(|(analysis, _)| analysis)
}

fn request_validated_with_metrics(llm: &dyn LocalLlm, user: &str, index: &TranscriptIndex) -> Result<(MeetingAnalysis, JsonResponse), AnalysisError> {
    let max_tokens = prompts::output_budget(llm.context_tokens());
    let base = JsonRequest {
        system: prompts::SYSTEM_PROMPT.into(),
        user: user.into(),
        schema: schemas::analysis_schema(),
        max_tokens,
        temperature: 0.2,
    };
    let first = llm.complete_json(&base)?;
    let (problems, analysis) = match schemas::parse(&first.content) {
        Ok(raw) => {
            let (a, report) = schemas::validate(&raw, index);
            if report.is_ok() {
                return Ok((a, first));
            }
            (report.problems, Some(a))
        }
        Err(e) => (vec![format!("the answer was not valid JSON for the required structure ({e})")], None),
    };
    tracing::info!(?problems, "analysis needs correction; retrying once");
    let retry = JsonRequest {
        user: format!("{user}\n\nPrevious answer:\n{}\n\n{}", first.content, prompts::correction_prompt(&problems)),
        ..base
    };
    let second = llm.complete_json(&retry)?;
    match schemas::parse(&second.content) {
        Ok(raw) => {
            let (a, report) = schemas::validate(&raw, index);
            if !report.is_ok() {
                // Items without valid evidence were dropped by the validator; the
                // rest is kept. Structural problems never reach this point.
                tracing::warn!(problems = ?report.problems, "analysis still had problems after retry; unsupported items dropped");
            }
            if a.title.is_empty() || a.summary.is_empty() {
                return Err(AnalysisError::Unavailable("The local AI returned an empty summary after retrying. Your transcript is safe; you can try again.".into()));
            }
            Ok((a, second))
        }
        Err(e) => {
            let _ = analysis;
            Err(AnalysisError::Unavailable(format!(
                "The local AI did not produce a valid summary for this meeting ({e}). Your transcript is safe; you can try again."
            )))
        }
    }
}

fn with_date(mut ix: TranscriptIndex, d: Option<chrono::NaiveDate>) -> TranscriptIndex {
    ix.meeting_date = d;
    ix
}

fn run_analysis(llm: &dyn LocalLlm, t: &TranscriptForModel, progress: &dyn Fn(f32)) -> Result<(MeetingAnalysis, &'static str), AnalysisError> {
    let index = with_date(prompts::index(&t.lines), t.meeting_date);
    let budget = prompts::transcript_budget(llm.context_tokens());
    let chunks = prompts::chunk_lines(&t.lines, budget);
    if chunks.len() <= 1 {
        let user = prompts::user_prompt(&t.title, &t.date, &t.lines, None);
        return Ok((request_validated(llm, &user, &index)?, "single_pass"));
    }
    // Chunked: structured intermediate results, then synthesis.
    let n = chunks.len();
    let mut parts: Vec<RawAnalysis> = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        progress(i as f32 / (n + 1) as f32);
        let user = prompts::user_prompt(&t.title, &t.date, chunk, Some((i + 1, n)));
        let chunk_index = with_date(prompts::index(chunk), t.meeting_date);
        let a = request_validated(llm, &user, &chunk_index)?;
        parts.push(to_raw(&a, &t.lines));
    }
    progress(n as f32 / (n + 1) as f32);
    let synth = prompts::synthesis_prompt(&t.title, &t.date, &index.speakers, &parts);
    anyhow_fit(llm, &synth)?;
    Ok((request_validated(llm, &synth, &index)?, "chunked"))
}

/// The synthesis input must fit too; if the parts are very large, fail
/// clearly rather than silently truncating.
fn anyhow_fit(llm: &dyn LocalLlm, prompt: &str) -> Result<(), AnalysisError> {
    if prompts::estimate_tokens(prompt) > prompts::transcript_budget(llm.context_tokens()) {
        return Err(AnalysisError::Unavailable(
            "This meeting is too long to summarise with the current model settings. Try a larger context in Settings → Advanced.".into(),
        ));
    }
    Ok(())
}

/// Convert a validated chunk result back to alias form for synthesis.
fn to_raw(a: &MeetingAnalysis, lines: &[Line]) -> RawAnalysis {
    let alias = |ids: &[String]| -> Vec<String> {
        ids.iter().filter_map(|id| lines.iter().find(|l| &l.segment_id == id).map(|l| l.alias.clone())).collect()
    };
    RawAnalysis {
        title: a.title.clone(),
        summary: a.summary.clone(),
        decisions: a.decisions.iter().map(|d| schemas::RawEvidenceItem { text: d.text.clone(), evidence: alias(&d.evidence_segment_ids) }).collect(),
        action_items: a
            .action_items
            .iter()
            .map(|x| schemas::RawActionItem {
                title: x.title.clone(),
                description: x.description.clone(),
                owner: x.owner_label.clone(),
                due: x.due_text.clone(),
                due_date: x.due_date.clone(),
                assignment_type: x.assignment_type,
                evidence: alias(&x.evidence_segment_ids),
                confidence: x.confidence as f64,
            })
            .collect(),
        unresolved_questions: a
            .unresolved_questions
            .iter()
            .map(|d| schemas::RawEvidenceItem { text: d.text.clone(), evidence: alias(&d.evidence_segment_ids) })
            .collect(),
    }
}

fn ensure_server(
    deps: &AnalysisDeps,
    server: &Mutex<Option<(Arc<LlamaServer>, Instant)>>,
    model_id: &str,
    context: u32,
) -> Result<Arc<LlamaServer>, AnalysisError> {
    let mut guard = server.lock();
    if let Some((s, _)) = guard.as_ref() {
        if s.model_id() == model_id && s.context_tokens() == context {
            let s = s.clone();
            *guard = Some((s.clone(), Instant::now()));
            return Ok(s);
        }
    }
    *guard = None; // stop a server with the wrong model/context first
    let path = model_manager::model_path(&deps.db, &deps.models, model_id).ok_or_else(|| {
        AnalysisError::Unavailable("The meeting summary model is not installed. Download it in Settings → Models.".into())
    })?;
    let settings = deps.settings.read().clone();
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let cfg = LlamaConfig {
        runtime_dir: deps.runtime_dir.clone(),
        model_id: model_id.to_string(),
        model_path: path,
        context_tokens: context,
        gpu_layers: settings.advanced.gpu_layers,
        backend: settings.advanced.inference_backend,
        threads: None,
        log_path: deps.log_dir.join("llama-server.log"),
        pid_path: deps.data_dir.join("llama-server.pid"),
        // Large models on slow disks can take a while to map.
        load_timeout: Duration::from_secs(120 + size / (100 << 20)),
        request_timeout: Duration::from_secs(900),
    };
    let s = Arc::new(LlamaServer::start(cfg)?);
    *guard = Some((s.clone(), Instant::now()));
    Ok(s)
}

fn choose_model(deps: &AnalysisDeps) -> Result<(String, u32, bool), AnalysisError> {
    let settings = deps.settings.read().clone();
    let hw = deps.system.detect_light();
    let bench = deps.system.load_benchmarks(&hw);
    let assessment = profile::assess(&hw, Some(&bench));
    let model_id = settings
        .advanced
        .llm_model_id
        .clone()
        .or(assessment.settings.llm_model_id.clone())
        .ok_or_else(|| AnalysisError::Unavailable("Meeting summaries are not available on this computer. Your transcript is complete.".into()))?;
    let spec = model_manager::spec_for(&deps.db, &model_id)
        .ok_or_else(|| AnalysisError::Unavailable("The selected summary model is not installed.".into()))?;
    let context = settings.advanced.context_tokens.unwrap_or(if assessment.settings.context_tokens > 0 {
        assessment.settings.context_tokens
    } else {
        8192
    });
    let plan = profile::plan_for_live_conditions(&hw, &spec, context, deps.speech.asr_loaded() || (deps.is_recording)());
    if let Some(w) = &plan.warning {
        let payload = crate::events::MemoryWarning { message: w.clone(), available_bytes: hw.memory.available_bytes };
        (deps.sink)(crate::events::MEMORY_WARNING, serde_json::to_value(&payload).unwrap_or_default());
    }
    let chosen = plan.llm_model_id.ok_or_else(|| AnalysisError::Failed(plan.warning.clone().unwrap_or_default()))?;
    // If the plan fell back to a model that isn't installed, stay with an installed one.
    let chosen = if model_manager::model_path(&deps.db, &deps.models, &chosen).is_some() { chosen } else { model_id };
    let keep_resident = assessment.settings.keep_llm_resident && !plan.unload_speech_first;
    if plan.unload_speech_first {
        deps.speech.unload_asr();
    }
    Ok((chosen, plan.context_tokens, keep_resident))
}

fn analyse(deps: &AnalysisDeps, meeting_id: &str, server: &Mutex<Option<(Arc<LlamaServer>, Instant)>>) -> Result<(), AnalysisError> {
    let settings = deps.settings.read().clone();
    let t = transcript_for_model(&deps.db, &settings, meeting_id)?;
    if t.lines.is_empty() {
        return Err(AnalysisError::Unavailable("There is no transcript to summarise.".into()));
    }
    // Sequential tiers: don't load the LLM while a meeting is being recorded.
    let mut waited = false;
    while (deps.is_recording)() && deps.system.cached_hardware().is_some_and(|hw| {
        profile::assess(&hw, None).settings.sequential_loading
    }) {
        if !waited {
            emit(deps, meeting_id, AnalysisStatus::Pending, Some("Waiting for the current recording to finish.".into()), None);
            waited = true;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    let (model_id, context, keep_resident) = choose_model(deps)?;
    emit(deps, meeting_id, AnalysisStatus::Running, Some("Loading the summary model…".into()), Some(0.05));
    let llm = ensure_server(deps, server, &model_id, context)?;
    let progress = |f: f32| emit(deps, meeting_id, AnalysisStatus::Running, Some("Writing notes…".into()), Some(0.1 + f * 0.85));
    let result = run_analysis(llm.as_ref(), &t, &progress);
    if !keep_resident {
        server.lock().take();
    } else if let Some(s) = server.lock().as_mut() {
        s.1 = Instant::now();
    }
    let (mut analysis, strategy) = result?;
    // Deterministic owner resolution.
    for a in &mut analysis.action_items {
        a.owner_person_id = a.owner_label.as_ref().and_then(|l| t.label_to_person.get(l).cloned());
    }
    store_analysis(&deps.db, meeting_id, &analysis, &model_id, context, strategy, t.revision)?;
    Ok(())
}

pub fn store_analysis(
    db: &Database,
    meeting_id: &str,
    a: &MeetingAnalysis,
    model_id: &str,
    context: u32,
    strategy: &str,
    revision: u32,
) -> rusqlite::Result<()> {
    let ts = now();
    db.transaction(|tx| {
        // Replace previous analysis output, but keep action items already sent
        // to Linear (their issue links must never be lost).
        tx.execute("DELETE FROM meeting_analysis WHERE meeting_id = ?1", [meeting_id])?;
        tx.execute("DELETE FROM action_items WHERE meeting_id = ?1 AND linear_sync_state IN ('none', 'failed')", [meeting_id])?;
        let aid = new_id();
        tx.execute(
            "INSERT INTO meeting_analysis (id, meeting_id, status, title, summary_json, model_id, context_tokens, strategy, transcript_revision, created_at, updated_at)
             VALUES (?1, ?2, 'ready', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![aid, meeting_id, a.title, serde_json::to_string(&a.summary).unwrap_or_default(), model_id, context, strategy, revision, ts],
        )?;
        for (i, d) in a.decisions.iter().enumerate() {
            let id = new_id();
            tx.execute("INSERT INTO decisions (id, analysis_id, position, text) VALUES (?1, ?2, ?3, ?4)", params![id, aid, i as i64, d.text])?;
            for s in &d.evidence_segment_ids {
                tx.execute("INSERT OR IGNORE INTO decision_evidence (decision_id, segment_id) VALUES (?1, ?2)", params![id, s])?;
            }
        }
        for (i, q) in a.unresolved_questions.iter().enumerate() {
            let id = new_id();
            tx.execute("INSERT INTO unresolved_questions (id, analysis_id, position, text) VALUES (?1, ?2, ?3, ?4)", params![id, aid, i as i64, q.text])?;
            for s in &q.evidence_segment_ids {
                tx.execute("INSERT OR IGNORE INTO question_evidence (question_id, segment_id) VALUES (?1, ?2)", params![id, s])?;
            }
        }
        let offset: i64 = tx.query_row("SELECT COALESCE(max(position) + 1, 0) FROM action_items WHERE meeting_id = ?1", [meeting_id], |r| r.get(0))?;
        for (i, x) in a.action_items.iter().enumerate() {
            let id = new_id();
            let selected = x.assignment_type.is_strong() && x.confidence >= 0.6;
            tx.execute(
                "INSERT INTO action_items (id, meeting_id, analysis_id, position, title, description, owner_person_id, owner_label, due_date, due_text,
                                           assignment_type, confidence, selected, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)",
                params![
                    id,
                    meeting_id,
                    aid,
                    offset + i as i64,
                    x.title,
                    x.description,
                    x.owner_person_id,
                    x.owner_label,
                    x.due_date,
                    x.due_text,
                    x.assignment_type.as_db(),
                    x.confidence,
                    selected as i32,
                    ts
                ],
            )?;
            for s in &x.evidence_segment_ids {
                tx.execute("INSERT OR IGNORE INTO action_item_evidence (action_item_id, segment_id) VALUES (?1, ?2)", params![id, s])?;
            }
            if let Some(p) = &x.owner_person_id {
                tx.execute("INSERT OR IGNORE INTO meeting_participants (meeting_id, person_id) VALUES (?1, ?2)", params![meeting_id, p])?;
            }
        }
        // Use the model's title only if the user has not renamed the meeting.
        tx.execute(
            "UPDATE meetings SET title = ?2, updated_at = ?3 WHERE id = ?1 AND title LIKE 'Meeting %'",
            params![meeting_id, a.title, ts],
        )?;
        Ok(())
    })
}

fn evidence_for(c: &rusqlite::Connection, sql: &str, id: &str, speakers: &HashMap<String, String>) -> rusqlite::Result<Vec<EvidenceRef>> {
    let mut stmt = c.prepare(sql)?;
    let rows = stmt.query_map([id], |r| {
        let sid: String = r.get(0)?;
        Ok(EvidenceRef {
            speaker: speakers.get(&sid).cloned().unwrap_or_default(),
            segment_id: sid,
            start_ms: r.get::<_, i64>(1)? as u64,
            text: r.get(2)?,
        })
    })?;
    rows.collect()
}

pub fn load_view(db: &Database, settings: &AppSettings, meeting_id: &str) -> rusqlite::Result<Option<AnalysisView>> {
    let row: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<u32>, String)> = db.with(|c| {
        c.query_row(
            "SELECT id, status, error, model_id, title, summary_json, transcript_revision, updated_at FROM meeting_analysis WHERE meeting_id = ?1",
            [meeting_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
        )
        .optional()
    })?;
    let Some((aid, status, error, model, title, summary_json, revision, updated)) = row else { return Ok(None) };
    let status = AnalysisStatus::from_db(status.as_deref().unwrap_or("failed"));
    let t = transcript_for_model(db, settings, meeting_id)?;
    let speakers: HashMap<String, String> = t.lines.iter().map(|l| (l.segment_id.clone(), l.speaker.clone())).collect();
    let strategy: Option<String> = db.with(|c| c.query_row("SELECT strategy FROM meeting_analysis WHERE id = ?1", [&aid], |r| r.get(0)))?;
    db.with(|c| {
        let items = |sql: &str, ev_sql: &str| -> rusqlite::Result<Vec<EvidenceItemView>> {
            let mut stmt = c.prepare(sql)?;
            let rows: Vec<(String, String)> = stmt.query_map([&aid], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            rows.into_iter()
                .map(|(id, text)| Ok(EvidenceItemView { evidence: evidence_for(c, ev_sql, &id, &speakers)?, id, text }))
                .collect()
        };
        let decisions = items(
            "SELECT id, text FROM decisions WHERE analysis_id = ?1 ORDER BY position",
            "SELECT t.id, t.start_ms, t.text FROM decision_evidence e JOIN transcript_segments t ON t.id = e.segment_id WHERE e.decision_id = ?1 ORDER BY t.start_ms",
        )?;
        let questions = items(
            "SELECT id, text FROM unresolved_questions WHERE analysis_id = ?1 ORDER BY position",
            "SELECT t.id, t.start_ms, t.text FROM question_evidence e JOIN transcript_segments t ON t.id = e.segment_id WHERE e.question_id = ?1 ORDER BY t.start_ms",
        )?;
        let action_items = load_actions(c, meeting_id, &speakers)?;
        Ok(Some(AnalysisView {
            status,
            error,
            model_id: model,
            strategy,
            updated_at: Some(updated),
            stale: revision.is_some_and(|r| r != t.revision),
            title,
            summary: summary_json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default(),
            decisions,
            action_items,
            unresolved_questions: questions,
        }))
    })
}

pub fn load_actions(c: &rusqlite::Connection, meeting_id: &str, speakers: &HashMap<String, String>) -> rusqlite::Result<Vec<ActionItemView>> {
    let mut stmt = c.prepare(
        "SELECT id, title, description, owner_person_id, owner_label, due_date, due_text, assignment_type, confidence, selected, dismissed,
                user_edited, linear_sync_state, linear_issue_id, linear_issue_identifier, linear_issue_url, linear_error,
                linear_team_id, linear_project_id, linear_assignee_id, linear_priority
         FROM action_items WHERE meeting_id = ?1 ORDER BY position",
    )?;
    let rows: Vec<ActionItemView> = stmt
        .query_map([meeting_id], |r| {
            Ok(ActionItemView {
                id: r.get(0)?,
                title: r.get(1)?,
                description: r.get(2)?,
                owner_person_id: r.get(3)?,
                owner_label: r.get(4)?,
                due_date: r.get(5)?,
                due_text: r.get(6)?,
                assignment_type: AssignmentType::from_db(&r.get::<_, String>(7)?),
                confidence: r.get(8)?,
                selected: r.get::<_, i64>(9)? != 0,
                dismissed: r.get::<_, i64>(10)? != 0,
                user_edited: r.get::<_, i64>(11)? != 0,
                evidence: Vec::new(),
                linear: LinearLink {
                    state: r.get(12)?,
                    issue_id: r.get(13)?,
                    identifier: r.get(14)?,
                    url: r.get(15)?,
                    error: r.get(16)?,
                    team_id: r.get(17)?,
                    project_id: r.get(18)?,
                    assignee_id: r.get(19)?,
                    priority: r.get::<_, Option<i64>>(20)?.map(|p| p as u8),
                },
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    rows.into_iter()
        .map(|mut a| {
            a.evidence = evidence_for(
                c,
                "SELECT t.id, t.start_ms, t.text FROM action_item_evidence e JOIN transcript_segments t ON t.id = e.segment_id WHERE e.action_item_id = ?1 ORDER BY t.start_ms",
                &a.id,
                speakers,
            )?;
            Ok(a)
        })
        .collect()
}

#[derive(Debug, Clone, serde::Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ActionItemPatch {
    #[serde(default)]
    #[ts(optional)]
    pub title: Option<String>,
    /// Absent = unchanged; `null` = clear.
    #[serde(default, deserialize_with = "double_option")]
    #[ts(optional = nullable)]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    #[ts(optional = nullable)]
    pub owner_person_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    #[ts(optional = nullable)]
    pub due_date: Option<Option<String>>,
    #[serde(default)]
    #[ts(optional)]
    pub selected: Option<bool>,
    #[serde(default)]
    #[ts(optional)]
    pub dismissed: Option<bool>,
}

/// Distinguishes a missing field (`None`) from an explicit `null` (`Some(None)`).
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    serde::Deserialize::deserialize(de).map(Some)
}

pub fn update_action(db: &Database, id: &str, p: &ActionItemPatch) -> anyhow::Result<()> {
    if let Some(Some(d)) = &p.due_date {
        anyhow::ensure!(chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok(), "Use a valid date.");
    }
    if let Some(t) = &p.title {
        anyhow::ensure!(!t.trim().is_empty(), "An action needs a title.");
    }
    let edits_content = p.title.is_some() || p.description.is_some() || p.owner_person_id.is_some() || p.due_date.is_some();
    db.with(|c| {
        c.execute(
            "UPDATE action_items SET
               title = COALESCE(?2, title),
               description = CASE WHEN ?3 THEN ?4 ELSE description END,
               owner_person_id = CASE WHEN ?5 THEN ?6 ELSE owner_person_id END,
               due_date = CASE WHEN ?7 THEN ?8 ELSE due_date END,
               selected = COALESCE(?9, selected),
               dismissed = COALESCE(?10, dismissed),
               user_edited = CASE WHEN ?11 THEN 1 ELSE user_edited END,
               updated_at = ?12
             WHERE id = ?1",
            params![
                id,
                p.title.as_deref().map(str::trim),
                p.description.is_some(),
                p.description.clone().flatten(),
                p.owner_person_id.is_some(),
                p.owner_person_id.clone().flatten(),
                p.due_date.is_some(),
                p.due_date.clone().flatten(),
                p.selected.map(|b| b as i32),
                p.dismissed.map(|b| b as i32),
                edits_content,
                now()
            ],
        )
    })?;
    Ok(())
}

/// Add an action manually (always user-owned, with optional evidence).
pub fn add_manual_action(db: &Database, meeting_id: &str, title: &str) -> anyhow::Result<String> {
    anyhow::ensure!(!title.trim().is_empty(), "An action needs a title.");
    let id = new_id();
    db.with(|c| {
        let pos: i64 = c.query_row("SELECT COALESCE(max(position) + 1, 0) FROM action_items WHERE meeting_id = ?1", [meeting_id], |r| r.get(0))?;
        c.execute(
            "INSERT INTO action_items (id, meeting_id, position, title, assignment_type, confidence, selected, user_edited, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'explicit_assignment', 1.0, 1, 1, ?5, ?5)",
            params![id, meeting_id, pos, title.trim(), now()],
        )
    })?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// LLM benchmark
// ---------------------------------------------------------------------------

const BENCH_TRANSCRIPT: &[(&str, &str)] = &[
    ("Walter", "Okay, let's lock the onboarding release for the twelfth."),
    ("Tom", "The API changes aren't done yet. I can take them and have them ready by Friday."),
    ("Walter", "Great. Sarah, can you review the new onboarding copy before Thursday?"),
    ("Sarah", "Sure, I'll do that."),
    ("Walter", "Maybe Tom can also look at the analytics discrepancy at some point."),
    ("Sarah", "Do we know whether legal needs to sign off on the new terms?"),
    ("Walter", "Not yet. Let's decide the pricing page stays as it is for this release."),
];

fn benchmark(deps: &AnalysisDeps, model_id: &str, server: &Mutex<Option<(Arc<LlamaServer>, Instant)>>) -> Result<LlmBenchmark, String> {
    let mut out = LlmBenchmark { model_id: model_id.into(), context_tokens: 4096, ..Default::default() };
    let hw = deps.system.cached_hardware().unwrap_or_else(|| deps.system.detect_full());
    out.backend = profile::llm_backend(&hw).into();
    deps.speech.unload_asr();
    server.lock().take();
    let started = Instant::now();
    let llm = match ensure_server(deps, server, model_id, 4096) {
        Ok(s) => s,
        Err(AnalysisError::Unavailable(m)) => return Err(m),
        Err(AnalysisError::Failed(m)) => {
            out.error = Some(m);
            return Ok(out);
        }
    };
    out.load_ms = started.elapsed().as_millis() as u64;
    let lines = prompts::build_lines(
        BENCH_TRANSCRIPT.iter().enumerate().map(|(i, (s, t))| (format!("b{i}"), i as u64 * 6000, s.to_string(), t.to_string())),
    );
    let index = with_date(prompts::index(&lines), chrono::NaiveDate::from_ymd_opt(2026, 9, 25));
    let user = prompts::user_prompt("Benchmark", "Friday 25 September 2026", &lines, None);
    benchmark_requests(llm.as_ref(), &user, &index, &mut out);
    out.peak_rss_bytes = crate::system::benchmark::current_rss();
    server.lock().take();
    Ok(out)
}

/// Benchmark the same validation and correction path used for real meetings.
fn benchmark_requests(llm: &dyn LocalLlm, user: &str, index: &TranscriptIndex, out: &mut LlmBenchmark) {
    out.stable = true;
    out.structured_output_valid = true;
    for i in 0..2 {
        match request_validated_with_metrics(llm, user, index) {
            Ok((_, r)) => {
                // First run warms caches; report the second.
                if i == 1 || out.generated_tokens == 0 {
                    out.prompt_tokens = r.prompt_tokens;
                    out.prompt_tokens_per_second = r.prompt_tokens_per_second;
                    out.generated_tokens = r.completion_tokens;
                    out.generation_tokens_per_second = r.generation_tokens_per_second;
                }
            }
            Err(e) => {
                out.stable = false;
                out.structured_output_valid = false;
                out.error = Some(match e {
                    AnalysisError::Unavailable(message) | AnalysisError::Failed(message) => message,
                });
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::JsonResponse;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ScriptedLlm {
        answers: Vec<String>,
        calls: AtomicUsize,
    }

    impl LocalLlm for ScriptedLlm {
        fn complete_json(&self, _: &JsonRequest) -> Result<JsonResponse, LlmError> {
            let i = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(JsonResponse { content: self.answers.get(i).cloned().unwrap_or_default(), ..Default::default() })
        }
        fn context_tokens(&self) -> u32 {
            8192
        }
        fn model_id(&self) -> &str {
            "scripted"
        }
    }

    fn index() -> TranscriptIndex {
        prompts::index(&prompts::build_lines(vec![
            ("a".into(), 0, "Walter".into(), "Tom, can you do the API?".into()),
            ("b".into(), 5000, "Tom".into(), "Yes, I'll do it by Friday.".into()),
        ]))
    }

    fn answer(evidence: &str) -> String {
        serde_json::json!({
            "title": "Sync", "summary": ["API work agreed."],
            "decisions": [], "unresolvedQuestions": [],
            "actionItems": [{ "title": "API changes", "description": null, "owner": "Tom", "due": "Friday", "dueDate": null,
                              "assignmentType": "explicit_acceptance", "evidence": [evidence], "confidence": 0.9 }]
        })
        .to_string()
    }

    #[test]
    fn patch_distinguishes_missing_and_null() {
        let p: ActionItemPatch = serde_json::from_str(r#"{"ownerPersonId": null, "title": "x"}"#).unwrap();
        assert_eq!(p.owner_person_id, Some(None));
        assert_eq!(p.due_date, None);
        assert_eq!(p.title.as_deref(), Some("x"));
    }

    #[test]
    fn valid_first_answer_needs_no_retry() {
        let llm = ScriptedLlm { answers: vec![answer("S2")], calls: AtomicUsize::new(0) };
        let a = request_validated(&llm, "u", &index()).unwrap();
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        assert_eq!(a.action_items[0].evidence_segment_ids, vec!["b"]);
    }

    #[test]
    fn retries_once_with_corrections() {
        let llm = ScriptedLlm { answers: vec![answer("S9"), answer("S2")], calls: AtomicUsize::new(0) };
        let a = request_validated(&llm, "u", &index()).unwrap();
        assert_eq!(llm.calls.load(Ordering::SeqCst), 2);
        assert_eq!(a.action_items.len(), 1);
    }

    #[test]
    fn malformed_twice_is_unavailable_not_guessed() {
        let llm = ScriptedLlm { answers: vec!["not json".into(), "{\"title\": 1}".into()], calls: AtomicUsize::new(0) };
        assert!(matches!(request_validated(&llm, "u", &index()), Err(AnalysisError::Unavailable(_))));
        assert_eq!(llm.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn benchmark_accepts_corrected_answers_like_real_meetings() {
        let llm = ScriptedLlm {
            answers: vec![answer("S9"), answer("S2"), answer("S9"), answer("S2")],
            calls: AtomicUsize::new(0),
        };
        let mut result = LlmBenchmark::default();
        benchmark_requests(&llm, "u", &index(), &mut result);
        assert!(result.stable && result.structured_output_valid);
        assert!(result.error.is_none());
        assert_eq!(llm.calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn benchmark_reports_unusable_output() {
        let llm = ScriptedLlm { answers: vec!["not json".into(); 2], calls: AtomicUsize::new(0) };
        let mut result = LlmBenchmark::default();
        benchmark_requests(&llm, "u", &index(), &mut result);
        assert!(!result.structured_output_valid);
        assert!(result.error.is_some());
    }

    #[test]
    fn empty_summary_after_retry_is_unavailable() {
        let mut empty: serde_json::Value = serde_json::from_str(&answer("S2")).unwrap();
        empty["summary"] = serde_json::json!([]);
        let llm = ScriptedLlm { answers: vec![empty.to_string(); 2], calls: AtomicUsize::new(0) };
        assert!(request_validated(&llm, "u", &index()).is_err());
    }

    #[test]
    fn stores_and_loads_with_evidence_and_owner() {
        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (mid, _, _) = super::super::store::create_meeting(&db, "Meeting 1 Jan", root.path(), true, true, "keep_forever").unwrap();
        let tom = people::create(&db, &people::PersonInput { display_name: "Tom".into(), email: None, is_self: None }).unwrap();
        let s1 = super::super::transcript::insert_segment(&db, &super::super::transcript::NewSegment {
            meeting_id: &mid, source: super::super::transcript::SegmentSource::System, start_ms: 1000, end_ms: 2000,
            text: "Yes, I'll do it by Friday.", words: &[], person_id: Some(&tom.id), speaker_confidence: Some(1.0),
        }).unwrap();
        let settings = AppSettings::default();
        let t = transcript_for_model(&db, &settings, &mid).unwrap();
        assert_eq!(t.lines[0].speaker, "Tom");
        assert_eq!(t.label_to_person.get("Tom"), Some(&tom.id));

        let llm = ScriptedLlm { answers: vec![answer("S1")], calls: AtomicUsize::new(0) };
        let (mut a, strategy) = run_analysis(&llm, &t, &|_| {}).unwrap();
        assert_eq!(strategy, "single_pass");
        for x in &mut a.action_items {
            x.owner_person_id = x.owner_label.as_ref().and_then(|l| t.label_to_person.get(l).cloned());
        }
        store_analysis(&db, &mid, &a, "scripted", 8192, strategy, t.revision).unwrap();
        let v = load_view(&db, &settings, &mid).unwrap().unwrap();
        assert_eq!(v.status, AnalysisStatus::Ready);
        assert!(!v.stale);
        assert_eq!(v.action_items[0].owner_person_id.as_deref(), Some(tom.id.as_str()));
        assert_eq!(v.action_items[0].evidence[0].segment_id, s1);
        assert_eq!(v.action_items[0].evidence[0].speaker, "Tom");
        assert!(v.action_items[0].selected, "strong + confident items are pre-selected");
        // The model's title replaces the default title.
        let title: String = db.with(|c| c.query_row("SELECT title FROM meetings WHERE id = ?1", [&mid], |r| r.get(0))).unwrap();
        assert_eq!(title, "Sync");

        // Editing the transcript marks the analysis stale.
        super::super::speakers::edit_segment_text(&db, &s1, "Yes, I'll do it by Thursday.").unwrap();
        assert!(load_view(&db, &settings, &mid).unwrap().unwrap().stale);

        // Items already created in Linear survive regeneration.
        let item = &v.action_items[0].id;
        db.with(|c| c.execute("UPDATE action_items SET linear_sync_state = 'created', linear_issue_id = 'L1' WHERE id = ?1", [item])).unwrap();
        store_analysis(&db, &mid, &a, "scripted", 8192, strategy, 2).unwrap();
        let v2 = load_view(&db, &settings, &mid).unwrap().unwrap();
        assert!(v2.action_items.iter().any(|x| x.linear.issue_id.as_deref() == Some("L1")));
    }

    #[test]
    fn long_meetings_use_chunked_synthesis() {
        let lines = prompts::build_lines((0..600).map(|i| (format!("id{i}"), i as u64 * 4000, "Tom".to_string(), "we talked about many things today and ".repeat(8))));
        let t = TranscriptForModel { lines, meeting_date: None, label_to_person: HashMap::new(), revision: 0, title: "t".into(), date: "d".into() };
        // Every call returns the same valid answer citing S1 (present in chunk 1 and full index).
        let llm = ScriptedLlm { answers: (0..50).map(|_| answer("S1")).collect(), calls: AtomicUsize::new(0) };
        let result = run_analysis(&llm, &t, &|_| {});
        match result {
            Ok((_, strategy)) => assert_eq!(strategy, "chunked"),
            Err(AnalysisError::Unavailable(m)) => assert!(m.contains("too long")),
            Err(e) => panic!("{e:?}"),
        }
        assert!(llm.calls.load(Ordering::SeqCst) > 1);
    }
}

#[cfg(test)]
mod real_llm_tests {
    use super::*;
    use crate::storage::settings::InferenceBackendPreference;

    /// Real llama-server + installed 4B model on the benchmark transcript.
    /// MINUTES_DATA_DIR=... cargo test --release real_llm_analysis -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_llm_analysis() {
        let data = PathBuf::from(std::env::var("MINUTES_DATA_DIR").unwrap());
        let db = Database::open(&data.join("minutes.db")).unwrap();
        let models = ModelManager::new(db.clone(), data.join("models"));
        let path = model_manager::model_path(&db, &models, crate::models::catalog::LLM_SMALL_ID).expect("4B installed");
        let dir = tempfile::tempdir().unwrap();
        let t0 = Instant::now();
        let server = LlamaServer::start(LlamaConfig {
            runtime_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/llama"),
            model_id: "4b".into(),
            model_path: path,
            context_tokens: 8192,
            gpu_layers: None,
            backend: InferenceBackendPreference::Auto,
            threads: None,
            log_path: dir.path().join("l.log"),
            pid_path: dir.path().join("p.pid"),
            load_timeout: Duration::from_secs(120),
            request_timeout: Duration::from_secs(300),
        })
        .unwrap();
        println!("loaded in {:?}", t0.elapsed());
        let lines = prompts::build_lines(
            BENCH_TRANSCRIPT.iter().enumerate().map(|(i, (s, t))| (format!("b{i}"), i as u64 * 6000, s.to_string(), t.to_string())),
        );
        let index = with_date(prompts::index(&lines), chrono::NaiveDate::from_ymd_opt(2026, 9, 25));
        let user = prompts::user_prompt("Onboarding sync", "Friday 25 September 2026", &lines, None);
        let t1 = Instant::now();
        let a = request_validated(&server, &user, &index).unwrap();
        println!("analysed in {:?}\n{}", t1.elapsed(), serde_json::to_string_pretty(&a).unwrap());
        assert!(!a.summary.is_empty());
        assert!(!a.action_items.is_empty());
        let tom = a.action_items.iter().find(|x| x.owner_label.as_deref() == Some("Tom") && x.title.to_lowercase().contains("api"));
        assert!(tom.is_some(), "Tom's API action should be found");
        // "Maybe Tom can also look at the analytics…" must not be a strong assignment.
        for x in a.action_items.iter().filter(|x| x.title.to_lowercase().contains("analytics")) {
            assert!(!x.assignment_type.is_strong(), "suggestion treated as assignment: {x:?}");
        }
        drop(server);
        assert!(!dir.path().join("p.pid").exists());
    }
}
