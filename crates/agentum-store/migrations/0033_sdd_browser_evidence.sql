-- Attempt-owned browser verification. Raw captures live in immutable,
-- content-addressed Agentum data storage; SQLite owns attribution, lifecycle,
-- review binding, durable events, and replay state.

CREATE TABLE sdd_evidence_blobs (
    sha256 TEXT PRIMARY KEY,
    byte_length INTEGER NOT NULL CHECK(byte_length > 0 AND byte_length <= 8388608),
    media_type TEXT NOT NULL,
    storage_relative_path TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL
);

CREATE TABLE sdd_browser_evidence (
    evidence_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES sdd_runs(run_id),
    attempt_id TEXT NOT NULL REFERENCES sdd_attempts(attempt_id),
    grant_id TEXT NOT NULL REFERENCES sdd_capability_grants(grant_id),
    spec_revision INTEGER NOT NULL,
    check_id TEXT NOT NULL,
    manifest_sha256 TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('passed', 'failed')),
    submitted_by TEXT NOT NULL,
    captured_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(run_id, attempt_id, check_id),
    UNIQUE(run_id, manifest_sha256)
);
CREATE INDEX idx_sdd_browser_evidence_run
    ON sdd_browser_evidence(run_id, spec_revision, created_at, evidence_id);

CREATE TABLE sdd_browser_evidence_blobs (
    evidence_id TEXT NOT NULL REFERENCES sdd_browser_evidence(evidence_id),
    sha256 TEXT NOT NULL REFERENCES sdd_evidence_blobs(sha256),
    role TEXT NOT NULL CHECK(role IN ('capture', 'console_transcript', 'network_transcript')),
    PRIMARY KEY(evidence_id, sha256, role)
);

-- review.md remains portable Markdown. These runtime-only columns prove which
-- immutable evidence set the independent review actually consumed.
ALTER TABLE sdd_artifact_revisions ADD COLUMN evidence_digest TEXT;
ALTER TABLE sdd_artifact_revisions ADD COLUMN evidence_manifest_hashes_json TEXT;
