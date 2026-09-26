-- Initial schema. Timestamps are RFC 3339 UTC strings; durations are ms.
-- Every relationship uses a foreign key. JSON columns hold only secondary
-- structures (word timings, raw validated model output, benchmark detail).

CREATE TABLE settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,             -- JSON-encoded value
    updated_at  TEXT NOT NULL
);

CREATE TABLE people (
    id            TEXT PRIMARY KEY,
    display_name  TEXT NOT NULL,
    email         TEXT,
    avatar_path   TEXT,
    is_self       INTEGER NOT NULL DEFAULT 0 CHECK (is_self IN (0, 1)),
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE UNIQUE INDEX people_single_self ON people(is_self) WHERE is_self = 1;

CREATE TABLE speaker_profiles (
    id               TEXT PRIMARY KEY,
    person_id        TEXT NOT NULL UNIQUE REFERENCES people(id) ON DELETE CASCADE,
    embedding_model  TEXT NOT NULL,        -- embeddings are only comparable within one model
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);

CREATE TABLE meetings (
    id                    TEXT PRIMARY KEY,
    title                 TEXT NOT NULL,
    status                TEXT NOT NULL CHECK (status IN ('recording', 'processing', 'ready', 'failed', 'interrupted')),
    started_at            TEXT NOT NULL,
    ended_at              TEXT,
    duration_ms           INTEGER,
    capture_microphone    INTEGER NOT NULL DEFAULT 1 CHECK (capture_microphone IN (0, 1)),
    capture_system        INTEGER NOT NULL DEFAULT 1 CHECK (capture_system IN (0, 1)),
    audio_dir             TEXT,
    audio_retention       TEXT NOT NULL DEFAULT 'delete_after_processing'
                          CHECK (audio_retention IN ('delete_after_processing', 'keep_7_days', 'keep_forever')),
    audio_deleted_at      TEXT,
    transcript_revision   INTEGER NOT NULL DEFAULT 0,  -- bumped on every transcript edit
    processing_error      TEXT,                         -- user-safe message when processing failed
    notion_page_id        TEXT,
    notion_page_url       TEXT,
    notion_synced_at      TEXT,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL
);
CREATE INDEX meetings_started ON meetings(started_at DESC);

-- One row per captured stream. Original capture format is preserved until
-- processing finishes (spec: keep streams separate, do not pre-mix).
CREATE TABLE audio_tracks (
    id               TEXT PRIMARY KEY,
    meeting_id       TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    source           TEXT NOT NULL CHECK (source IN ('microphone', 'system')),
    segment_index    INTEGER NOT NULL DEFAULT 0,   -- >0 after a device reconnect
    start_offset_ms  INTEGER NOT NULL DEFAULT 0,   -- from meeting start
    device_id        TEXT,
    device_name      TEXT,
    path             TEXT NOT NULL,
    sample_rate      INTEGER NOT NULL,
    channels         INTEGER NOT NULL,
    frames_written   INTEGER NOT NULL DEFAULT 0,
    status           TEXT NOT NULL CHECK (status IN ('recording', 'complete', 'failed', 'disconnected')),
    error            TEXT,
    updated_at       TEXT NOT NULL,
    UNIQUE (meeting_id, source, segment_index)
);

-- Diarization clusters are per meeting. Their identity can change after
-- transcription (refinement pass, user confirmation).
CREATE TABLE speaker_clusters (
    id                   TEXT PRIMARY KEY,
    meeting_id           TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    label                TEXT NOT NULL,             -- "Speaker 1" or a user rename
    source               TEXT NOT NULL CHECK (source IN ('microphone', 'system', 'mixed')),
    person_id            TEXT REFERENCES people(id) ON DELETE SET NULL,
    identity_type        TEXT NOT NULL DEFAULT 'unknown'
                         CHECK (identity_type IN ('known', 'possible', 'unknown', 'confirmed', 'local_user')),
    identity_confidence  REAL,
    merged_into          TEXT REFERENCES speaker_clusters(id) ON DELETE SET NULL,
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL
);
CREATE INDEX speaker_clusters_meeting ON speaker_clusters(meeting_id);

CREATE TABLE transcript_segments (
    id                        TEXT PRIMARY KEY,
    meeting_id                TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    start_ms                  INTEGER NOT NULL,
    end_ms                    INTEGER NOT NULL CHECK (end_ms >= start_ms),
    source                    TEXT NOT NULL CHECK (source IN ('microphone', 'system', 'mixed')),
    speaker_cluster_id        TEXT REFERENCES speaker_clusters(id) ON DELETE SET NULL,
    person_id                 TEXT REFERENCES people(id) ON DELETE SET NULL,
    speaker_confidence        REAL,
    text                      TEXT NOT NULL,
    original_text             TEXT,                   -- ASR output before user correction
    transcription_confidence  REAL,
    words_json                TEXT,                   -- [{text,startMs,endMs}] secondary structure
    is_provisional            INTEGER NOT NULL DEFAULT 1 CHECK (is_provisional IN (0, 1)),
    edited_at                 TEXT,
    created_at                TEXT NOT NULL
);
CREATE INDEX transcript_segments_meeting_time ON transcript_segments(meeting_id, start_ms);

CREATE TABLE speaker_embeddings (
    id           TEXT PRIMARY KEY,
    profile_id   TEXT NOT NULL REFERENCES speaker_profiles(id) ON DELETE CASCADE,
    source       TEXT NOT NULL CHECK (source IN ('enrollment', 'confirmed_meeting')),
    meeting_id   TEXT REFERENCES meetings(id) ON DELETE SET NULL,
    dimensions   INTEGER NOT NULL,
    ciphertext   BLOB NOT NULL,     -- AES-256-GCM, key held in the OS credential store
    nonce        BLOB NOT NULL,
    quality      REAL,
    created_at   TEXT NOT NULL
);
CREATE INDEX speaker_embeddings_profile ON speaker_embeddings(profile_id);

CREATE TABLE meeting_participants (
    meeting_id  TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    person_id   TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    PRIMARY KEY (meeting_id, person_id)
);

CREATE TABLE meeting_analysis (
    id                   TEXT PRIMARY KEY,
    meeting_id           TEXT NOT NULL UNIQUE REFERENCES meetings(id) ON DELETE CASCADE,
    status               TEXT NOT NULL CHECK (status IN ('pending', 'running', 'ready', 'unavailable', 'failed')),
    title                TEXT,
    summary_json         TEXT,          -- string[]
    model_id             TEXT,
    context_tokens       INTEGER,
    strategy             TEXT CHECK (strategy IN ('single_pass', 'chunked')),
    transcript_revision  INTEGER,       -- which transcript revision this analysis used
    error                TEXT,          -- user-safe message
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL
);

CREATE TABLE decisions (
    id           TEXT PRIMARY KEY,
    analysis_id  TEXT NOT NULL REFERENCES meeting_analysis(id) ON DELETE CASCADE,
    position     INTEGER NOT NULL,
    text         TEXT NOT NULL
);

CREATE TABLE decision_evidence (
    decision_id  TEXT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    segment_id   TEXT NOT NULL REFERENCES transcript_segments(id) ON DELETE CASCADE,
    PRIMARY KEY (decision_id, segment_id)
);

CREATE TABLE unresolved_questions (
    id           TEXT PRIMARY KEY,
    analysis_id  TEXT NOT NULL REFERENCES meeting_analysis(id) ON DELETE CASCADE,
    position     INTEGER NOT NULL,
    text         TEXT NOT NULL
);

CREATE TABLE question_evidence (
    question_id  TEXT NOT NULL REFERENCES unresolved_questions(id) ON DELETE CASCADE,
    segment_id   TEXT NOT NULL REFERENCES transcript_segments(id) ON DELETE CASCADE,
    PRIMARY KEY (question_id, segment_id)
);

CREATE TABLE action_items (
    id                        TEXT PRIMARY KEY,
    meeting_id                TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    analysis_id               TEXT REFERENCES meeting_analysis(id) ON DELETE SET NULL,
    position                  INTEGER NOT NULL,
    title                     TEXT NOT NULL,
    description               TEXT,
    owner_person_id           TEXT REFERENCES people(id) ON DELETE SET NULL,
    due_date                  TEXT,
    assignment_type           TEXT NOT NULL CHECK (assignment_type IN
                               ('explicit_acceptance', 'explicit_assignment', 'suggested', 'unclear')),
    confidence                REAL NOT NULL,
    selected                  INTEGER NOT NULL DEFAULT 0 CHECK (selected IN (0, 1)),
    user_edited               INTEGER NOT NULL DEFAULT 0 CHECK (user_edited IN (0, 1)),
    dismissed                 INTEGER NOT NULL DEFAULT 0 CHECK (dismissed IN (0, 1)),
    -- Linear sync. The idempotency key is a client-generated UUID passed as
    -- IssueCreateInput.id so a retry can never create a second issue.
    linear_idempotency_key    TEXT UNIQUE,
    linear_sync_state         TEXT NOT NULL DEFAULT 'none'
                              CHECK (linear_sync_state IN ('none', 'pending', 'created', 'failed', 'unknown')),
    linear_issue_id           TEXT,
    linear_issue_identifier   TEXT,
    linear_issue_url          TEXT,
    linear_team_id            TEXT,
    linear_project_id         TEXT,
    linear_assignee_id        TEXT,
    linear_priority           INTEGER CHECK (linear_priority BETWEEN 0 AND 4),
    linear_error              TEXT,
    created_at                TEXT NOT NULL,
    updated_at                TEXT NOT NULL
);
CREATE INDEX action_items_meeting ON action_items(meeting_id);

CREATE TABLE action_item_evidence (
    action_item_id  TEXT NOT NULL REFERENCES action_items(id) ON DELETE CASCADE,
    segment_id      TEXT NOT NULL REFERENCES transcript_segments(id) ON DELETE CASCADE,
    PRIMARY KEY (action_item_id, segment_id)
);

-- Deterministic mappings from local entities to remote IDs.
CREATE TABLE integration_mappings (
    id           TEXT PRIMARY KEY,
    provider     TEXT NOT NULL CHECK (provider IN ('notion', 'linear')),
    entity_type  TEXT NOT NULL CHECK (entity_type IN ('person', 'team', 'project', 'data_source', 'default_team', 'default_project')),
    local_id     TEXT NOT NULL,     -- person id, or a fixed key for defaults
    remote_id    TEXT NOT NULL,
    remote_name  TEXT,
    updated_at   TEXT NOT NULL,
    UNIQUE (provider, entity_type, local_id)
);

-- Audit trail for every outbound integration write.
CREATE TABLE integration_operations (
    id               TEXT PRIMARY KEY,
    provider         TEXT NOT NULL CHECK (provider IN ('notion', 'linear')),
    kind             TEXT NOT NULL,
    meeting_id       TEXT REFERENCES meetings(id) ON DELETE CASCADE,
    action_item_id   TEXT REFERENCES action_items(id) ON DELETE CASCADE,
    idempotency_key  TEXT,
    status           TEXT NOT NULL CHECK (status IN ('started', 'succeeded', 'failed', 'unknown')),
    remote_id        TEXT,
    error            TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);

CREATE TABLE model_installations (
    model_id          TEXT PRIMARY KEY,
    version           TEXT NOT NULL,
    purpose           TEXT NOT NULL CHECK (purpose IN ('asr', 'vad', 'diarization', 'embedding', 'llm')),
    path              TEXT NOT NULL,
    byte_size         INTEGER NOT NULL,
    bytes_downloaded  INTEGER NOT NULL DEFAULT 0,
    status            TEXT NOT NULL CHECK (status IN ('downloading', 'paused', 'verifying', 'installed', 'failed')),
    source            TEXT NOT NULL CHECK (source IN ('catalog', 'custom_import')),
    error             TEXT,
    installed_at      TEXT,
    updated_at        TEXT NOT NULL
);

CREATE TABLE benchmark_results (
    id                    TEXT PRIMARY KEY,
    kind                  TEXT NOT NULL CHECK (kind IN ('synthetic', 'asr', 'llm')),
    model_id              TEXT,
    hardware_fingerprint  TEXT NOT NULL,   -- invalidates results when hardware changes
    passed                INTEGER NOT NULL CHECK (passed IN (0, 1)),
    result_json           TEXT NOT NULL,
    created_at            TEXT NOT NULL
);
CREATE INDEX benchmark_results_kind ON benchmark_results(kind, created_at DESC);
