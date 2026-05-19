//! `/api/profiles` — REST CRUD over the shared connection-profiles file.
//!
//! Same `profiles.toml` the TUI reads/writes via
//! `agentum_core::profiles::Profiles`. Tokens are *not* in this surface
//! by design — bearer credentials stay client-local (browser storage
//! for the dashboard, `credentials.toml` for the TUI). The server only
//! syncs URL + fingerprint + insecure-bit + default pointer.
//!
//! Atomicity: every mutation loads → mutates → drops. The TOML rewrite
//! is whole-file last-writer-wins, which is fine at human edit cadence.

use agentum_core::profiles::{Profile, Profiles, ProfilesFile, is_valid_name};
use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, put};
use serde::Deserialize;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/profiles", get(list).post(create))
        // `/default` is a singleton resource — PUT with a body to set,
        // and the body's `name: null` clears the pointer (mirrors the
        // TUI's `agentum profiles use --clear`). Declared *before* the
        // `/api/profiles/{name}` route so axum picks this match first.
        .route("/api/profiles/default", put(set_default))
        .route("/api/profiles/{name}", put(update).delete(delete))
}

// ---------- payloads ----------

#[derive(Debug, Deserialize)]
struct CreateBody {
    name: String,
    #[serde(flatten)]
    profile: Profile,
}

#[derive(Debug, Deserialize)]
struct DefaultBody {
    /// `null` clears the default pointer; otherwise the name must
    /// reference an existing profile.
    #[serde(default)]
    name: Option<String>,
}

// ---------- helpers ----------

fn load_store() -> Result<Profiles, ApiError> {
    let dir = agentum_store::paths::config_dir()
        .map_err(|e| ApiError::Internal(format!("resolve config dir: {e}")))?;
    Profiles::load_from(dir.join("profiles.toml"))
        .map_err(|e| ApiError::Internal(format!("load profiles.toml: {e}")))
}

fn save_err(e: anyhow::Error) -> ApiError {
    ApiError::Internal(format!("write profiles.toml: {e}"))
}

// ---------- handlers ----------

async fn list() -> Result<Json<ProfilesFile>, ApiError> {
    let store = load_store()?;
    Ok(Json(store.file().clone()))
}

async fn create(Json(payload): Json<CreateBody>) -> Result<(StatusCode, Json<Profile>), ApiError> {
    if !is_valid_name(&payload.name) {
        return Err(ApiError::BadRequest(format!(
            "invalid profile name: `{}`",
            payload.name
        )));
    }
    let mut store = load_store()?;
    if store.get(&payload.name).is_some() {
        return Err(ApiError::Conflict(format!(
            "profile `{}` already exists",
            payload.name
        )));
    }
    let profile = payload.profile.clone();
    store
        .upsert(payload.name, profile.clone())
        .map_err(save_err)?;
    Ok((StatusCode::CREATED, Json(profile)))
}

async fn update(
    Path(name): Path<String>,
    Json(profile): Json<Profile>,
) -> Result<Json<Profile>, ApiError> {
    if !is_valid_name(&name) {
        return Err(ApiError::BadRequest(format!(
            "invalid profile name: `{name}`"
        )));
    }
    let mut store = load_store()?;
    store.upsert(name, profile.clone()).map_err(save_err)?;
    Ok(Json(profile))
}

async fn delete(Path(name): Path<String>) -> Result<StatusCode, ApiError> {
    let mut store = load_store()?;
    let removed = store.remove(&name).map_err(save_err)?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("profile `{name}`")))
    }
}

async fn set_default(Json(body): Json<DefaultBody>) -> Result<StatusCode, ApiError> {
    let mut store = load_store()?;
    // `Profiles::set_default` returns an error when the name doesn't
    // exist. Translate that into a 404 so the client can distinguish
    // "I asked for an unknown profile" from "filesystem broke".
    if let Some(ref n) = body.name {
        if store.get(n).is_none() {
            return Err(ApiError::NotFound(format!("profile `{n}`")));
        }
    }
    store.set_default(body.name).map_err(save_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    //! Handler-level round trips driven directly through the in-process
    //! functions. The path resolution goes through
    //! `agentum_store::paths::config_dir()` which honours `XDG_CONFIG_HOME`,
    //! so each test runs with that env var pointed at a temp dir.
    //!
    //! Env vars are process-wide; cargo runs tests in parallel by default.
    //! `TEST_LOCK` serialises the entire module so the env mutation and
    //! the file it resolves to stay coherent across the suite. Without
    //! this, parallel tests race on `XDG_CONFIG_HOME` and one sees
    //! another's profiles.toml.
    use super::*;
    use agentum_core::profiles::{Profile, Profiles};
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    struct TestEnv {
        _dir: TempDir,
        _guard: MutexGuard<'static, ()>,
    }

    fn isolate_xdg() -> TestEnv {
        // `lock().unwrap_or_else(...)` recovers from a poisoned mutex —
        // an earlier panicked test shouldn't take down the rest of the
        // suite with a "lock poisoned" error.
        let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().unwrap();
        // SAFETY: `set_var` is unsound under multi-threaded access.
        // `TEST_LOCK` serialises this whole module so only one thread
        // mutates the env at a time. The alternative — threading a
        // path through every handler — would bloat production call
        // sites for no runtime benefit.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", dir.path());
        }
        TestEnv {
            _dir: dir,
            _guard: guard,
        }
    }

    #[tokio::test]
    async fn list_is_empty_when_no_file() {
        let _g = isolate_xdg();
        let resp = list().await.unwrap();
        assert!(resp.0.profiles.is_empty());
        assert!(resp.0.default.is_none());
    }

    #[tokio::test]
    async fn create_then_list_round_trip() {
        let _g = isolate_xdg();
        let (status, _) = create(Json(CreateBody {
            name: "local".into(),
            profile: Profile {
                url: "https://127.0.0.1:8822".into(),
                fingerprint: None,
                insecure: false,
            },
        }))
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CREATED);

        let listing = list().await.unwrap();
        assert_eq!(listing.0.profiles.len(), 1);
        assert!(listing.0.profiles.contains_key("local"));
    }

    #[tokio::test]
    async fn rejects_invalid_name() {
        let _g = isolate_xdg();
        let err = create(Json(CreateBody {
            name: "bad name".into(),
            profile: Profile {
                url: "https://x".into(),
                fingerprint: None,
                insecure: false,
            },
        }))
        .await
        .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[tokio::test]
    async fn duplicate_create_conflicts() {
        let _g = isolate_xdg();
        let body = || CreateBody {
            name: "dup".into(),
            profile: Profile {
                url: "https://x".into(),
                fingerprint: None,
                insecure: false,
            },
        };
        let _ = create(Json(body())).await.unwrap();
        let err = create(Json(body())).await.unwrap_err();
        assert!(matches!(err, ApiError::Conflict(_)));
    }

    #[tokio::test]
    async fn update_replaces_in_place() {
        let _g = isolate_xdg();
        let _ = create(Json(CreateBody {
            name: "vps".into(),
            profile: Profile {
                url: "https://old".into(),
                fingerprint: None,
                insecure: false,
            },
        }))
        .await
        .unwrap();
        let _ = update(
            Path("vps".into()),
            Json(Profile {
                url: "https://new".into(),
                fingerprint: Some("AB:CD".into()),
                insecure: true,
            }),
        )
        .await
        .unwrap();

        let listing = list().await.unwrap();
        let stored = listing.0.profiles.get("vps").unwrap();
        assert_eq!(stored.url, "https://new");
        assert_eq!(stored.fingerprint.as_deref(), Some("AB:CD"));
        assert!(stored.insecure);
    }

    #[tokio::test]
    async fn delete_missing_returns_404() {
        let _g = isolate_xdg();
        let err = delete(Path("ghost".into())).await.unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    #[tokio::test]
    async fn set_default_validates_target() {
        let _g = isolate_xdg();
        let err = set_default(Json(DefaultBody {
            name: Some("unknown".into()),
        }))
        .await
        .unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));

        // Now create + set + verify it sticks across a reload.
        let _ = create(Json(CreateBody {
            name: "local".into(),
            profile: Profile {
                url: "https://127.0.0.1:8822".into(),
                fingerprint: None,
                insecure: false,
            },
        }))
        .await
        .unwrap();
        set_default(Json(DefaultBody {
            name: Some("local".into()),
        }))
        .await
        .unwrap();
        let reloaded = Profiles::load_from(
            agentum_store::paths::config_dir()
                .unwrap()
                .join("profiles.toml"),
        )
        .unwrap();
        assert_eq!(reloaded.default_name(), Some("local"));
    }
}
