//! Durable state for the shared-worktree harness coordinator, workers and
//! transactional patch broker.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{Result, Store};

fn now() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct HarnessOrchestratedRunRow {
    pub run_id: String,
    pub workdir: String,
    pub plan_json: String,
    pub status: String,
    pub coordinator_session: Option<String>,
    #[serde(skip_serializing)]
    pub coordinator_token: String,
    pub max_concurrency: i64,
    pub final_gate_runs: i64,
    pub checkpoint_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct HarnessOrchestratedTaskRow {
    pub run_id: String,
    pub task_id: String,
    pub external_task_id: Option<String>,
    pub status: String,
    pub packet_json: String,
    pub deps_json: String,
    pub writable_json: String,
    pub create_dirs_json: String,
    pub worker_session: Option<String>,
    #[serde(skip_serializing)]
    pub worker_token: String,
    pub enforcement: String,
    pub context_remaining: Option<i64>,
    pub result_json: Option<String>,
    pub error_tail: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct HarnessFileLeaseRow {
    pub run_id: String,
    pub path: String,
    pub task_id: String,
    pub content_hash: String,
    pub frozen: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct HarnessPatchRow {
    pub patch_id: String,
    pub run_id: String,
    pub task_id: String,
    pub summary: String,
    pub operations_json: String,
    pub preimages_json: String,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct HarnessManagedSessionRow {
    pub session_id: String,
    pub run_id: String,
    pub task_id: Option<String>,
    pub role: String,
    pub capability_scope: String,
    pub context_remaining: Option<i64>,
    pub replaced_by: Option<String>,
    pub active: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl Store {
    pub async fn harness_create_orchestrated_run(
        &self,
        run_id: &str,
        workdir: &str,
        plan_json: &str,
        coordinator_token: &str,
        max_concurrency: i64,
    ) -> Result<()> {
        let ts = now()?;
        sqlx::query(
            "INSERT OR REPLACE INTO harness_orchestrated_runs
             (run_id,workdir,plan_json,status,coordinator_session,coordinator_token,
              max_concurrency,final_gate_runs,checkpoint_json,created_at,updated_at)
             VALUES (?,?,?,'planning',NULL,?,?,0,NULL,?,?)",
        )
        .bind(run_id)
        .bind(workdir)
        .bind(plan_json)
        .bind(coordinator_token)
        .bind(max_concurrency)
        .bind(&ts)
        .bind(&ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn harness_get_orchestrated_run(
        &self,
        run_id: &str,
    ) -> Result<Option<HarnessOrchestratedRunRow>> {
        Ok(
            sqlx::query_as("SELECT * FROM harness_orchestrated_runs WHERE run_id = ?")
                .bind(run_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn harness_orchestrated_runs(&self) -> Result<Vec<HarnessOrchestratedRunRow>> {
        Ok(
            sqlx::query_as("SELECT * FROM harness_orchestrated_runs ORDER BY created_at")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn harness_update_run(
        &self,
        run_id: &str,
        status: &str,
        coordinator_session: Option<&str>,
        checkpoint_json: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE harness_orchestrated_runs SET status=?,
             coordinator_session=COALESCE(?,coordinator_session),
             checkpoint_json=COALESCE(?,checkpoint_json),updated_at=? WHERE run_id=?",
        )
        .bind(status)
        .bind(coordinator_session)
        .bind(checkpoint_json)
        .bind(now()?)
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn harness_replace_plan(&self, run_id: &str, plan_json: &str) -> Result<()> {
        sqlx::query("UPDATE harness_orchestrated_runs SET plan_json=?,updated_at=? WHERE run_id=?")
            .bind(plan_json)
            .bind(now()?)
            .bind(run_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Atomically claim the run-level final gate. Exactly one caller gets true.
    pub async fn harness_increment_final_gate_runs(&self, run_id: &str) -> Result<bool> {
        let changed = sqlx::query(
            "UPDATE harness_orchestrated_runs SET final_gate_runs=1,status='final_verifying',
             updated_at=? WHERE run_id=? AND final_gate_runs=0",
        )
        .bind(now()?)
        .bind(run_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(changed == 1)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn harness_insert_task(
        &self,
        run_id: &str,
        task_id: &str,
        external_task_id: Option<&str>,
        status: &str,
        packet_json: &str,
        deps_json: &str,
        writable_json: &str,
        create_dirs_json: &str,
        worker_token: &str,
        enforcement: &str,
    ) -> Result<()> {
        let ts = now()?;
        sqlx::query(
            "INSERT INTO harness_orchestrated_tasks
             (run_id,task_id,external_task_id,status,packet_json,deps_json,writable_json,
              create_dirs_json,worker_token,enforcement,created_at,updated_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(run_id)
        .bind(task_id)
        .bind(external_task_id)
        .bind(status)
        .bind(packet_json)
        .bind(deps_json)
        .bind(writable_json)
        .bind(create_dirs_json)
        .bind(worker_token)
        .bind(enforcement)
        .bind(&ts)
        .bind(&ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn harness_tasks(&self, run_id: &str) -> Result<Vec<HarnessOrchestratedTaskRow>> {
        Ok(sqlx::query_as(
            "SELECT * FROM harness_orchestrated_tasks WHERE run_id=? ORDER BY task_id",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn harness_task(
        &self,
        run_id: &str,
        task_id: &str,
    ) -> Result<Option<HarnessOrchestratedTaskRow>> {
        Ok(
            sqlx::query_as("SELECT * FROM harness_orchestrated_tasks WHERE run_id=? AND task_id=?")
                .bind(run_id)
                .bind(task_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn harness_update_task(
        &self,
        run_id: &str,
        task_id: &str,
        status: &str,
        worker_session: Option<&str>,
        result_json: Option<&str>,
        error_tail: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE harness_orchestrated_tasks SET status=?,
             worker_session=COALESCE(?,worker_session),result_json=COALESCE(?,result_json),
             error_tail=?,updated_at=? WHERE run_id=? AND task_id=?",
        )
        .bind(status)
        .bind(worker_session)
        .bind(result_json)
        .bind(error_tail)
        .bind(now()?)
        .bind(run_id)
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn harness_set_task_scope(
        &self,
        run_id: &str,
        task_id: &str,
        writable_json: &str,
        worker_session: Option<&str>,
        enforcement: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE harness_orchestrated_tasks SET writable_json=?,
             worker_session=COALESCE(?,worker_session),enforcement=COALESCE(?,enforcement),
             updated_at=? WHERE run_id=? AND task_id=?",
        )
        .bind(writable_json)
        .bind(worker_session)
        .bind(enforcement)
        .bind(now()?)
        .bind(run_id)
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn harness_promote_ready_tasks(&self, run_id: &str) -> Result<()> {
        let tasks = self.harness_tasks(run_id).await?;
        let done: std::collections::HashSet<String> = tasks
            .iter()
            .filter(|t| t.status == "completed")
            .map(|t| t.task_id.clone())
            .collect();
        for task in tasks.iter().filter(|t| t.status == "pending") {
            let deps: Vec<String> = serde_json::from_str(&task.deps_json)?;
            if deps.iter().all(|dep| done.contains(dep)) {
                self.harness_update_task(run_id, &task.task_id, "ready", None, None, None)
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn harness_insert_lease(
        &self,
        run_id: &str,
        path: &str,
        task_id: &str,
        hash: &str,
    ) -> Result<()> {
        sqlx::query("INSERT INTO harness_file_leases (run_id,path,task_id,content_hash,frozen) VALUES (?,?,?,?,0)")
            .bind(run_id).bind(path).bind(task_id).bind(hash).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn harness_leases(&self, run_id: &str) -> Result<Vec<HarnessFileLeaseRow>> {
        Ok(
            sqlx::query_as("SELECT * FROM harness_file_leases WHERE run_id=? ORDER BY path")
                .bind(run_id)
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn harness_release_leases(&self, run_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM harness_file_leases WHERE run_id=?")
            .bind(run_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn harness_lease(
        &self,
        run_id: &str,
        path: &str,
    ) -> Result<Option<HarnessFileLeaseRow>> {
        Ok(
            sqlx::query_as("SELECT * FROM harness_file_leases WHERE run_id=? AND path=?")
                .bind(run_id)
                .bind(path)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn harness_update_lease_hash(
        &self,
        run_id: &str,
        path: &str,
        hash: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE harness_file_leases SET content_hash=?,frozen=0 WHERE run_id=? AND path=?",
        )
        .bind(hash)
        .bind(run_id)
        .bind(path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn harness_freeze_lease(&self, run_id: &str, path: &str) -> Result<()> {
        sqlx::query("UPDATE harness_file_leases SET frozen=1 WHERE run_id=? AND path=?")
            .bind(run_id)
            .bind(path)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn harness_transfer_lease(
        &self,
        run_id: &str,
        path: &str,
        from_task: &str,
        to_task: &str,
    ) -> Result<bool> {
        let changed = sqlx::query(
            "UPDATE harness_file_leases SET task_id=?,frozen=0
             WHERE run_id=? AND path=? AND task_id=?",
        )
        .bind(to_task)
        .bind(run_id)
        .bind(path)
        .bind(from_task)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(changed == 1)
    }

    pub async fn harness_insert_patch(
        &self,
        patch_id: &str,
        run_id: &str,
        task_id: &str,
        summary: &str,
        operations_json: &str,
        preimages_json: &str,
        status: &str,
    ) -> Result<()> {
        let ts = now()?;
        sqlx::query(
            "INSERT INTO harness_patch_ledger
             (patch_id,run_id,task_id,summary,operations_json,preimages_json,status,created_at,updated_at)
             VALUES (?,?,?,?,?,?,?,?,?)",
        ).bind(patch_id).bind(run_id).bind(task_id).bind(summary).bind(operations_json)
        .bind(preimages_json).bind(status).bind(&ts).bind(&ts).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn harness_update_patch(
        &self,
        patch_id: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE harness_patch_ledger SET status=?,error=?,updated_at=? WHERE patch_id=?",
        )
        .bind(status)
        .bind(error)
        .bind(now()?)
        .bind(patch_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn harness_patches(&self, run_id: &str) -> Result<Vec<HarnessPatchRow>> {
        Ok(sqlx::query_as(
            "SELECT * FROM harness_patch_ledger WHERE run_id=? ORDER BY created_at,patch_id",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn harness_incomplete_patches(&self) -> Result<Vec<HarnessPatchRow>> {
        Ok(sqlx::query_as(
            "SELECT * FROM harness_patch_ledger WHERE status IN ('prepared','applying')",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn harness_register_managed_session(
        &self,
        session_id: &str,
        run_id: &str,
        task_id: Option<&str>,
        role: &str,
        scope: &str,
    ) -> Result<()> {
        let ts = now()?;
        sqlx::query(
            "INSERT OR REPLACE INTO harness_managed_sessions
             (session_id,run_id,task_id,role,capability_scope,active,created_at,updated_at)
             VALUES (?,?,?,?,?,1,?,?)",
        )
        .bind(session_id)
        .bind(run_id)
        .bind(task_id)
        .bind(role)
        .bind(scope)
        .bind(&ts)
        .bind(&ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn harness_managed_session(
        &self,
        session_id: &str,
    ) -> Result<Option<HarnessManagedSessionRow>> {
        Ok(
            sqlx::query_as("SELECT * FROM harness_managed_sessions WHERE session_id=?")
                .bind(session_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn harness_active_sessions(
        &self,
        run_id: &str,
    ) -> Result<Vec<HarnessManagedSessionRow>> {
        Ok(sqlx::query_as("SELECT * FROM harness_managed_sessions WHERE run_id=? AND active<>0 ORDER BY created_at")
            .bind(run_id).fetch_all(&self.pool).await?)
    }

    pub async fn harness_release_run_sessions(&self, run_id: &str) -> Result<()> {
        sqlx::query("UPDATE harness_managed_sessions SET active=0,updated_at=? WHERE run_id=?")
            .bind(now()?)
            .bind(run_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn harness_replace_managed_session(
        &self,
        old: &str,
        replacement: &str,
    ) -> Result<()> {
        sqlx::query("UPDATE harness_managed_sessions SET active=0,replaced_by=?,updated_at=? WHERE session_id=?")
            .bind(replacement).bind(now()?).bind(old).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn harness_claim_session_rotation(&self, session_id: &str) -> Result<bool> {
        let changed = sqlx::query("UPDATE harness_managed_sessions SET active=2,updated_at=? WHERE session_id=? AND active=1")
            .bind(now()?).bind(session_id).execute(&self.pool).await?.rows_affected();
        Ok(changed == 1)
    }

    pub async fn harness_cancel_session_rotation(&self, session_id: &str) -> Result<()> {
        sqlx::query("UPDATE harness_managed_sessions SET active=1,updated_at=? WHERE session_id=? AND active=2")
            .bind(now()?).bind(session_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn harness_record_decision(
        &self,
        run_id: &str,
        decision: &str,
        payload: Option<&str>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO harness_coordinator_decisions (run_id,decision,payload_json,created_at) VALUES (?,?,?,?)")
            .bind(run_id).bind(decision).bind(payload).bind(now()?).execute(&self.pool).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    #[tokio::test]
    async fn dependencies_promote_after_completion() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("db.sqlite")).await.unwrap();
        s.harness_create_orchestrated_run("r", "/tmp/r", "{}", "c", 4)
            .await
            .unwrap();
        s.harness_insert_task(
            "r", "a", None, "ready", "{}", "[]", "[]", "[]", "ta", "enforced",
        )
        .await
        .unwrap();
        s.harness_insert_task(
            "r", "b", None, "pending", "{}", "[\"a\"]", "[]", "[]", "tb", "enforced",
        )
        .await
        .unwrap();
        s.harness_update_task("r", "a", "completed", None, None, None)
            .await
            .unwrap();
        s.harness_promote_ready_tasks("r").await.unwrap();
        assert_eq!(
            s.harness_task("r", "b").await.unwrap().unwrap().status,
            "ready"
        );
    }

    #[tokio::test]
    async fn four_independent_tasks_are_ready_and_final_gate_is_claimed_once() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("db.sqlite")).await.unwrap();
        s.harness_create_orchestrated_run("r", "/tmp/r", "{}", "c", 4)
            .await
            .unwrap();
        for id in ["a", "b", "c", "d"] {
            s.harness_insert_task(
                "r", id, None, "pending", "{}", "[]", "[]", "[]", id, "enforced",
            )
            .await
            .unwrap();
        }
        s.harness_promote_ready_tasks("r").await.unwrap();
        let tasks = s.harness_tasks("r").await.unwrap();
        assert_eq!(
            tasks.iter().filter(|task| task.status == "ready").count(),
            4
        );
        assert!(s.harness_increment_final_gate_runs("r").await.unwrap());
        assert!(!s.harness_increment_final_gate_runs("r").await.unwrap());
    }
}
