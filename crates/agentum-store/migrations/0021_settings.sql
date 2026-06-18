-- Generic key/value settings for runtime feature gating.
--
-- First consumer: `orchestration.enabled` — whether agentum's MCP server
-- advertises (and accepts calls to) the inter-agent orchestration tools
-- (send/check messages, task DAG). Toggled from the desktop Settings → Agent
-- Orchestration pane and read by `routes/mcp.rs` at tools/list + tools/call.
-- An absent key means "unset" (the reader supplies the default), so a fresh DB
-- doesn't presuppose a value. One row per setting.

CREATE TABLE settings (
    key        TEXT PRIMARY KEY NOT NULL,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
