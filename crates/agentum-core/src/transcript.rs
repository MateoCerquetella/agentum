//! Per-agent plan / todo / task state extracted from Claude Code's
//! JSONL session transcripts (`~/.claude/projects/<encoded-cwd>/<sid>.jsonl`).
//!
//! Claude Code writes one JSON object per line. The objects we care about
//! carry `tool_use` blocks for the task-tracking tool family
//! (`TaskCreate`, `TaskUpdate`), `ExitPlanMode`, and the subagent-dispatch
//! tool (`Agent`, formerly `Task`). Everything else is ignored.
//!
//! - `TaskCreate` adds a row with status `pending`; the matching
//!   `tool_result` carries the assigned numeric id (`"Task #N created
//!   successfully: <subject>"`), which we parse to bind subsequent
//!   `TaskUpdate` calls.
//! - `TaskUpdate` patches a row's status / fields by `taskId`; status
//!   `deleted` removes the row.
//! - `ExitPlanMode` payloads land as the current plan body.
//! - `Agent` (legacy: `Task`) tool_use ↔ tool_result pairs become a
//!   small background-task list with status + duration.
//!
//! For backward compatibility we also still recognize the legacy
//! `TodoWrite` tool (older transcripts written by previous Claude Code
//! versions): the latest call wins and replaces the todo list outright.
//!
//! Kept dep-light (serde + std only) so this can run inside the server
//! and be exercised by unit tests without spinning up tokio or sqlx.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Status of a single TodoWrite entry. Mirrors Claude's vocabulary so
/// the parser is a pass-through (no string juggling at render time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    pub status: TodoStatus,
    /// Free-text description supplied to `TaskCreate` (`input.description`).
    /// Empty for legacy `TodoWrite`-sourced rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Numeric id assigned by Claude Code's task runtime, parsed out of
    /// the `TaskCreate` tool_result text (`"Task #N created successfully: …"`).
    /// `None` while the create call has no result yet, and always `None`
    /// for legacy `TodoWrite`-sourced rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// `tool_use_id` of the originating `TaskCreate` call. Used to bind
    /// the numeric `task_id` once the matching tool_result lands. Skipped
    /// from the wire format — it's an internal join key only.
    #[serde(default, skip_serializing, skip_deserializing)]
    pub create_tool_id: Option<String>,
}

/// Status of a background `Task` tool dispatch. `Running` until the
/// matching tool_result arrives; the result's `is_error` flag flips it
/// to `Failed` (otherwise `Completed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    /// `tool_use_id` from the transcript — used to match the result.
    pub id: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    pub status: TaskStatus,
    /// When the dispatch was first observed (RFC3339).
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// Time-to-result in milliseconds; `None` while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Snapshot of one agent's plan/todos/tasks. Built by replaying a
/// transcript top-to-bottom; later entries overwrite earlier ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentTaskState {
    /// The most recent ExitPlanMode `plan` body (markdown). `None` until
    /// the agent has called ExitPlanMode at least once in this session.
    pub plan: Option<String>,
    /// The current todo list (latest TodoWrite wins).
    pub todos: Vec<TodoItem>,
    /// In-flight + completed background `Task` dispatches, oldest-first.
    pub tasks: Vec<TaskRecord>,
}

impl AgentTaskState {
    pub fn is_empty(&self) -> bool {
        self.plan.is_none() && self.todos.is_empty() && self.tasks.is_empty()
    }
}

// ---------- path resolution ----------

/// Translate a workdir into the directory Claude Code uses for its
/// JSONL transcripts. Claude encodes the absolute path by replacing
/// every *non-alphanumeric* character with `-` (its `[^a-zA-Z0-9]`
/// → `-`), e.g. `/home/me/proj` → `-home-me-proj`.
///
/// Replacing only `/` was a latent bug: worktrees live under
/// `.claude-worktrees/`, so a worktree workdir like
/// `…/proj/.claude-worktrees/feat` must encode to `…-proj--claude-worktrees-feat`
/// (the `.` collapses with the leading `/` into `--`). A `/`-only
/// replacement kept the dot and pointed at the wrong (empty) project
/// dir; `ClaudeAdapter::launch` then mistook the live session for a
/// first launch, re-issued `--session-id`, and Claude crashed with
/// `Error: Session ID <X> is already in use` on every worktree restart.
///
/// Returns `None` if the home dir is unavailable (`$HOME`, or
/// `%USERPROFILE%` on Windows).
pub fn project_dir_for(workdir: &Path) -> Option<PathBuf> {
    let home = crate::home_dir()?;
    let abs = if workdir.is_absolute() {
        workdir.to_path_buf()
    } else {
        return None;
    };
    // Claude encodes the abs path by mapping every non-alphanumeric char to
    // `-`. On Windows that also folds the drive-letter colon and the `\`
    // separators (`C:\proj\x` -> `C--proj-x`), so the encoding is already
    // platform-robust — no separate `\`/drive handling needed.
    let s: String = abs
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    Some(home.join(".claude").join("projects").join(s))
}

/// Deterministic transcript path for a session. Claude Code names
/// transcript files `<session-id>.jsonl`, and agentum launches every
/// claude with `--session-id <agentum-session-uuid>` (see
/// `ClaudeAdapter::launch`), so the agentum session id *is* the file
/// stem. Returns `None` if the home dir is unavailable (`$HOME`, or
/// `%USERPROFILE%` on Windows) or `workdir` is not absolute.
///
/// This replaced an earlier mtime heuristic that picked the
/// most-recently-modified `*.jsonl` in the project dir. With multiple
/// agents in a single workdir that scheme cross-pollinated todos —
/// whichever agent typed last won.
pub fn transcript_path_for(workdir: &Path, session_id: Uuid) -> Option<PathBuf> {
    Some(project_dir_for(workdir)?.join(format!("{session_id}.jsonl")))
}

/// Pick the most recently modified `*.jsonl` in `dir`, ignoring
/// `exclude`. Returns `None` if the directory doesn't exist, is
/// empty, or contains only the excluded path.
///
/// Re-introduced in v0.6.26 as a *fallback* for pre-v0.6.25 sessions
/// where claude was launched without `--session-id <agentum-uuid>`
/// and so writes to its own random UUID — the deterministic
/// [`transcript_path_for`] never materializes for those sessions, and
/// without this fallback the Plan / Todos / Tasks panels stay empty
/// forever. New sessions started post-v0.6.25 hit the deterministic
/// path on their first turn and never need this; the cross-pollination
/// risk only applies when multiple pre-pin agents share one workdir.
pub fn latest_transcript_excluding(dir: &Path, exclude: Option<&Path>) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        if exclude.is_some_and(|x| x == path) {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        let Ok(mt) = meta.modified() else { continue };
        if best.as_ref().is_none_or(|(b, _)| mt > *b) {
            best = Some((mt, path));
        }
    }
    best.map(|(_, p)| p)
}

// ---------- parser ----------

/// Apply one JSONL line to `state`. Unknown / malformed lines are
/// silently ignored (transcripts are written incrementally and a
/// half-flushed line at EOF is normal).
pub fn apply_line(
    state: &mut AgentTaskState,
    pending: &mut HashMap<String, OffsetDateTime>,
    line: &str,
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let Ok(v): Result<serde_json::Value, _> = serde_json::from_str(trimmed) else {
        return;
    };

    // Claude's transcript wraps every entry in either {type:"user", message:{…}}
    // or {type:"assistant", message:{…}}. The `message.content` array contains
    // the typed blocks we care about (text, tool_use, tool_result).
    let timestamp = v
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc);

    let Some(content) = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return;
    };

    for block in content {
        let kind = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match kind {
            "tool_use" => apply_tool_use(state, pending, block, timestamp),
            "tool_result" => apply_tool_result(state, pending, block, timestamp),
            "text" => apply_text(state, pending, block),
            _ => {}
        }
    }
}

/// Claude Code injects slash-commands into the transcript as a user
/// text block wrapped in `<command-name>…</command-name>` /
/// `<command-message>…</command-message>` tags. `/clear` (and
/// `/compact` to a lesser extent) wipes the conversation context, so
/// the plan / todos / tasks tied to the previous context need to go
/// too — otherwise the right-side panel keeps showing stale entries
/// after the agent's own UI has dropped them.
///
/// Detection is in the parser instead of the TUI keystroke path
/// because the keystroke shadow can't survive Claude's slash-command
/// picker (Up/Down to navigate, Tab to autocomplete, picker-Enter to
/// commit). A transcript-level event lands no matter how the command
/// was entered, on any client.
fn apply_text(
    state: &mut AgentTaskState,
    pending: &mut HashMap<String, OffsetDateTime>,
    block: &serde_json::Value,
) {
    let Some(text) = block.get("text").and_then(|t| t.as_str()) else {
        return;
    };
    // Cheap substring check before the more expensive parse — the
    // `<command-name>` envelope only appears for slash-commands, so
    // 99% of text blocks bail here.
    if !text.contains("<command-name>") {
        return;
    }
    let Some(cmd) = extract_command_name(text) else {
        return;
    };
    // Normalize to the bare verb so future commands (e.g. /clear-cache,
    // /clear-context) opt in deliberately instead of accidentally
    // matching a substring.
    let verb = cmd.trim().trim_start_matches('/').to_ascii_lowercase();
    if matches!(verb.as_str(), "clear" | "compact") {
        *state = AgentTaskState::default();
        pending.clear();
    }
}

/// Pull the value out of a single `<command-name>…</command-name>`
/// envelope. Returns `None` if either tag is missing or malformed —
/// we don't try to handle multiple commands per text block because
/// Claude's transcripts only ever emit one envelope per slash run.
fn extract_command_name(text: &str) -> Option<&str> {
    let start_tag = "<command-name>";
    let end_tag = "</command-name>";
    let start = text.find(start_tag)? + start_tag.len();
    let end_rel = text[start..].find(end_tag)?;
    Some(&text[start..start + end_rel])
}

fn apply_tool_use(
    state: &mut AgentTaskState,
    pending: &mut HashMap<String, OffsetDateTime>,
    block: &serde_json::Value,
    ts: OffsetDateTime,
) {
    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let input = block.get("input");

    match name {
        // Current Claude Code task tool family. `TaskCreate` adds a row
        // with status `pending`; the assigned numeric id arrives later
        // in the matching tool_result and is wired up there.
        "TaskCreate" => {
            let Some(tool_use_id) = block.get("id").and_then(|i| i.as_str()).map(str::to_string)
            else {
                return;
            };
            // Skip duplicates from a transcript replay.
            if state
                .todos
                .iter()
                .any(|t| t.create_tool_id.as_deref() == Some(tool_use_id.as_str()))
            {
                return;
            }
            let subject = input
                .and_then(|i| i.get("subject"))
                .and_then(|s| s.as_str())
                .unwrap_or("(task)")
                .to_string();
            let active_form = input
                .and_then(|i| i.get("activeForm"))
                .and_then(|a| a.as_str())
                .map(str::to_string);
            let description = input
                .and_then(|i| i.get("description"))
                .and_then(|d| d.as_str())
                .map(str::to_string);
            state.todos.push(TodoItem {
                content: subject,
                active_form,
                status: TodoStatus::Pending,
                description,
                task_id: None,
                create_tool_id: Some(tool_use_id),
            });
        }
        // `TaskUpdate` patches an existing row by numeric `taskId`. We
        // accept either a string ("1") or a JSON number (1) since the
        // input shape isn't strictly enforced upstream. `status:"deleted"`
        // removes the row.
        "TaskUpdate" => {
            let task_id = input.and_then(|i| i.get("taskId")).and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
            });
            let Some(task_id) = task_id else {
                return;
            };

            let status_str = input
                .and_then(|i| i.get("status"))
                .and_then(|s| s.as_str())
                .map(str::to_string);

            // Deletion: drop the row entirely.
            if status_str.as_deref() == Some("deleted") {
                state
                    .todos
                    .retain(|t| t.task_id.as_deref() != Some(task_id.as_str()));
                return;
            }

            let new_subject = input
                .and_then(|i| i.get("subject"))
                .and_then(|s| s.as_str())
                .map(str::to_string);
            let new_active_form = input
                .and_then(|i| i.get("activeForm"))
                .and_then(|a| a.as_str())
                .map(str::to_string);
            let new_description = input
                .and_then(|i| i.get("description"))
                .and_then(|d| d.as_str())
                .map(str::to_string);

            let Some(target) = state
                .todos
                .iter_mut()
                .find(|t| t.task_id.as_deref() == Some(task_id.as_str()))
            else {
                return;
            };

            if let Some(s) = status_str {
                target.status = match s.as_str() {
                    "in_progress" => TodoStatus::InProgress,
                    "completed" => TodoStatus::Completed,
                    _ => TodoStatus::Pending,
                };
            }
            if let Some(s) = new_subject {
                target.content = s;
            }
            if new_active_form.is_some() {
                target.active_form = new_active_form;
            }
            if new_description.is_some() {
                target.description = new_description;
            }
        }
        // Legacy: pre-task-family Claude Code rewrote the whole todo list
        // on every call. Kept so old transcripts still render.
        "TodoWrite" => {
            let Some(todos) = input
                .and_then(|i| i.get("todos"))
                .and_then(|t| t.as_array())
            else {
                return;
            };
            let parsed: Vec<TodoItem> = todos
                .iter()
                .filter_map(|t| {
                    let content = t.get("content")?.as_str()?.to_string();
                    let active_form = t
                        .get("activeForm")
                        .and_then(|a| a.as_str())
                        .map(str::to_string);
                    let status_str = t.get("status")?.as_str()?;
                    let status = match status_str {
                        "in_progress" => TodoStatus::InProgress,
                        "completed" => TodoStatus::Completed,
                        _ => TodoStatus::Pending,
                    };
                    Some(TodoItem {
                        content,
                        active_form,
                        status,
                        description: None,
                        task_id: None,
                        create_tool_id: None,
                    })
                })
                .collect();
            // Latest TodoWrite wins — Claude rewrites the full list each call.
            state.todos = parsed;
        }
        "ExitPlanMode" => {
            if let Some(plan) = input.and_then(|i| i.get("plan")).and_then(|p| p.as_str()) {
                state.plan = Some(plan.to_string());
            }
        }
        // Subagent dispatch. `Agent` is the current name; `Task` is kept
        // for backward compatibility with older transcripts.
        "Agent" | "Task" => {
            let id = match block.get("id").and_then(|i| i.as_str()) {
                Some(s) => s.to_string(),
                None => return,
            };
            let description = input
                .and_then(|i| i.get("description"))
                .and_then(|d| d.as_str())
                .unwrap_or("(task)")
                .to_string();
            let subagent_type = input
                .and_then(|i| i.get("subagent_type"))
                .and_then(|t| t.as_str())
                .map(str::to_string);
            // Skip if we've already recorded this dispatch (replayed line).
            if state.tasks.iter().any(|t| t.id == id) {
                return;
            }
            pending.insert(id.clone(), ts);
            state.tasks.push(TaskRecord {
                id,
                description,
                subagent_type,
                status: TaskStatus::Running,
                started_at: ts,
                duration_ms: None,
            });
        }
        _ => {}
    }
}

fn apply_tool_result(
    state: &mut AgentTaskState,
    pending: &mut HashMap<String, OffsetDateTime>,
    block: &serde_json::Value,
    ts: OffsetDateTime,
) {
    let Some(id) = block
        .get("tool_use_id")
        .and_then(|i| i.as_str())
        .map(str::to_string)
    else {
        return;
    };

    // First: is this the result of a TaskCreate? The result content is a
    // short string like `"Task #3 created successfully: <subject>"` —
    // parse out N and bind it to the matching todo row so future
    // TaskUpdate(taskId="3") calls land. Tolerate missing/non-string
    // content (errors render as objects); just skip binding then.
    if let Some(todo) = state
        .todos
        .iter_mut()
        .find(|t| t.create_tool_id.as_deref() == Some(id.as_str()))
    {
        let result_text = block.get("content").and_then(|c| c.as_str());
        if let Some(text) = result_text
            && let Some(n) = parse_task_create_id(text)
        {
            todo.task_id = Some(n);
        }
        // Whether or not we parsed an id, the create has been observed —
        // clear the join key so a later replayed line doesn't re-bind it.
        todo.create_tool_id = None;
        return;
    }

    // Otherwise: maybe it's a subagent dispatch (Agent / legacy Task)
    // result. Close the matching record.
    let is_error = block
        .get("is_error")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);

    let Some(rec) = state.tasks.iter_mut().find(|t| t.id == id) else {
        return;
    };
    if matches!(rec.status, TaskStatus::Running) {
        rec.status = if is_error {
            TaskStatus::Failed
        } else {
            TaskStatus::Completed
        };
        if let Some(start) = pending.remove(&id) {
            let dur = (ts - start).whole_milliseconds().max(0) as u64;
            rec.duration_ms = Some(dur);
        }
    }
}

/// Pull `N` out of `"Task #N created successfully: …"`. Returns `None`
/// if the prefix doesn't match — keeps us robust to future wording
/// changes (the row stays unbound and just won't get future updates).
fn parse_task_create_id(text: &str) -> Option<String> {
    let rest = text.strip_prefix("Task #")?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_string())
}

/// Replay an entire transcript file from scratch. Convenience wrapper
/// around `apply_line` for callers that don't need incremental tailing.
pub fn parse_file(path: &Path) -> AgentTaskState {
    let mut state = AgentTaskState::default();
    let mut pending: HashMap<String, OffsetDateTime> = HashMap::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return state;
    };
    for line in content.lines() {
        apply_line(&mut state, &mut pending, line);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point `home_dir()` at a fixed test home and restore the real
    /// environment on drop. Platform-aware: on Windows it sets
    /// `%USERPROFILE%` (and clears `HOME`, which a Git-Bash dev box might
    /// otherwise have set, so resolution is deterministic); on Unix it sets
    /// `HOME`. The test home is `C:\h` on Windows, `/tmp/h` on Unix.
    ///
    /// Holds the crate-wide [`crate::TEST_ENV_LOCK`] for the whole test so
    /// that mutating process-global env here can't race the `profiles` env
    /// tests (cargo runs tests in parallel — env is process-global, not
    /// per-test). The lock is released when this guard drops.
    struct HomeEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev_home: Option<std::ffi::OsString>,
        #[cfg(windows)]
        prev_userprofile: Option<std::ffi::OsString>,
    }

    impl Drop for HomeEnvGuard {
        fn drop(&mut self) {
            unsafe {
                match self.prev_home.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
                #[cfg(windows)]
                match self.prev_userprofile.take() {
                    Some(v) => std::env::set_var("USERPROFILE", v),
                    None => std::env::remove_var("USERPROFILE"),
                }
            }
        }
    }

    fn home_env_guard() -> HomeEnvGuard {
        // Take the shared lock BEFORE touching env, and hold it for the whole
        // test (stored on the returned guard) so a parallel env test can't read
        // a half-mutated environment.
        let _lock = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev_home = std::env::var_os("HOME");
        #[cfg(windows)]
        let prev_userprofile = std::env::var_os("USERPROFILE");
        unsafe {
            #[cfg(windows)]
            {
                std::env::remove_var("HOME");
                std::env::set_var("USERPROFILE", r"C:\h");
            }
            #[cfg(not(windows))]
            std::env::set_var("HOME", "/tmp/h");
        }
        HomeEnvGuard {
            _lock,
            prev_home,
            #[cfg(windows)]
            prev_userprofile,
        }
    }

    #[test]
    fn project_dir_encodes_workdir() {
        // Inputs/expectations are platform-specific because `home_dir()`
        // reads `%USERPROFILE%` on Windows (HOME is unset there) and
        // `Path::is_absolute()` only accepts drive-rooted paths on Windows
        // (`/...` is not absolute). On Unix we still drive `HOME` + a
        // Unix-absolute workdir, matching the original assertion.
        let _g = home_env_guard();
        #[cfg(windows)]
        {
            let dir = project_dir_for(Path::new(r"C:\proj\x")).unwrap();
            // `C:\proj\x` → drive colon and `\` are non-alphanumeric → `-`.
            assert_eq!(
                dir,
                PathBuf::from(r"C:\h\.claude\projects\C--proj-x"),
                "Windows drive colon + backslashes must each fold to `-`"
            );
        }
        #[cfg(not(windows))]
        {
            let dir = project_dir_for(Path::new("/home/me/proj/x")).unwrap();
            assert_eq!(
                dir,
                PathBuf::from("/tmp/h/.claude/projects/-home-me-proj-x")
            );
        }
    }

    #[test]
    fn project_dir_encodes_non_alnum_like_claude() {
        // Regression for the "Session ID <X> is already in use" crash:
        // Claude Code encodes the cwd by replacing EVERY non-alphanumeric
        // char with `-` (its `[^a-zA-Z0-9] -> -`), not just `/`. Worktrees
        // live under `.claude-worktrees/`, so the `.` matters: `/.claude`
        // must become `--claude` (two dashes), or our transcript-existence
        // check looks in the wrong dir, falls through to `--session-id`,
        // and Claude rejects the already-claimed id on restart.
        let _g = home_env_guard();
        #[cfg(windows)]
        {
            // Same invariant on Windows: every non-alphanumeric char folds
            // to `-`. The `.` in `.claude-worktrees`, the `\` separators,
            // and the drive colon all collapse together.
            let dir = project_dir_for(Path::new(r"C:\proj\.claude-worktrees\feat-x")).unwrap();
            assert_eq!(
                dir,
                PathBuf::from(r"C:\h\.claude\projects\C--proj--claude-worktrees-feat-x"),
                "`.claude` (dot) + `\\` separators + drive colon must each fold to `-`"
            );
            // A space is also non-alphanumeric and must become `-`.
            let spaced = project_dir_for(Path::new(r"C:\My Proj")).unwrap();
            assert_eq!(spaced, PathBuf::from(r"C:\h\.claude\projects\C--My-Proj"));
        }
        #[cfg(not(windows))]
        {
            let dir = project_dir_for(Path::new("/home/me/proj/.claude-worktrees/feat-x")).unwrap();
            assert_eq!(
                dir,
                PathBuf::from("/tmp/h/.claude/projects/-home-me-proj--claude-worktrees-feat-x"),
                "`/.claude` must encode to `--claude` (dot -> dash), matching Claude"
            );
            // A space in the path is also non-alphanumeric and must become `-`.
            let spaced = project_dir_for(Path::new("/home/me/My Proj")).unwrap();
            assert_eq!(
                spaced,
                PathBuf::from("/tmp/h/.claude/projects/-home-me-My-Proj")
            );
        }
    }

    #[test]
    fn transcript_path_pins_to_session_id() {
        // Two sessions in the same workdir resolve to two different
        // files — that's the whole point of switching from mtime
        // heuristic to deterministic id-pinning.
        let _g = home_env_guard();
        let id_a = Uuid::parse_str("00000000-0000-0000-0000-00000000000a").unwrap();
        let id_b = Uuid::parse_str("00000000-0000-0000-0000-00000000000b").unwrap();
        // Use a platform-absolute workdir: `/...` isn't absolute on Windows,
        // so `project_dir_for` would return None and `.unwrap()` would panic.
        #[cfg(windows)]
        let (workdir, expected) = (
            Path::new(r"C:\proj"),
            PathBuf::from(
                r"C:\h\.claude\projects\C--proj\00000000-0000-0000-0000-00000000000a.jsonl",
            ),
        );
        #[cfg(not(windows))]
        let (workdir, expected) = (
            Path::new("/home/me/proj"),
            PathBuf::from(
                "/tmp/h/.claude/projects/-home-me-proj/00000000-0000-0000-0000-00000000000a.jsonl",
            ),
        );
        let pa = transcript_path_for(workdir, id_a).unwrap();
        let pb = transcript_path_for(workdir, id_b).unwrap();
        assert_eq!(pa, expected);
        assert_ne!(pa, pb);
    }

    #[test]
    fn todowrite_replaces_full_list() {
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();
        let line1 = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"t1","name":"TodoWrite","input":{"todos":[{"content":"a","status":"pending","activeForm":"doing a"}]}}]}}"#;
        apply_line(&mut state, &mut pending, line1);
        assert_eq!(state.todos.len(), 1);
        assert_eq!(state.todos[0].content, "a");
        assert!(matches!(state.todos[0].status, TodoStatus::Pending));

        let line2 = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"t2","name":"TodoWrite","input":{"todos":[{"content":"a","status":"completed"},{"content":"b","status":"in_progress"}]}}]}}"#;
        apply_line(&mut state, &mut pending, line2);
        assert_eq!(state.todos.len(), 2);
        assert!(matches!(state.todos[0].status, TodoStatus::Completed));
        assert!(matches!(state.todos[1].status, TodoStatus::InProgress));
    }

    #[test]
    fn exit_plan_mode_captures_body() {
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();
        let line = r##"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"p1","name":"ExitPlanMode","input":{"plan":"# Step 1\nDo a thing"}}]}}"##;
        apply_line(&mut state, &mut pending, line);
        assert_eq!(state.plan.as_deref(), Some("# Step 1\nDo a thing"));
    }

    #[test]
    fn task_pairs_with_result() {
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();
        let dispatch = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"task-abc","name":"Task","input":{"description":"explore","subagent_type":"Explore"}}]}}"#;
        apply_line(&mut state, &mut pending, dispatch);
        assert_eq!(state.tasks.len(), 1);
        assert!(matches!(state.tasks[0].status, TaskStatus::Running));

        let result = r#"{"type":"user","timestamp":"2025-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"task-abc","is_error":false,"content":"ok"}]}}"#;
        apply_line(&mut state, &mut pending, result);
        assert!(matches!(state.tasks[0].status, TaskStatus::Completed));
        assert_eq!(state.tasks[0].duration_ms, Some(5_000));
    }

    #[test]
    fn agent_dispatch_pairs_with_result() {
        // Same lifecycle as legacy Task, but under the current `Agent`
        // tool name — this is what live transcripts actually emit.
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();
        let dispatch = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"agent-1","name":"Agent","input":{"description":"map repo","subagent_type":"Explore"}}]}}"#;
        apply_line(&mut state, &mut pending, dispatch);
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.tasks[0].description, "map repo");
        assert!(matches!(state.tasks[0].status, TaskStatus::Running));

        let result = r#"{"type":"user","timestamp":"2025-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"agent-1","is_error":false,"content":"done"}]}}"#;
        apply_line(&mut state, &mut pending, result);
        assert!(matches!(state.tasks[0].status, TaskStatus::Completed));
        assert_eq!(state.tasks[0].duration_ms, Some(2_000));
    }

    #[test]
    fn task_create_then_update_lifecycle() {
        // Reproduce a real transcript shape: TaskCreate → tool_result
        // ("Task #1 created successfully: …") → TaskUpdate(in_progress)
        // → TaskUpdate(completed). Verifies the task-family parser end
        // to end since the legacy TodoWrite path never sees this anymore.
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();

        let create = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"toolu_create_1","name":"TaskCreate","input":{"subject":"Build it","description":"Build the thing","activeForm":"Building it"}}]}}"#;
        apply_line(&mut state, &mut pending, create);
        assert_eq!(state.todos.len(), 1);
        assert_eq!(state.todos[0].content, "Build it");
        assert_eq!(state.todos[0].active_form.as_deref(), Some("Building it"));
        assert!(matches!(state.todos[0].status, TodoStatus::Pending));
        assert_eq!(state.todos[0].task_id, None);

        let create_result = r#"{"type":"user","timestamp":"2025-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_create_1","content":"Task #1 created successfully: Build it"}]}}"#;
        apply_line(&mut state, &mut pending, create_result);
        assert_eq!(state.todos[0].task_id.as_deref(), Some("1"));

        let update_in_progress = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"toolu_upd_1","name":"TaskUpdate","input":{"taskId":"1","status":"in_progress"}}]}}"#;
        apply_line(&mut state, &mut pending, update_in_progress);
        assert!(matches!(state.todos[0].status, TodoStatus::InProgress));

        let update_completed = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:03Z","message":{"content":[{"type":"tool_use","id":"toolu_upd_2","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}"#;
        apply_line(&mut state, &mut pending, update_completed);
        assert!(matches!(state.todos[0].status, TodoStatus::Completed));
    }

    #[test]
    fn task_update_deleted_drops_row() {
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();
        let create = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"c1","name":"TaskCreate","input":{"subject":"Tmp"}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2025-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"c1","content":"Task #7 created successfully: Tmp"}]}}"#;
        apply_line(&mut state, &mut pending, create);
        apply_line(&mut state, &mut pending, result);
        assert_eq!(state.todos.len(), 1);

        let del = r#"{"type":"assistant","timestamp":"2025-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"u1","name":"TaskUpdate","input":{"taskId":"7","status":"deleted"}}]}}"#;
        apply_line(&mut state, &mut pending, del);
        assert!(state.todos.is_empty());
    }

    #[test]
    fn task_update_accepts_numeric_id() {
        // Some callers pass `taskId` as a JSON number rather than a
        // string. Both should work.
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();
        apply_line(
            &mut state,
            &mut pending,
            r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"c1","name":"TaskCreate","input":{"subject":"X"}}]}}"#,
        );
        apply_line(
            &mut state,
            &mut pending,
            r#"{"type":"user","timestamp":"2025-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"c1","content":"Task #4 created successfully: X"}]}}"#,
        );
        apply_line(
            &mut state,
            &mut pending,
            r#"{"type":"assistant","timestamp":"2025-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"u1","name":"TaskUpdate","input":{"taskId":4,"status":"in_progress"}}]}}"#,
        );
        assert!(matches!(state.todos[0].status, TodoStatus::InProgress));
    }

    #[test]
    fn parse_task_create_id_handles_known_shapes() {
        assert_eq!(
            parse_task_create_id("Task #1 created successfully: foo"),
            Some("1".to_string())
        );
        assert_eq!(
            parse_task_create_id("Task #42 created successfully: bar"),
            Some("42".to_string())
        );
        assert_eq!(parse_task_create_id("nope"), None);
        assert_eq!(parse_task_create_id("Task #"), None);
    }

    #[test]
    fn malformed_lines_are_ignored() {
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();
        apply_line(&mut state, &mut pending, "");
        apply_line(&mut state, &mut pending, "not json");
        apply_line(&mut state, &mut pending, r#"{"truncated":"#);
        apply_line(&mut state, &mut pending, r#"{"type":"system"}"#);
        assert!(state.is_empty());
    }

    #[test]
    fn clear_command_resets_state() {
        // Build up a state with a plan + a todo, then drop a `/clear`
        // command in the transcript. The plan/todo should be wiped so
        // the right-side panel doesn't carry over context the agent
        // has forgotten about.
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();
        apply_line(
            &mut state,
            &mut pending,
            r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"p1","name":"ExitPlanMode","input":{"plan":"step one"}}]}}"#,
        );
        apply_line(
            &mut state,
            &mut pending,
            r#"{"type":"assistant","timestamp":"2025-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"c1","name":"TaskCreate","input":{"subject":"X"}}]}}"#,
        );
        assert!(!state.is_empty(), "state should have plan + todo");

        apply_line(
            &mut state,
            &mut pending,
            r#"{"type":"user","timestamp":"2025-01-01T00:00:02Z","message":{"content":[{"type":"text","text":"<command-name>/clear</command-name><command-message>clear</command-message>"}]}}"#,
        );
        assert!(
            state.is_empty(),
            "state should be wiped after /clear, got {state:?}"
        );
    }

    #[test]
    fn compact_command_also_resets_state() {
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();
        apply_line(
            &mut state,
            &mut pending,
            r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"c1","name":"TaskCreate","input":{"subject":"X"}}]}}"#,
        );
        assert!(!state.is_empty());
        apply_line(
            &mut state,
            &mut pending,
            r#"{"type":"user","timestamp":"2025-01-01T00:00:02Z","message":{"content":[{"type":"text","text":"<command-name>/compact</command-name>"}]}}"#,
        );
        assert!(state.is_empty());
    }

    #[test]
    fn unrelated_command_does_not_reset_state() {
        // Defensive — make sure `/clear-cache` or other commands that
        // happen to contain "clear" as a substring don't trigger the
        // wipe.
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();
        apply_line(
            &mut state,
            &mut pending,
            r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"c1","name":"TaskCreate","input":{"subject":"X"}}]}}"#,
        );
        apply_line(
            &mut state,
            &mut pending,
            r#"{"type":"user","timestamp":"2025-01-01T00:00:02Z","message":{"content":[{"type":"text","text":"<command-name>/help</command-name>"}]}}"#,
        );
        assert!(!state.is_empty(), "/help shouldn't wipe state");
    }
}
