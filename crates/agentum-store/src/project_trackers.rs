use agentum_core::ProjectTrackerConfig;
use sqlx::Row;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{Result, Store, StoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectTrackerWrite {
    Written(ProjectTrackerConfig),
    Conflict(Option<ProjectTrackerConfig>),
}

impl Store {
    pub async fn get_project_tracker_config(
        &self,
        repo_id: &str,
    ) -> Result<Option<ProjectTrackerConfig>> {
        let row = sqlx::query("SELECT config_json FROM project_tracker_configs WHERE repo_id = ?")
            .bind(repo_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("config_json")))
            .transpose()
            .map_err(StoreError::from)
    }

    /// Atomically create/replace a canonical config. `None` means the caller
    /// expects no row; `Some(n)` is a compare-and-swap against revision n.
    pub async fn put_project_tracker_config(
        &self,
        mut config: ProjectTrackerConfig,
        expected_revision: Option<i64>,
    ) -> Result<ProjectTrackerWrite> {
        let mut tx = self.begin_write().await?;
        let current =
            sqlx::query("SELECT config_json FROM project_tracker_configs WHERE repo_id = ?")
                .bind(&config.repo_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| serde_json::from_str::<ProjectTrackerConfig>(row.get("config_json")))
                .transpose()?;
        if current.as_ref().map(|c| c.revision) != expected_revision {
            tx.rollback().await?;
            return Ok(ProjectTrackerWrite::Conflict(current));
        }
        config.revision = current.as_ref().map_or(1, |c| c.revision + 1);
        let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let json = serde_json::to_string(&config)?;
        sqlx::query(
            "INSERT INTO project_tracker_configs(repo_id, revision, config_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?) ON CONFLICT(repo_id) DO UPDATE SET \
             revision = excluded.revision, config_json = excluded.config_json, updated_at = excluded.updated_at",
        )
        .bind(&config.repo_id)
        .bind(config.revision)
        .bind(json)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ProjectTrackerWrite::Written(config))
    }

    pub async fn delete_project_tracker_config(
        &self,
        repo_id: &str,
        expected_revision: Option<i64>,
    ) -> Result<ProjectTrackerWrite> {
        let mut tx = self.begin_write().await?;
        let current =
            sqlx::query("SELECT config_json FROM project_tracker_configs WHERE repo_id = ?")
                .bind(repo_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| serde_json::from_str::<ProjectTrackerConfig>(row.get("config_json")))
                .transpose()?;
        if current.is_none() {
            tx.rollback().await?;
            return Ok(ProjectTrackerWrite::Written(ProjectTrackerConfig {
                schema_version: agentum_core::PROJECT_TRACKER_SCHEMA_VERSION,
                repo_id: repo_id.to_string(),
                revision: 0,
                provider: None,
                github: None,
                linear: None,
                task_preferences: Default::default(),
                provenance: agentum_core::ProjectTrackerProvenance::Configured,
            }));
        }
        if current.as_ref().map(|c| c.revision) != expected_revision {
            tx.rollback().await?;
            return Ok(ProjectTrackerWrite::Conflict(current));
        }
        sqlx::query("DELETE FROM project_tracker_configs WHERE repo_id = ?")
            .bind(repo_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(ProjectTrackerWrite::Written(
            current.expect("checked above"),
        ))
    }

    pub async fn find_project_trackers_by_github_slug(
        &self,
        slug: &str,
    ) -> Result<Vec<ProjectTrackerConfig>> {
        let rows = sqlx::query("SELECT config_json FROM project_tracker_configs")
            .fetch_all(&self.pool)
            .await?;
        let needle = slug.trim().to_ascii_lowercase();
        rows.into_iter()
            .map(|row| serde_json::from_str::<ProjectTrackerConfig>(row.get("config_json")))
            .filter_map(|parsed| match parsed {
                Ok(config)
                    if config.github.as_ref().is_some_and(|g| {
                        g.repository_slug.trim().to_ascii_lowercase() == needle
                    }) =>
                {
                    Some(Ok(config))
                }
                Ok(_) => None,
                Err(error) => Some(Err(StoreError::from(error))),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_core::{ProjectTrackerProvenance, ProjectTrackerProvider};

    fn config(repo: &str, provider: ProjectTrackerProvider) -> ProjectTrackerConfig {
        ProjectTrackerConfig {
            schema_version: agentum_core::PROJECT_TRACKER_SCHEMA_VERSION,
            repo_id: repo.into(),
            revision: 0,
            provider: Some(provider),
            github: None,
            linear: None,
            task_preferences: Default::default(),
            provenance: ProjectTrackerProvenance::Configured,
        }
    }

    #[tokio::test]
    async fn project_tracker_cas_and_deletion_are_repo_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store.sqlite")).await.unwrap();
        let ProjectTrackerWrite::Written(a1) = store
            .put_project_tracker_config(config("a", ProjectTrackerProvider::Github), None)
            .await
            .unwrap()
        else {
            panic!("first write")
        };
        let ProjectTrackerWrite::Written(b1) = store
            .put_project_tracker_config(config("b", ProjectTrackerProvider::Linear), None)
            .await
            .unwrap()
        else {
            panic!("second write")
        };
        assert_eq!((a1.revision, b1.revision), (1, 1));
        assert!(matches!(
            store.put_project_tracker_config(config("a", ProjectTrackerProvider::Linear), None).await.unwrap(),
            ProjectTrackerWrite::Conflict(Some(current)) if current == a1
        ));
        assert!(matches!(
            store
                .delete_project_tracker_config("a", Some(1))
                .await
                .unwrap(),
            ProjectTrackerWrite::Written(_)
        ));
        assert!(
            store
                .get_project_tracker_config("a")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.get_project_tracker_config("b").await.unwrap(),
            Some(b1)
        );
    }
}
