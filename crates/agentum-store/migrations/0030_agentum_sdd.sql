-- Agentum-native SDD. Repository artifacts are portable intent; all mutable
-- orchestration state and audit history live in these normalized tables.

CREATE TABLE sdd_specs (
    spec_id TEXT PRIMARY KEY,
    spec_ulid TEXT NOT NULL UNIQUE,
    repo_id TEXT NOT NULL,
    title TEXT NOT NULL,
    slug TEXT NOT NULL,
    profile TEXT NOT NULL CHECK(profile IN ('standard', 'high_risk')),
    control TEXT NOT NULL CHECK(control IN ('guarded', 'interactive', 'autopilot')),
    provider TEXT NOT NULL,
    source_ref_json TEXT,
    current_revision INTEGER NOT NULL,
    aggregate_revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_sdd_specs_repo ON sdd_specs(repo_id, updated_at DESC);

-- A repository has one immutable artifact-set identity. Concurrent spec
-- creation races on this row and then shares the winning identity, preventing
-- independent worktrees from publishing conflicting manifests.
CREATE TABLE sdd_repo_artifact_sets (
    repo_id TEXT PRIMARY KEY,
    artifact_set_id TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL
);

CREATE TABLE sdd_spec_revisions (
    spec_id TEXT NOT NULL REFERENCES sdd_specs(spec_id),
    revision INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    content TEXT NOT NULL,
    submitted_by TEXT NOT NULL,
    imported_external INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    PRIMARY KEY (spec_id, revision)
);

CREATE TABLE sdd_runs (
    run_id TEXT PRIMARY KEY,
    spec_id TEXT NOT NULL REFERENCES sdd_specs(spec_id),
    repo_id TEXT NOT NULL,
    phase TEXT NOT NULL CHECK(phase IN (
        'specification', 'design', 'planning', 'implementation', 'verification',
        'review', 'ready', 'delivery', 'completed'
    )),
    status TEXT NOT NULL CHECK(status IN (
        'idle', 'queued', 'running', 'waiting', 'retry_scheduled', 'pausing',
        'paused', 'blocked', 'canceling', 'canceled', 'failed', 'succeeded'
    )),
    aggregate_revision INTEGER NOT NULL DEFAULT 1,
    base_ref TEXT NOT NULL,
    base_commit TEXT NOT NULL,
    branch_name TEXT NOT NULL,
    authoritative_path TEXT NOT NULL,
    workspace_fingerprint TEXT NOT NULL,
    policy_json TEXT NOT NULL,
    blocker TEXT,
    quarantined INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_sdd_runs_spec ON sdd_runs(spec_id, created_at DESC);
CREATE INDEX idx_sdd_runs_repo ON sdd_runs(repo_id, updated_at DESC);

CREATE TABLE sdd_artifact_revisions (
    artifact_revision_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES sdd_runs(run_id),
    spec_id TEXT NOT NULL REFERENCES sdd_specs(spec_id),
    kind TEXT NOT NULL CHECK(kind IN ('specification', 'design', 'plan', 'decisions', 'review')),
    revision INTEGER NOT NULL,
    spec_revision INTEGER NOT NULL,
    relative_path TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    submitted_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(run_id, kind, revision)
);
CREATE INDEX idx_sdd_artifacts_run ON sdd_artifact_revisions(run_id, kind, revision DESC);

CREATE TABLE sdd_tasks (
    run_id TEXT NOT NULL REFERENCES sdd_runs(run_id),
    task_id TEXT NOT NULL,
    spec_revision INTEGER NOT NULL,
    intent_json TEXT NOT NULL,
    runtime_status TEXT NOT NULL CHECK(runtime_status IN (
        'idle', 'queued', 'running', 'waiting', 'retry_scheduled', 'pausing',
        'paused', 'blocked', 'canceling', 'canceled', 'failed', 'succeeded'
    )),
    aggregate_revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(run_id, task_id)
);

CREATE TABLE sdd_attempts (
    attempt_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES sdd_runs(run_id),
    task_id TEXT,
    spec_revision INTEGER NOT NULL,
    provider TEXT NOT NULL,
    isolated_path TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN (
        'idle', 'queued', 'running', 'waiting', 'retry_scheduled', 'pausing',
        'paused', 'blocked', 'canceling', 'canceled', 'failed', 'succeeded'
    )),
    session_identity TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    error_summary TEXT
);

CREATE TABLE sdd_capability_grants (
    grant_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES sdd_runs(run_id),
    attempt_id TEXT NOT NULL REFERENCES sdd_attempts(attempt_id),
    capability TEXT NOT NULL,
    scope_json TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TEXT NOT NULL,
    revoked_at TEXT
);

CREATE TABLE sdd_leases (
    run_id TEXT NOT NULL REFERENCES sdd_runs(run_id),
    relative_path TEXT NOT NULL,
    attempt_id TEXT NOT NULL REFERENCES sdd_attempts(attempt_id),
    preimage_hash TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    PRIMARY KEY(run_id, relative_path)
);

CREATE TABLE sdd_patch_ledger (
    patch_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES sdd_runs(run_id),
    attempt_id TEXT NOT NULL REFERENCES sdd_attempts(attempt_id),
    operations_json TEXT NOT NULL,
    preimages_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending', 'applied', 'rolled_back', 'quarantined', 'failed')),
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE sdd_verification_results (
    verification_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES sdd_runs(run_id),
    attempt_id TEXT NOT NULL REFERENCES sdd_attempts(attempt_id),
    task_id TEXT,
    spec_revision INTEGER NOT NULL,
    command_index INTEGER NOT NULL,
    command_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('succeeded', 'failed', 'timed_out', 'canceled')),
    exit_code INTEGER,
    output_hash TEXT NOT NULL,
    output_excerpt TEXT NOT NULL,
    duration_ms INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(run_id, attempt_id, command_index)
);
CREATE INDEX idx_sdd_verification_run
    ON sdd_verification_results(run_id, created_at);

CREATE TABLE sdd_approval_requests (
    approval_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES sdd_runs(run_id),
    purpose TEXT NOT NULL,
    digest TEXT NOT NULL,
    requested_revision INTEGER NOT NULL,
    requested_by TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending', 'approved', 'rejected', 'invalidated')),
    invalidated_at TEXT,
    created_at TEXT NOT NULL,
    decided_at TEXT
);
CREATE UNIQUE INDEX idx_sdd_one_live_approval
    ON sdd_approval_requests(run_id, purpose)
    WHERE status = 'pending';

CREATE TABLE sdd_approval_decisions (
    decision_id TEXT PRIMARY KEY,
    approval_id TEXT NOT NULL REFERENCES sdd_approval_requests(approval_id),
    digest TEXT NOT NULL,
    actor_type TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    decision TEXT NOT NULL,
    reason TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(approval_id)
);

CREATE TABLE sdd_external_links (
    link_id TEXT PRIMARY KEY,
    spec_id TEXT NOT NULL REFERENCES sdd_specs(spec_id),
    provider TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    site_id TEXT,
    external_id TEXT NOT NULL,
    key TEXT,
    url TEXT NOT NULL,
    source_revision TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(spec_id, provider, connection_id, external_id)
);

CREATE TABLE sdd_import_jobs (
    import_id TEXT PRIMARY KEY,
    repo_id TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    preview_json TEXT NOT NULL,
    disposition TEXT NOT NULL,
    created_at TEXT NOT NULL,
    committed_at TEXT,
    UNIQUE(repo_id, source_kind, source_hash)
);

CREATE TABLE sdd_delivery_previews (
    preview_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES sdd_runs(run_id),
    actor_id TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    digest TEXT NOT NULL,
    run_revision INTEGER NOT NULL,
    spec_revision INTEGER NOT NULL,
    actions_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending', 'confirmed', 'expired', 'invalidated')),
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    confirmed_at TEXT
);
CREATE INDEX idx_sdd_delivery_preview_run
    ON sdd_delivery_previews(run_id, created_at DESC);

CREATE TABLE sdd_delivery_actions (
    preview_id TEXT NOT NULL REFERENCES sdd_delivery_previews(preview_id),
    action_id TEXT NOT NULL,
    action_type TEXT NOT NULL,
    intent_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending', 'running', 'succeeded', 'failed', 'sync_pending')),
    result_json TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(preview_id, action_id)
);

CREATE TABLE sdd_events (
    cursor INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    repo_id TEXT NOT NULL,
    spec_id TEXT,
    run_id TEXT,
    aggregate_revision INTEGER NOT NULL,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_sdd_events_repo_cursor ON sdd_events(repo_id, cursor);
CREATE INDEX idx_sdd_events_run_cursor ON sdd_events(run_id, cursor);

CREATE TABLE sdd_outbox (
    outbox_id TEXT PRIMARY KEY,
    event_cursor INTEGER NOT NULL REFERENCES sdd_events(cursor),
    destination TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    available_at TEXT NOT NULL,
    delivered_at TEXT,
    last_error TEXT
);
CREATE INDEX idx_sdd_outbox_ready ON sdd_outbox(delivered_at, available_at);

-- Every mutating request stores its exact response in the same transaction as
-- state. A retried request is a read, never a second side effect.
CREATE TABLE sdd_idempotency (
    scope TEXT NOT NULL,
    request_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    expected_revision INTEGER NOT NULL,
    response_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(scope, request_id)
);

-- Creation is a filesystem/network saga.  This reservation is committed
-- before Git worktree creation or provider execution so an interrupted create
-- remains visible and recoverable instead of becoming an orphan worktree.
CREATE TABLE sdd_create_sagas (
    repo_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    spec_id TEXT NOT NULL UNIQUE,
    run_id TEXT NOT NULL UNIQUE,
    stage TEXT NOT NULL CHECK(stage IN (
        'reserved', 'workspace_ready', 'authoring', 'publishing',
        'completed', 'failed', 'canceled', 'recovery_required'
    )),
    repository_path TEXT NOT NULL,
    authoritative_path TEXT NOT NULL,
    branch_name TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    attempt_path TEXT NOT NULL,
    error_summary TEXT,
    response_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(repo_id, request_id)
);
CREATE INDEX idx_sdd_create_sagas_recovery
    ON sdd_create_sagas(stage, updated_at);
