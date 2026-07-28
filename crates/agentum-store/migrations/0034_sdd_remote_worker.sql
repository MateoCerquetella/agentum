-- Durable host-side state for the fixed `agentum-sdd-v1` SSH subsystem.
-- The worker is a separate process per SSH channel, so all idempotency,
-- sequencing, recovery, and its global concurrency-one lease live in SQLite.

CREATE TABLE IF NOT EXISTS sdd_remote_worker_runs (
    run_id TEXT PRIMARY KEY,
    host_id TEXT NOT NULL,
    repository_identity_sha256 TEXT NOT NULL,
    artifact_set_id TEXT NOT NULL,
    spec_id TEXT NOT NULL,
    spec_revision INTEGER NOT NULL,
    base_commit TEXT NOT NULL,
    provider TEXT NOT NULL,
    authoritative_path TEXT NOT NULL,
    branch_name TEXT NOT NULL,
    approval_digest TEXT,
    next_phase TEXT NOT NULL,
    completed_phases INTEGER NOT NULL DEFAULT 0,
    workspace_state_sha256 TEXT NOT NULL,
    last_result_sha256 TEXT NOT NULL,
    status TEXT NOT NULL,
    blocker TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sdd_remote_worker_requests (
    request_id TEXT PRIMARY KEY,
    request_sha256 TEXT NOT NULL,
    run_id TEXT NOT NULL REFERENCES sdd_remote_worker_runs(run_id) ON DELETE CASCADE,
    operation TEXT NOT NULL,
    phase TEXT,
    stage TEXT NOT NULL,
    attempt_path TEXT,
    response_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sdd_remote_worker_requests_run
    ON sdd_remote_worker_requests(run_id, created_at, request_id);

CREATE TABLE IF NOT EXISTS sdd_remote_worker_patch_journal (
    patch_id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL REFERENCES sdd_remote_worker_requests(request_id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES sdd_remote_worker_runs(run_id) ON DELETE CASCADE,
    operations_json TEXT NOT NULL,
    preimages_json TEXT NOT NULL,
    status TEXT NOT NULL,
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sdd_remote_worker_lease (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    owner_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    acquired_at TEXT NOT NULL
);
