-- Starting an already-discovered specification is a filesystem/Git saga of
-- its own. It must not share the create-spec reservation namespace because
-- request IDs are scoped to different public endpoints.
CREATE TABLE sdd_run_create_sagas (
    spec_id TEXT NOT NULL REFERENCES sdd_specs(spec_id),
    repo_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    run_id TEXT NOT NULL UNIQUE,
    stage TEXT NOT NULL CHECK(stage IN (
        'reserved', 'workspace_ready', 'publishing', 'completed', 'failed',
        'canceled', 'recovery_required'
    )),
    expected_spec_revision INTEGER NOT NULL,
    expected_spec_hash TEXT NOT NULL,
    expected_aggregate_revision INTEGER NOT NULL,
    repository_path TEXT NOT NULL,
    authoritative_path TEXT NOT NULL,
    branch_name TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    attempt_path TEXT NOT NULL,
    error_summary TEXT,
    response_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(spec_id, request_id)
);

CREATE INDEX idx_sdd_run_create_sagas_recovery
    ON sdd_run_create_sagas(stage, updated_at);

-- Failed/canceled reservations remain immutable recovery history but must not
-- strand the specification forever. Exactly one non-terminal first-run saga
-- may own its filesystem/Git targets, and a completed saga permanently keeps
-- the one-run invariant even if the aggregate is damaged later.
CREATE UNIQUE INDEX idx_sdd_run_create_sagas_one_live_per_spec
    ON sdd_run_create_sagas(spec_id)
    WHERE stage IN ('reserved', 'workspace_ready', 'publishing',
                    'recovery_required', 'completed');
