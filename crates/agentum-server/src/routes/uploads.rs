//! `/api/sessions/{id}/uploads` — POST raw image bytes; the server writes to
//! `<workdir>/.agentum-uploads/` on the session's saved host and types the
//! relative path into that host's exact tmux pane.
//!
//! This is the daemon side of the TUI's Ctrl-V image-paste flow. The TUI
//! reads the LOCAL OS clipboard via `arboard`, PNG-encodes the RGBA
//! pixels, and POSTs the bytes here. We sniff the `Content-Type`,
//! sanitize it down to a known image extension, write the bytes under
//! `.agentum-uploads/<ts>-<rand>.<ext>`, and uses the host-aware tmux adapter to
//! send the relative path (plus a trailing space, *no* Enter) into the pane so
//! the agent picks it up as a file reference. The user commits the prompt with
//! Enter.
//!
//! The filename is daemon-controlled (timestamp + random hex + sanitised
//! extension) — never derived from user-supplied headers or the body —
//! so `send-keys` can never inject shell metacharacters or Enter into
//! the pane (T-up-01, T-up-05 in the plan's STRIDE register).
//!
//! Route is merged in `lib.rs::router()` BEFORE the bearer-token
//! middleware layer; it inherits auth enforcement and is NOT added to
//! `auth::is_public`.

use agentum_core::{Event, HostKind, LOCAL_HOST_ID};
use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::post;
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

/// Maximum upload body size. Matches what Claude Code accepts on its
/// own attachment surface; larger images would push the daemon's
/// memory footprint up unhelpfully (the body is held in memory until
/// `tokio::fs::write` completes) without buying the agent anything.
const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

/// `YYYYMMDD-HHMMSS` slug used in upload filenames. Compile-time
/// `format_description!` keeps formatting allocation-free per call.
const TS_FORMAT: &[FormatItem<'_>] =
    format_description!("[year][month][day]-[hour][minute][second]");

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/sessions/{id}/uploads",
        // 25 MiB cap. axum 0.8's default body limit is 2 MiB — the
        // route-level override lifts it just for this endpoint so the
        // global default still protects other routes.
        post(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
    )
}

#[derive(Serialize)]
struct UploadResponse {
    /// Absolute path on the daemon host. Useful for debugging via
    /// `curl | jq`; the agent itself never sees this value.
    path: String,
    /// `.agentum-uploads/<ts>-<rand>.<ext>` — what the daemon types
    /// into the tmux pane and what callers should echo in toasts.
    relative_path: String,
    size_bytes: u64,
}

async fn upload(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<UploadResponse>), ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("empty upload body".into()));
    }
    if body.len() > MAX_UPLOAD_BYTES {
        return Err(ApiError::BadRequest("upload exceeds 25 MiB".into()));
    }

    let id = parse_uuid(&id)?;
    // Resolve and hold the canonical host -> session lifecycle order before
    // reloading either record. Host PUT/DELETE and session stop/restart cannot
    // cross this transaction and redirect bytes or input to another revision.
    let (_host_guard, _session_guard) =
        super::sessions::acquire_host_and_session_lifecycle(&state, id).await?;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let host_id = session.host_id.unwrap_or(LOCAL_HOST_ID);
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("session host is missing: {host_id}")))?;
    let target = session
        .tmux_target
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("session is not running".into()))?;

    if !crate::host_runtime::has_session(&host, target)
        .await
        .map_err(|error| ApiError::from_host_runtime(&host, error))?
    {
        return Err(ApiError::BadRequest(
            "tmux session not active for this session".into(),
        ));
    }

    // Map Content-Type → extension; then sanitize. The two passes are
    // deliberate: `mime_to_ext` is the happy path (known image types),
    // `sanitize_ext` is the safety net (anything weird or attacker-
    // controlled collapses to "bin" and never carries a path
    // separator).
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ext = sanitize_ext(mime_to_ext(mime));

    // Remote session creation persists an absolute host-resolved workdir. Do
    // not expand `~` against the daemon's HOME here; that was the local-shadow
    // half of the original bug. `effective_cwd` also respects local worktrees.
    let now = OffsetDateTime::now_utc();
    let rand_hex = short_rand_hex();
    let relative_path = relative_upload_path(ext, now, &rand_hex);
    let workdir = match &host.kind {
        HostKind::Local => super::util::expand_workdir(session.effective_cwd())?,
        HostKind::Ssh { .. } => std::path::PathBuf::from(session.effective_cwd()),
    };
    let abs_path = upload_destination(&workdir, &relative_path)?;

    let abs_path_str = abs_path
        .to_str()
        .ok_or_else(|| ApiError::BadRequest("session upload path is not valid UTF-8".into()))?;
    crate::host_runtime::write_remote_file_bytes(&host, abs_path_str, &body)
        .await
        .map_err(|error| ApiError::from_host_runtime(&host, error))?;

    // Type the relative path into the pane. `false` = no trailing
    // Enter — the agent's prompt commits when the user hits return
    // themselves. Trailing space gives breathing room before the
    // user's typed context.
    crate::host_runtime::send_keys(&host, target, &format!("{relative_path} "), false)
        .await
        .map_err(|error| ApiError::from_host_runtime(&host, error))?;

    // Broadcast on the event bus so other clients (dashboard, peer
    // TUIs) can mirror the activity. Send-error means "no
    // subscribers" — that's fine, swallow it (mirrors the pattern in
    // routes::sessions::send around L354).
    let _ = state.bus.send(
        Event::new("session.upload")
            .with_session(session.id, &session.name)
            .with_payload(serde_json::json!({
                "path": relative_path,
                "size_bytes": body.len(),
            })),
    );

    let resp = UploadResponse {
        path: abs_path.to_string_lossy().into_owned(),
        relative_path: relative_path.clone(),
        size_bytes: body.len() as u64,
    };

    // Clipboard broker correlation: an `agentum clip-agent` that
    // received a `/api/clipboard/agent` request frame echoes the
    // request_id back on the upload so the broker can wake the
    // waiting TUI immediately instead of letting the 3 s timer
    // expire. Header is optional — direct uploads (e.g. TUI's local
    // fallback, dashboard image paste) skip it and the 200 response
    // shape stays identical.
    if let Some(rid_hdr) = headers.get("X-Clipboard-Request-Id")
        && let Ok(rid_str) = rid_hdr.to_str()
        && let Ok(request_id) = Uuid::parse_str(rid_str)
    {
        super::clipboard::tests_helpers_complete_clipboard_request(
            &state,
            request_id,
            super::clipboard::ClipboardOutcome::Uploaded {
                path: resp.path.clone(),
                relative_path: resp.relative_path.clone(),
                size_bytes: resp.size_bytes,
            },
        );
    }

    Ok((StatusCode::CREATED, Json(resp)))
}

/// Build the relative upload path. Pure function (no I/O, no random
/// number generation) so the format can be pinned by a unit test.
///
/// Format: `.agentum-uploads/YYYYMMDD-HHMMSS-XXXX.<ext>` — no spaces,
/// no shell metacharacters, no path separators in either the timestamp
/// or the random hex (T-up-05).
fn relative_upload_path(ext: &str, now: OffsetDateTime, rand_hex: &str) -> String {
    let ts = now
        .format(TS_FORMAT)
        .unwrap_or_else(|_| "00000000-000000".into());
    format!(".agentum-uploads/{ts}-{rand_hex}.{ext}")
}

fn upload_destination(
    workdir: &std::path::Path,
    relative_path: &str,
) -> Result<std::path::PathBuf, ApiError> {
    if !workdir.is_absolute() {
        return Err(ApiError::BadRequest(format!(
            "session workdir is not absolute: {}",
            workdir.display()
        )));
    }
    Ok(workdir.join(relative_path))
}

/// Map a Content-Type header value to a file extension. Unknown or
/// missing types return `"bin"` so the caller's `sanitize_ext` pass
/// produces a safe fallback rather than silently dropping the
/// upload.
fn mime_to_ext(mime: &str) -> &str {
    // Strip any `; charset=…` parameter before matching.
    let bare = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match bare.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        _ => "bin",
    }
}

/// Clamp an extension to a known-safe set. Anything containing `/`,
/// `\`, `.`, or a length over 4 characters collapses to `"bin"` so
/// the daemon-built filename can never carry a directory traversal,
/// shell metacharacter, or oversized header echo.
fn sanitize_ext(ext: &str) -> &'static str {
    let lower = ext.to_ascii_lowercase();
    match lower.as_str() {
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "gif" => "gif",
        "webp" => "webp",
        "bmp" => "bmp",
        _ => "bin",
    }
}

/// Short hex tag for the upload filename. 5 bytes (10 hex chars)
/// drawn from a fresh UUIDv4 — same RNG path the rest of the daemon
/// already trusts. Plenty for per-session-second collision avoidance
/// without pulling in another rand crate.
fn short_rand_hex() -> String {
    let bytes = Uuid::new_v4();
    let bytes = bytes.as_bytes();
    // 5 bytes → 10 lowercase hex chars
    let mut out = String::with_capacity(10);
    for b in &bytes[..5] {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|e| ApiError::BadRequest(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn relative_upload_path_has_expected_shape() {
        let now = datetime!(2026-05-26 12:34:56 UTC);
        let p = relative_upload_path("png", now, "abcd1234");
        // We intentionally pin every component: directory, timestamp,
        // rand hex, extension. Any drift in the format is a wire-
        // contract change.
        assert_eq!(p, ".agentum-uploads/20260526-123456-abcd1234.png");
    }

    #[test]
    fn upload_destination_stays_under_absolute_session_workdir() {
        let path = upload_destination(
            std::path::Path::new("/srv/project"),
            ".agentum-uploads/20260526-123456-abcd1234.png",
        )
        .unwrap();
        assert_eq!(
            path,
            std::path::PathBuf::from("/srv/project/.agentum-uploads/20260526-123456-abcd1234.png")
        );
        assert!(
            upload_destination(std::path::Path::new("~/project"), ".agentum-uploads/a.png")
                .is_err()
        );
        assert!(
            upload_destination(
                std::path::Path::new("relative/project"),
                ".agentum-uploads/a.png"
            )
            .is_err()
        );
    }

    #[test]
    fn sanitize_ext_normalises_known_image_types() {
        assert_eq!(sanitize_ext("png"), "png");
        assert_eq!(sanitize_ext("PNG"), "png");
        assert_eq!(sanitize_ext("jpeg"), "jpg");
        assert_eq!(sanitize_ext("JPG"), "jpg");
        assert_eq!(sanitize_ext("gif"), "gif");
        assert_eq!(sanitize_ext("webp"), "webp");
        assert_eq!(sanitize_ext("bmp"), "bmp");
    }

    #[test]
    fn sanitize_ext_blocks_path_traversal_and_garbage() {
        // T-up-01: filename construction must never embed a path
        // separator or shell metacharacter, even if the
        // Content-Type header is attacker-controlled.
        assert_eq!(sanitize_ext("../etc"), "bin");
        assert_eq!(sanitize_ext(""), "bin");
        assert_eq!(sanitize_ext("jpegjpegjpeg"), "bin");
        assert_eq!(sanitize_ext("png; charset=utf-8"), "bin");
        assert_eq!(sanitize_ext("p/n/g"), "bin");
        assert_eq!(sanitize_ext("p.n.g"), "bin");
    }

    #[test]
    fn mime_to_ext_strips_charset_param() {
        // Real Content-Type headers occasionally arrive with a
        // charset suffix (`image/png; charset=binary`); we still
        // want the bare type to drive the extension.
        assert_eq!(mime_to_ext("image/png"), "png");
        assert_eq!(mime_to_ext("image/png; charset=binary"), "png");
        assert_eq!(mime_to_ext("IMAGE/PNG"), "png");
        assert_eq!(mime_to_ext("image/jpeg"), "jpg");
        assert_eq!(mime_to_ext("application/octet-stream"), "bin");
        assert_eq!(mime_to_ext(""), "bin");
    }

    #[test]
    fn short_rand_hex_is_10_chars_lowercase_hex() {
        // Pinning the shape guarantees the upload filename never
        // grows unboundedly and stays inside the "safe filename
        // alphabet" the watchdog and the agent's prompt parsers
        // already cope with.
        let h = short_rand_hex();
        assert_eq!(h.len(), 10);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
