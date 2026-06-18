-- Inter-agent orchestration: a SQLite-backed mail store + task DAG + dispatch
-- contexts. Distinct from migration 0015 (the board/goals "orchestrator", which
-- is a planner surface) — this is the runtime that backs `agentum orchestration`
-- send/check/reply/inbox, task-create/list/update, and dispatch.
--
-- Handles are terminal/session identities (the session name, also injected into
-- panes as AGENTUM_TERMINAL_HANDLE). Group addresses (@all/@claude/@idle/
-- @worktree:<id>) are resolved to concrete handles at send time and fanned out
-- to one row per recipient, so read tracking is independent per recipient.

CREATE TABLE orchestration_messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    thread_id   TEXT NOT NULL,
    sender      TEXT NOT NULL,
    recipient   TEXT NOT NULL,
    subject     TEXT NOT NULL,
    body        TEXT NOT NULL DEFAULT '',
    msg_type    TEXT NOT NULL DEFAULT 'status',
    priority    TEXT NOT NULL DEFAULT 'normal',
    payload     TEXT,                       -- optional JSON blob
    read        INTEGER NOT NULL DEFAULT 0, -- 0 = unread, 1 = read
    created_at  TEXT NOT NULL
);

-- The hot path is "unread messages for handle X", polled by `check`/`inbox`.
CREATE INDEX idx_orch_messages_recipient ON orchestration_messages(recipient, read);
CREATE INDEX idx_orch_messages_thread ON orchestration_messages(thread_id);

CREATE TABLE orchestration_tasks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    spec        TEXT NOT NULL,
    -- pending | ready | dispatched | completed | failed | blocked
    status      TEXT NOT NULL DEFAULT 'pending',
    parent_id   INTEGER,
    result      TEXT,                       -- optional JSON result
    created_at  TEXT NOT NULL
);

CREATE INDEX idx_orch_tasks_status ON orchestration_tasks(status);

-- Edge table: `task_id` depends on `dep_id`. A task is `ready` once every
-- dep_id it lists is `completed`. Indexed both directions so DAG resolution
-- (find dependents of a just-completed task) and readiness checks are cheap.
CREATE TABLE orchestration_task_deps (
    task_id     INTEGER NOT NULL,
    dep_id      INTEGER NOT NULL,
    PRIMARY KEY (task_id, dep_id)
);

CREATE INDEX idx_orch_task_deps_dep ON orchestration_task_deps(dep_id);

CREATE TABLE orchestration_dispatches (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     INTEGER NOT NULL,
    assignee    TEXT NOT NULL,              -- recipient handle
    -- dispatched | completed | failed | circuit_broken
    status      TEXT NOT NULL DEFAULT 'dispatched',
    attempts    INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL
);

CREATE INDEX idx_orch_dispatches_task ON orchestration_dispatches(task_id);
