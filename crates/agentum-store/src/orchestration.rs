//! Inter-agent orchestration store: a mail store + task DAG + dispatch contexts
//! (migration 0020). Backs the `agentum orchestration` command surface.
//!
//! This is a child module of the crate root, so its `impl Store` block can use
//! the private `pool`. Types are plain (serde + sqlx::FromRow) and live here
//! rather than agentum-core because only the server/store touch them.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{Result, Store};

/// A delivered orchestration message (one row per recipient — group sends fan
/// out). `read` is 0/1; `payload` is an optional JSON string.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct OrchMessage {
    pub id: i64,
    pub thread_id: String,
    pub sender: String,
    pub recipient: String,
    pub subject: String,
    pub body: String,
    pub msg_type: String,
    pub priority: String,
    pub payload: Option<String>,
    pub read: i64,
    pub created_at: String,
}

/// Fields for a single delivered message. The route resolves a `--to` handle or
/// group into N of these (one per concrete recipient) sharing a `thread_id`.
#[derive(Debug, Clone)]
pub struct NewOrchMessage {
    pub thread_id: String,
    pub sender: String,
    pub recipient: String,
    pub subject: String,
    pub body: String,
    pub msg_type: String,
    pub priority: String,
    pub payload: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct OrchTask {
    pub id: i64,
    pub spec: String,
    pub status: String,
    pub parent_id: Option<i64>,
    pub result: Option<String>,
    pub created_at: String,
    /// The task ids this one depends on. Not a column — filled by the query.
    #[sqlx(skip)]
    #[serde(default)]
    pub deps: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct OrchDispatch {
    pub id: i64,
    pub task_id: i64,
    pub assignee: String,
    pub status: String,
    pub attempts: i64,
    pub created_at: String,
}

fn now_rfc3339() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

impl Store {
    // ---------- mail ----------

    /// Insert one delivered message and return it with its assigned id.
    pub async fn orch_insert_message(&self, m: &NewOrchMessage) -> Result<OrchMessage> {
        let created_at = now_rfc3339()?;
        let res = sqlx::query(
            "INSERT INTO orchestration_messages
               (thread_id, sender, recipient, subject, body, msg_type, priority, payload, read, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(&m.thread_id)
        .bind(&m.sender)
        .bind(&m.recipient)
        .bind(&m.subject)
        .bind(&m.body)
        .bind(&m.msg_type)
        .bind(&m.priority)
        .bind(&m.payload)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;
        Ok(OrchMessage {
            id: res.last_insert_rowid(),
            thread_id: m.thread_id.clone(),
            sender: m.sender.clone(),
            recipient: m.recipient.clone(),
            subject: m.subject.clone(),
            body: m.body.clone(),
            msg_type: m.msg_type.clone(),
            priority: m.priority.clone(),
            payload: m.payload.clone(),
            read: 0,
            created_at,
        })
    }

    /// Inbox for a recipient handle, newest first. `unread_only` filters to
    /// `read = 0`; `types` (if non-empty) filters by `msg_type`.
    pub async fn orch_inbox(
        &self,
        recipient: &str,
        unread_only: bool,
        types: &[String],
        limit: i64,
    ) -> Result<Vec<OrchMessage>> {
        // Built dynamically because the type filter is a variable-length IN list;
        // every fragment is parameterized, so no injection surface.
        let mut sql = String::from("SELECT * FROM orchestration_messages WHERE recipient = ?");
        if unread_only {
            sql.push_str(" AND read = 0");
        }
        if !types.is_empty() {
            let placeholders = vec!["?"; types.len()].join(", ");
            sql.push_str(&format!(" AND msg_type IN ({placeholders})"));
        }
        sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");

        let mut q = sqlx::query_as::<_, OrchMessage>(&sql).bind(recipient);
        for t in types {
            q = q.bind(t);
        }
        q = q.bind(limit);
        Ok(q.fetch_all(&self.pool).await?)
    }

    pub async fn orch_get_message(&self, id: i64) -> Result<Option<OrchMessage>> {
        Ok(
            sqlx::query_as::<_, OrchMessage>("SELECT * FROM orchestration_messages WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Mark a set of message ids read. No-op for an empty slice.
    pub async fn orch_mark_read(&self, ids: &[i64]) -> Result<()> {
        for id in ids {
            sqlx::query("UPDATE orchestration_messages SET read = 1 WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    // ---------- tasks ----------

    /// Create a task with optional deps. Initial status is `ready` when it has
    /// no incomplete deps, else `pending` — the same rule the DAG resolver uses.
    pub async fn orch_create_task(
        &self,
        spec: &str,
        deps: &[i64],
        parent_id: Option<i64>,
    ) -> Result<OrchTask> {
        let created_at = now_rfc3339()?;
        let status = if self.orch_deps_all_completed(deps).await? {
            "ready"
        } else {
            "pending"
        };
        let res = sqlx::query(
            "INSERT INTO orchestration_tasks (spec, status, parent_id, result, created_at)
             VALUES (?, ?, ?, NULL, ?)",
        )
        .bind(spec)
        .bind(status)
        .bind(parent_id)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;
        let id = res.last_insert_rowid();
        for dep in deps {
            sqlx::query(
                "INSERT OR IGNORE INTO orchestration_task_deps (task_id, dep_id) VALUES (?, ?)",
            )
            .bind(id)
            .bind(dep)
            .execute(&self.pool)
            .await?;
        }
        Ok(OrchTask {
            id,
            spec: spec.to_string(),
            status: status.to_string(),
            parent_id,
            result: None,
            created_at,
            deps: deps.to_vec(),
        })
    }

    /// True when every id in `deps` refers to a `completed` task. An empty list
    /// is vacuously true (a task with no deps is immediately ready).
    async fn orch_deps_all_completed(&self, deps: &[i64]) -> Result<bool> {
        if deps.is_empty() {
            return Ok(true);
        }
        // One query instead of one `SELECT status` per dep. Counts how many of
        // the *distinct* dep ids resolve to a `completed` task; equals the
        // distinct count iff every dep exists AND is completed — the same
        // semantics as the old loop (a missing dep id ≠ completed → false).
        let distinct: std::collections::HashSet<i64> = deps.iter().copied().collect();
        let placeholders = vec!["?"; distinct.len()].join(",");
        let sql = format!(
            "SELECT COUNT(*) FROM orchestration_tasks \
             WHERE id IN ({placeholders}) AND status = 'completed'"
        );
        let mut q = sqlx::query_scalar::<_, i64>(&sql);
        for dep in &distinct {
            q = q.bind(dep);
        }
        let completed: i64 = q.fetch_one(&self.pool).await?;
        Ok(completed as usize == distinct.len())
    }

    async fn orch_task_deps(&self, task_id: i64) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT dep_id FROM orchestration_task_deps WHERE task_id = ? ORDER BY dep_id",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// List tasks (optionally filtered by status, or only `ready` ones), each
    /// with its `deps` filled in. Newest first.
    pub async fn orch_list_tasks(
        &self,
        status: Option<&str>,
        ready_only: bool,
    ) -> Result<Vec<OrchTask>> {
        let mut sql = String::from("SELECT * FROM orchestration_tasks");
        let effective_status = if ready_only { Some("ready") } else { status };
        if effective_status.is_some() {
            sql.push_str(" WHERE status = ?");
        }
        sql.push_str(" ORDER BY id DESC");
        let mut q = sqlx::query_as::<_, OrchTask>(&sql);
        if let Some(s) = effective_status {
            q = q.bind(s);
        }
        let mut tasks = q.fetch_all(&self.pool).await?;
        if !tasks.is_empty() {
            // Fill every task's `deps` with a single grouped query instead of a
            // `SELECT dep_id` per task (N+1). `ORDER BY task_id, dep_id` keeps
            // each task's dep order identical to the old per-task query.
            let ids: Vec<i64> = tasks.iter().map(|t| t.id).collect();
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "SELECT task_id, dep_id FROM orchestration_task_deps \
                 WHERE task_id IN ({placeholders}) ORDER BY task_id, dep_id"
            );
            let mut q2 = sqlx::query_as::<_, (i64, i64)>(&sql);
            for id in &ids {
                q2 = q2.bind(id);
            }
            let pairs = q2.fetch_all(&self.pool).await?;
            let mut by_task: std::collections::HashMap<i64, Vec<i64>> =
                std::collections::HashMap::new();
            for (task_id, dep_id) in pairs {
                by_task.entry(task_id).or_default().push(dep_id);
            }
            for t in &mut tasks {
                t.deps = by_task.remove(&t.id).unwrap_or_default();
            }
        }
        Ok(tasks)
    }

    pub async fn orch_get_task(&self, id: i64) -> Result<Option<OrchTask>> {
        let row = sqlx::query_as::<_, OrchTask>("SELECT * FROM orchestration_tasks WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(mut t) => {
                t.deps = self.orch_task_deps(t.id).await?;
                Ok(Some(t))
            }
            None => Ok(None),
        }
    }

    /// Update a task's status (and optional JSON result). When it transitions to
    /// `completed`, run DAG resolution: any `pending` dependent whose deps are
    /// now all completed is promoted to `ready`. Returns the updated task.
    pub async fn orch_update_task(
        &self,
        id: i64,
        status: &str,
        result: Option<&str>,
    ) -> Result<Option<OrchTask>> {
        let affected = sqlx::query(
            "UPDATE orchestration_tasks SET status = ?, result = COALESCE(?, result) WHERE id = ?",
        )
        .bind(status)
        .bind(result)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Ok(None);
        }
        if status == "completed" {
            self.orch_promote_ready(id).await?;
        }
        self.orch_get_task(id).await
    }

    /// After `completed_id` completes, promote every `pending` task that depends
    /// on it to `ready` if all of that task's deps are now completed.
    async fn orch_promote_ready(&self, completed_id: i64) -> Result<()> {
        let dependents: Vec<i64> =
            sqlx::query_scalar("SELECT task_id FROM orchestration_task_deps WHERE dep_id = ?")
                .bind(completed_id)
                .fetch_all(&self.pool)
                .await?;
        for dep_task in dependents {
            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM orchestration_tasks WHERE id = ?")
                    .bind(dep_task)
                    .fetch_optional(&self.pool)
                    .await?;
            if status.as_deref() != Some("pending") {
                continue;
            }
            let deps = self.orch_task_deps(dep_task).await?;
            if self.orch_deps_all_completed(&deps).await? {
                sqlx::query("UPDATE orchestration_tasks SET status = 'ready' WHERE id = ?")
                    .bind(dep_task)
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(())
    }

    // ---------- dispatch ----------

    /// Record a dispatch of a task to an assignee handle and mark the task
    /// `dispatched`. Returns the dispatch context.
    pub async fn orch_create_dispatch(&self, task_id: i64, assignee: &str) -> Result<OrchDispatch> {
        let created_at = now_rfc3339()?;
        let res = sqlx::query(
            "INSERT INTO orchestration_dispatches (task_id, assignee, status, attempts, created_at)
             VALUES (?, ?, 'dispatched', 1, ?)",
        )
        .bind(task_id)
        .bind(assignee)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;
        sqlx::query("UPDATE orchestration_tasks SET status = 'dispatched' WHERE id = ?")
            .bind(task_id)
            .execute(&self.pool)
            .await?;
        Ok(OrchDispatch {
            id: res.last_insert_rowid(),
            task_id,
            assignee: assignee.to_string(),
            status: "dispatched".to_string(),
            attempts: 1,
            created_at,
        })
    }

    pub async fn orch_dispatches_for_task(&self, task_id: i64) -> Result<Vec<OrchDispatch>> {
        Ok(sqlx::query_as::<_, OrchDispatch>(
            "SELECT * FROM orchestration_dispatches WHERE task_id = ? ORDER BY id ASC",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    async fn store() -> Store {
        let dir = std::env::temp_dir().join(format!("agentum-orch-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // Unique db per test via an atomic counter folded into the name.
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        Store::open(&dir.join(format!("orch-{n}.db")))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn task_dag_promotes_dependents_on_completion() {
        let s = store().await;
        let a = s.orch_create_task("build", &[], None).await.unwrap();
        // b depends on a → starts pending (a is not completed yet).
        let b = s.orch_create_task("test", &[a.id], None).await.unwrap();
        assert_eq!(a.status, "ready", "no deps → ready");
        assert_eq!(b.status, "pending", "dep incomplete → pending");

        // Complete a → b is promoted to ready.
        s.orch_update_task(a.id, "completed", None).await.unwrap();
        let b2 = s.orch_get_task(b.id).await.unwrap().unwrap();
        assert_eq!(b2.status, "ready", "dep completed → promoted to ready");
    }

    #[tokio::test]
    async fn task_with_already_completed_dep_starts_ready() {
        let s = store().await;
        let a = s.orch_create_task("a", &[], None).await.unwrap();
        s.orch_update_task(a.id, "completed", None).await.unwrap();
        // Created AFTER a completed → immediately ready.
        let b = s.orch_create_task("b", &[a.id], None).await.unwrap();
        assert_eq!(b.status, "ready");
    }

    #[tokio::test]
    async fn mail_inbox_filters_unread_and_type() {
        use super::NewOrchMessage;
        let s = store().await;
        let mk = |subject: &str, ty: &str| NewOrchMessage {
            thread_id: "t1".into(),
            sender: "coord".into(),
            recipient: "worker".into(),
            subject: subject.into(),
            body: String::new(),
            msg_type: ty.into(),
            priority: "normal".into(),
            payload: None,
        };
        let m1 = s.orch_insert_message(&mk("hi", "status")).await.unwrap();
        s.orch_insert_message(&mk("go", "dispatch")).await.unwrap();

        // All for worker: 2.
        assert_eq!(
            s.orch_inbox("worker", false, &[], 50).await.unwrap().len(),
            2
        );
        // Only dispatch type: 1.
        let only_dispatch = s
            .orch_inbox("worker", false, &["dispatch".to_string()], 50)
            .await
            .unwrap();
        assert_eq!(only_dispatch.len(), 1);
        assert_eq!(only_dispatch[0].subject, "go");

        // Mark m1 read → unread count drops to 1.
        s.orch_mark_read(&[m1.id]).await.unwrap();
        assert_eq!(
            s.orch_inbox("worker", true, &[], 50).await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn dispatch_marks_task_dispatched() {
        let s = store().await;
        let t = s.orch_create_task("work", &[], None).await.unwrap();
        let d = s.orch_create_dispatch(t.id, "worker-1").await.unwrap();
        assert_eq!(d.assignee, "worker-1");
        let t2 = s.orch_get_task(t.id).await.unwrap().unwrap();
        assert_eq!(t2.status, "dispatched");
        assert_eq!(s.orch_dispatches_for_task(t.id).await.unwrap().len(), 1);
    }
}
