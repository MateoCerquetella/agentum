-- Durable authoritative worktree/repo/host/path identity for orchestrated
-- harness runs. Nullable columns preserve pre-scope rows; readers fall back to
-- the historical local `workdir` when all scope columns are NULL.
ALTER TABLE harness_orchestrated_runs ADD COLUMN scope_worktree_id TEXT;
ALTER TABLE harness_orchestrated_runs ADD COLUMN scope_repo_id TEXT;
ALTER TABLE harness_orchestrated_runs ADD COLUMN scope_host_id TEXT;
ALTER TABLE harness_orchestrated_runs ADD COLUMN scope_path TEXT;
