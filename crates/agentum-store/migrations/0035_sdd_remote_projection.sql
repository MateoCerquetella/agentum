-- Desktop-side authoritative projection for the fixed agentum-sdd-v1 SSH
-- subsystem. Worker state is deliberately not trusted as desktop history:
-- every accepted checkpoint is bound to the ordinary SDD aggregate CAS.

CREATE TABLE sdd_remote_runs (
    run_id TEXT PRIMARY KEY REFERENCES sdd_runs(run_id) ON DELETE CASCADE,
    host_id TEXT NOT NULL,
    repository_identity_sha256 TEXT NOT NULL CHECK(length(repository_identity_sha256) = 64),
    artifact_set_id TEXT NOT NULL,
    worker_version TEXT NOT NULL,
    plan_json TEXT NOT NULL,
    checkpoint_json TEXT NOT NULL,
    checkpoint_revision INTEGER NOT NULL DEFAULT 1,
    active_request_id TEXT,
    status TEXT NOT NULL CHECK(status IN (
        'waiting', 'queued', 'running', 'paused', 'blocked', 'canceled', 'failed', 'succeeded'
    )),
    last_error_code TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_sdd_remote_runs_host ON sdd_remote_runs(host_id, updated_at DESC);

CREATE TABLE sdd_remote_requests (
    request_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES sdd_remote_runs(run_id) ON DELETE CASCADE,
    phase TEXT NOT NULL CHECK(phase IN (
        'design', 'planning', 'implementation', 'verification', 'review'
    )),
    request_json TEXT NOT NULL,
    request_sha256 TEXT NOT NULL CHECK(length(request_sha256) = 64),
    expected_run_revision INTEGER NOT NULL,
    attempt_id TEXT NOT NULL REFERENCES sdd_attempts(attempt_id),
    status TEXT NOT NULL CHECK(status IN (
        'running', 'cancel_requested', 'succeeded', 'failed', 'canceled', 'interrupted'
    )),
    response_json TEXT,
    error_code TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_sdd_remote_requests_run ON sdd_remote_requests(run_id, created_at);

CREATE TABLE sdd_remote_artifact_payloads (
    artifact_revision_id TEXT PRIMARY KEY
        REFERENCES sdd_artifact_revisions(artifact_revision_id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES sdd_remote_runs(run_id) ON DELETE CASCADE,
    request_id TEXT,
    content TEXT NOT NULL,
    content_sha256 TEXT NOT NULL CHECK(length(content_sha256) = 64),
    created_at TEXT NOT NULL,
    FOREIGN KEY(request_id) REFERENCES sdd_remote_requests(request_id)
);
CREATE INDEX idx_sdd_remote_payloads_run ON sdd_remote_artifact_payloads(run_id, created_at);

CREATE TABLE sdd_remote_evidence (
    request_id TEXT PRIMARY KEY REFERENCES sdd_remote_requests(request_id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES sdd_remote_runs(run_id) ON DELETE CASCADE,
    phase TEXT NOT NULL,
    evidence_sha256 TEXT NOT NULL CHECK(length(evidence_sha256) = 64),
    summary TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE sdd_remote_task_completions (
    run_id TEXT NOT NULL REFERENCES sdd_remote_runs(run_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL,
    request_id TEXT NOT NULL REFERENCES sdd_remote_requests(request_id) ON DELETE CASCADE,
    patch_sha256 TEXT NOT NULL CHECK(length(patch_sha256) = 64),
    write_set_sha256 TEXT NOT NULL CHECK(length(write_set_sha256) = 64),
    created_at TEXT NOT NULL,
    PRIMARY KEY(run_id, task_id),
    FOREIGN KEY(run_id, task_id) REFERENCES sdd_tasks(run_id, task_id)
);

-- The typed author request and publication intent are committed before the
-- first SSH side effect. A desktop restart can replay the same worker request
-- id and finish the ordinary aggregate transaction without a local-path
-- fallback or an orphaned remote worktree.
CREATE TABLE sdd_remote_create_intents (
    repo_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    author_request_json TEXT NOT NULL,
    publication_intent_json TEXT NOT NULL,
    author_result_json TEXT,
    status TEXT NOT NULL CHECK(status IN ('prepared', 'authored', 'completed', 'failed')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(repo_id, request_id),
    FOREIGN KEY(repo_id, request_id) REFERENCES sdd_create_sagas(repo_id, request_id)
);
