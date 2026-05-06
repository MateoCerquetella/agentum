//! Per-agent plan / todo / task state extracted from Claude Code's
//! JSONL session transcripts (`~/.claude/projects/<encoded-cwd>/<sid>.jsonl`).
//!
//! Claude Code writes one JSON object per line. The objects we care about
//! carry `tool_use` blocks for `TodoWrite`, `ExitPlanMode`, and `Task` —
//! everything else is ignored. Latest TodoWrite wins (Claude rewrites the
//! whole list each call); ExitPlanMode payloads accumulate as the
//! "current plan"; `Task` tool_use ↔ `tool_result` pairs become a small
//! background-task list with status + duration.
//!
//! Kept dep-light (serde + std only) so this can run inside the server
//! and be exercised by unit tests without spinning up tokio or sqlx.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

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
/// every `/` with `-` (e.g. `/home/me/proj` → `-home-me-proj`).
///
/// Returns `None` if `$HOME` is unavailable.
pub fn project_dir_for(workdir: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let abs = if workdir.is_absolute() {
        workdir.to_path_buf()
    } else {
        return None;
    };
    let s = abs.to_string_lossy().replace('/', "-");
    Some(home.join(".claude").join("projects").join(s))
}

/// Pick the most recently modified `*.jsonl` in `dir`. Returns `None`
/// if the directory doesn't exist or is empty.
///
/// This is a heuristic — Claude Code names files by session UUID, and
/// the TUI doesn't (yet) know which UUID corresponds to which agentum
/// session. mtime is the right tiebreaker for "the agent currently
/// running in this workdir".
pub fn latest_transcript(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
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
pub fn apply_line(state: &mut AgentTaskState, pending: &mut HashMap<String, OffsetDateTime>, line: &str) {
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
            _ => {}
        }
    }
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
        "Task" => {
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

    #[test]
    fn project_dir_encodes_workdir() {
        // Save existing HOME so this test doesn't bleed across runs.
        let saved = std::env::var_os("HOME");
        // SAFETY: tests run single-threaded by default; we restore HOME
        // before returning.
        unsafe {
            std::env::set_var("HOME", "/tmp/h");
        }
        let dir = project_dir_for(Path::new("/home/me/proj/x")).unwrap();
        assert_eq!(
            dir,
            PathBuf::from("/tmp/h/.claude/projects/-home-me-proj-x")
        );
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
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
    fn malformed_lines_are_ignored() {
        let mut state = AgentTaskState::default();
        let mut pending = HashMap::new();
        apply_line(&mut state, &mut pending, "");
        apply_line(&mut state, &mut pending, "not json");
        apply_line(&mut state, &mut pending, r#"{"truncated":"#);
        apply_line(&mut state, &mut pending, r#"{"type":"system"}"#);
        assert!(state.is_empty());
    }
}
