-- Durable state for shared-worktree harness orchestration.
CREATE TABLE harness_orchestrated_runs (
    run_id TEXT PRIMARY KEY, workdir TEXT NOT NULL, plan_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'planning', coordinator_session TEXT,
    coordinator_token TEXT NOT NULL, max_concurrency INTEGER NOT NULL DEFAULT 4,
    final_gate_runs INTEGER NOT NULL DEFAULT 0, checkpoint_json TEXT,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);

CREATE TABLE harness_orchestrated_tasks (
    run_id TEXT NOT NULL, task_id TEXT NOT NULL, external_task_id TEXT,
    status TEXT NOT NULL DEFAULT 'pending', packet_json TEXT NOT NULL,
    deps_json TEXT NOT NULL DEFAULT '[]', writable_json TEXT NOT NULL DEFAULT '[]',
    create_dirs_json TEXT NOT NULL DEFAULT '[]', worker_session TEXT,
    worker_token TEXT NOT NULL, enforcement TEXT NOT NULL DEFAULT 'best_effort',
    context_remaining INTEGER, result_json TEXT, error_tail TEXT,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
    PRIMARY KEY (run_id, task_id)
);
CREATE INDEX idx_harness_tasks_run_status ON harness_orchestrated_tasks(run_id, status);

CREATE TABLE harness_file_leases (
    run_id TEXT NOT NULL, path TEXT NOT NULL, task_id TEXT NOT NULL,
    content_hash TEXT NOT NULL, frozen INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (run_id, path)
);

CREATE TABLE harness_patch_ledger (
    patch_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, task_id TEXT NOT NULL,
    summary TEXT NOT NULL, operations_json TEXT NOT NULL, preimages_json TEXT NOT NULL,
    status TEXT NOT NULL, error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE INDEX idx_harness_patches_run ON harness_patch_ledger(run_id, created_at);

CREATE TABLE harness_managed_sessions (
    session_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, task_id TEXT,
    role TEXT NOT NULL, capability_scope TEXT NOT NULL, context_remaining INTEGER,
    replaced_by TEXT, active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE INDEX idx_harness_managed_run ON harness_managed_sessions(run_id, active);

CREATE TABLE harness_coordinator_decisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT, run_id TEXT NOT NULL,
    decision TEXT NOT NULL, payload_json TEXT, created_at TEXT NOT NULL
);
CREATE INDEX idx_harness_decisions_run ON harness_coordinator_decisions(run_id, id);
