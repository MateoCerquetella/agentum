//! In-app auto-update, driven by `tauri-plugin-updater`.
//!
//! The bottom-right `UpdateCard` (ui/src/components/UpdateCard.tsx) is a finished
//! state machine that calls these commands and reacts to `updater-status` events.
//! Our job here is to make the events it expects real: drive
//! check → available → download(progress) → downloaded → install, emitting an
//! [`UpdateStatus`] that mirrors the TS `UpdateStatus` union in
//! `ui/src/shared/types.ts` EXACTLY (tagged by `state`, camelCase fields) so the
//! renderer needs no changes.
//!
//! Flow split (matches the card): `updater_check` finds an update and stashes it;
//! `updater_download` downloads the bytes (emitting progress) and stashes them;
//! `updater_quit_and_install` installs the stashed bytes and relaunches. The card
//! auto-calls quit-and-install once it sees `downloaded` for a card-initiated
//! download, so the user only clicks "Update" once.

use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;

/// Tauri event the renderer's `api.updater.onStatus` subscribes to.
const STATUS_EVENT: &str = "updater-status";

/// Renderer-facing update status. MUST stay byte-for-byte compatible with the
/// `UpdateStatus` union in `ui/src/shared/types.ts`: `serde(tag = "state")` with
/// kebab-case variants reproduces `state: 'not-available'` etc., and the
/// per-field `rename` keeps `userInitiated`/`changelog` camelCase. Changing a
/// name here silently breaks the card's pattern matching.
#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum UpdateStatus {
    Idle,
    Checking {
        #[serde(rename = "userInitiated")]
        user_initiated: bool,
    },
    Available {
        version: String,
        // Always null for now → the card renders its "simple" variant. Rich
        // changelog (title/media/notes) is a future enhancement; `null` is an
        // explicitly handled value in the TS union, not a missing field.
        changelog: Option<Value>,
    },
    NotAvailable {
        #[serde(rename = "userInitiated")]
        user_initiated: bool,
    },
    Downloading {
        percent: u8,
        version: String,
    },
    Downloaded {
        version: String,
    },
    Error {
        message: String,
        #[serde(rename = "userInitiated")]
        user_initiated: bool,
    },
}

/// Holds the in-flight update across the check → download → install commands.
/// The plugin's `Update` is what `download`/`install` operate on, so it must
/// survive between separate IPC calls; the downloaded bytes likewise bridge
/// `download` → `quit_and_install`. `last_status` lets `get_status` answer the
/// renderer's hydrate-on-mount call with the real current state.
#[derive(Default)]
pub struct UpdaterRuntime {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    pending: Option<tauri_plugin_updater::Update>,
    downloaded: Option<Vec<u8>>,
    last_status: Option<UpdateStatus>,
}

/// Record + broadcast a status. Stored as `last_status` first so a renderer that
/// mounts (or remounts) mid-flow and calls `get_status` sees the latest phase.
fn emit_status(app: &AppHandle, status: UpdateStatus) {
    if let Some(rt) = app.try_state::<UpdaterRuntime>() {
        if let Ok(mut inner) = rt.inner.lock() {
            inner.last_status = Some(status.clone());
        }
    }
    let _ = app.emit(STATUS_EVENT, status);
}

/// Run a check and emit the result. Shared by the silent launch check
/// (`user_initiated = false`, so a "you're up to date" result stays invisible)
/// and the explicit Settings/card check (`true`, which surfaces every outcome).
pub async fn run_check(app: AppHandle, user_initiated: bool) {
    emit_status(&app, UpdateStatus::Checking { user_initiated });

    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            emit_status(
                &app,
                UpdateStatus::Error {
                    message: format!("updater unavailable: {e}"),
                    user_initiated,
                },
            );
            return;
        }
    };

    match updater.check().await {
        Ok(Some(update)) => {
            let version = update.version.clone();
            // Stash the Update so `download` can act on the same object; clear any
            // stale bytes from a previous, superseded update.
            if let Some(rt) = app.try_state::<UpdaterRuntime>() {
                if let Ok(mut inner) = rt.inner.lock() {
                    inner.pending = Some(update);
                    inner.downloaded = None;
                }
            }
            emit_status(
                &app,
                UpdateStatus::Available {
                    version,
                    changelog: None,
                },
            );
        }
        Ok(None) => emit_status(&app, UpdateStatus::NotAvailable { user_initiated }),
        Err(e) => emit_status(
            &app,
            UpdateStatus::Error {
                message: format!("{e}"),
                user_initiated,
            },
        ),
    }
}

/// Cadence for the background re-check loop. The launch check runs immediately;
/// thereafter we re-check on this interval so an app that's left open surfaces a
/// new release on its own — previously the check was launch-only, so a running
/// instance never noticed newer versions until a relaunch.
const RECHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// True when an update is already surfaced or installing — the periodic loop
/// skips a re-check then, so it neither clobbers an in-flight download nor
/// re-emits a card the user is already looking at.
fn update_in_flight(app: &AppHandle) -> bool {
    let Some(rt) = app.try_state::<UpdaterRuntime>() else {
        return false;
    };
    let Ok(inner) = rt.inner.lock() else {
        return false;
    };
    matches!(
        inner.last_status.as_ref(),
        Some(UpdateStatus::Available { .. })
            | Some(UpdateStatus::Downloading { .. })
            | Some(UpdateStatus::Downloaded { .. })
    )
}

/// Immediate launch check, then silent background re-checks every
/// [`RECHECK_INTERVAL`]. Spawned once from `setup`. Re-emitting `Available` for a
/// version the user already dismissed is suppressed renderer-side
/// (`dismissedUpdateVersion`), so this doesn't re-nag; a genuinely newer version
/// surfaces the card.
pub async fn run_check_loop(app: AppHandle) {
    run_check(app.clone(), false).await;
    loop {
        tokio::time::sleep(RECHECK_INTERVAL).await;
        if update_in_flight(&app) {
            continue;
        }
        run_check(app.clone(), false).await;
    }
}

#[tauri::command]
pub fn updater_get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
pub fn updater_get_status(state: State<'_, UpdaterRuntime>) -> Value {
    let status = state
        .inner
        .lock()
        .ok()
        .and_then(|i| i.last_status.clone())
        .unwrap_or(UpdateStatus::Idle);
    serde_json::to_value(status).unwrap_or_else(|_| json!({ "state": "idle" }))
}

/// Explicit user check (Settings button / error-card "Re-check"). Always
/// user-initiated so the card shows "Checking…/Up to date" feedback. The
/// `include_prerelease` flag from the UI is accepted but not yet acted on — the
/// configured endpoint serves only stable releases.
#[tauri::command]
pub async fn updater_check(app: AppHandle, include_prerelease: Option<bool>) {
    let _ = include_prerelease;
    run_check(app, true).await;
}

/// Download the stashed update, emitting `downloading` progress, then `downloaded`.
/// Does not install — that's `quit_and_install`, which the card calls next.
#[tauri::command]
pub async fn updater_download(app: AppHandle) -> Result<(), String> {
    // Clone the Update out of the lock; a std Mutex guard must never be held
    // across the .await below.
    let update = {
        let rt = app
            .try_state::<UpdaterRuntime>()
            .ok_or("updater state missing")?;
        let inner = rt.inner.lock().map_err(|_| "updater lock poisoned")?;
        inner.pending.clone()
    };
    let update = match update {
        Some(u) => u,
        None => {
            // No cached update (e.g. the app was relaunched between check and
            // click). Re-check so the card refreshes, and fail loudly.
            run_check(app.clone(), true).await;
            return Err("no update available to download".into());
        }
    };
    let version = update.version.clone();

    let app_for_chunk = app.clone();
    let ver_for_chunk = version.clone();
    let mut downloaded: u64 = 0;
    let mut last_percent: i16 = -1; // force the first emit even at 0%
    let bytes = update
        .download(
            move |chunk_len, content_len| {
                downloaded = downloaded.saturating_add(chunk_len as u64);
                let percent = match content_len {
                    Some(total) if total > 0 => ((downloaded.min(total) * 100) / total) as i16,
                    // No Content-Length: we can't compute a percentage, so hold
                    // at 0 rather than emit a fake bar.
                    _ => 0,
                };
                if percent != last_percent {
                    last_percent = percent;
                    emit_status(
                        &app_for_chunk,
                        UpdateStatus::Downloading {
                            percent: percent as u8,
                            version: ver_for_chunk.clone(),
                        },
                    );
                }
            },
            || {},
        )
        .await
        .map_err(|e| {
            let msg = format!("{e}");
            // Background flag: the card keys error visibility off the cached
            // version (present here), not this flag, so false is correct.
            emit_status(
                &app,
                UpdateStatus::Error {
                    message: msg.clone(),
                    user_initiated: false,
                },
            );
            msg
        })?;

    if let Some(rt) = app.try_state::<UpdaterRuntime>() {
        if let Ok(mut inner) = rt.inner.lock() {
            inner.downloaded = Some(bytes);
        }
    }
    emit_status(&app, UpdateStatus::Downloaded { version });
    Ok(())
}

/// Install the downloaded bytes and relaunch into the new version.
/// `app.restart()` replaces the process and never returns.
#[tauri::command]
pub async fn updater_quit_and_install(app: AppHandle) -> Result<(), String> {
    let (update, bytes) = {
        let rt = app
            .try_state::<UpdaterRuntime>()
            .ok_or("updater state missing")?;
        let inner = rt.inner.lock().map_err(|_| "updater lock poisoned")?;
        (inner.pending.clone(), inner.downloaded.clone())
    };
    let update = update.ok_or("no update prepared")?;
    let bytes = bytes.ok_or("update not downloaded yet")?;
    update
        .install(bytes)
        .map_err(|e| format!("install failed: {e}"))?;
    // `restart()` diverges (-> !): it replaces the process and never returns, so
    // it satisfies the Result return type with no explicit Ok and no code after.
    app.restart()
}

/// UI-only affordance (dismissing the nudge is tracked renderer-side). Kept as a
/// registered command so the contract stays complete; nothing to do natively.
#[tauri::command]
pub fn updater_dismiss_nudge() {}

#[cfg(test)]
mod tests {
    use super::*;

    // Guards the wire contract with ui/src/shared/types.ts `UpdateStatus`. If a
    // tag string or field name drifts here, the bottom-right UpdateCard silently
    // stops matching — this catches it at `cargo test` instead of at runtime.
    #[test]
    fn status_serializes_to_the_ts_union_shape() {
        let j = |s: UpdateStatus| serde_json::to_value(s).unwrap();

        assert_eq!(
            j(UpdateStatus::Idle),
            serde_json::json!({ "state": "idle" })
        );
        assert_eq!(
            j(UpdateStatus::Checking {
                user_initiated: true
            }),
            serde_json::json!({ "state": "checking", "userInitiated": true })
        );
        assert_eq!(
            j(UpdateStatus::Available {
                version: "0.15.0".into(),
                changelog: None
            }),
            serde_json::json!({ "state": "available", "version": "0.15.0", "changelog": null })
        );
        assert_eq!(
            j(UpdateStatus::NotAvailable {
                user_initiated: false
            }),
            serde_json::json!({ "state": "not-available", "userInitiated": false })
        );
        assert_eq!(
            j(UpdateStatus::Downloading {
                percent: 42,
                version: "0.15.0".into()
            }),
            serde_json::json!({ "state": "downloading", "percent": 42, "version": "0.15.0" })
        );
        assert_eq!(
            j(UpdateStatus::Downloaded {
                version: "0.15.0".into()
            }),
            serde_json::json!({ "state": "downloaded", "version": "0.15.0" })
        );
        assert_eq!(
            j(UpdateStatus::Error {
                message: "boom".into(),
                user_initiated: true
            }),
            serde_json::json!({ "state": "error", "message": "boom", "userInitiated": true })
        );
    }
}
