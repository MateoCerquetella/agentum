CREATE TABLE project_tracker_configs (
    repo_id TEXT PRIMARY KEY NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    config_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_project_tracker_configs_updated_at
    ON project_tracker_configs(updated_at);
