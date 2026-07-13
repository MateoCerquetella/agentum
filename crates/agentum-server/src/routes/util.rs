//! Shared route helpers.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::error::ApiError;

/// Expand `~` / `~/x` to the daemon's `$HOME` and trim trailing
/// slashes (preserving a bare `/`). Other paths pass through unchanged.
///
/// The dashboard's `DirPicker` placeholder hints at `~/projects/foo`,
/// and users typing or pasting tilde-prefixed paths used to hit a
/// `400 workdir does not exist` because `PathBuf::from("~/…").exists()`
/// is always false — tilde expansion is a shell concern, not an OS one.
/// `/api/fs/list` already resolves the same way, so every workdir
/// gate now matches the picker's behaviour.
pub(crate) fn expand_workdir(raw: &str) -> Result<PathBuf, ApiError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    expand_with_home(raw, home.as_deref())
}

/// The explicit-home form — `pub(crate)` as the TEST SEAM: callers that must
/// unit-test tilde expansion (chat's repo-context gather) pass a temp home
/// instead of mutating `HOME`, which races the parallel test suite.
pub(crate) fn expand_with_home(raw: &str, home: Option<&Path>) -> Result<PathBuf, ApiError> {
    let trimmed = raw.trim();
    let trimmed = if trimmed.len() > 1 {
        trimmed.trim_end_matches('/')
    } else {
        trimmed
    };
    if trimmed.is_empty() {
        return Err(ApiError::BadRequest("workdir is empty".into()));
    }
    if trimmed == "~" || trimmed.starts_with("~/") {
        let home = home.ok_or_else(|| ApiError::Internal("HOME not set".into()))?;
        if trimmed == "~" {
            return Ok(home.to_path_buf());
        }
        return Ok(home.join(&trimmed[2..]));
    }
    Ok(PathBuf::from(trimmed))
}

/// Parse a path-segment UUID, mapping a malformed id to a 400. Shared by the
/// session/git/host/upload routes (previously copy-pasted into each).
pub(crate) fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|e| ApiError::BadRequest(e.to_string()))
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_bare_tilde() {
        let home = PathBuf::from("/home/test");
        assert_eq!(
            expand_with_home("~", Some(&home)).unwrap(),
            PathBuf::from("/home/test")
        );
        assert_eq!(
            expand_with_home("~/", Some(&home)).unwrap(),
            PathBuf::from("/home/test")
        );
        assert_eq!(
            expand_with_home("~/projects/foo", Some(&home)).unwrap(),
            PathBuf::from("/home/test/projects/foo")
        );
        assert_eq!(
            expand_with_home("~/projects/foo/", Some(&home)).unwrap(),
            PathBuf::from("/home/test/projects/foo")
        );
    }

    #[test]
    fn absolute_pass_through_with_trailing_slash_trim() {
        let home = PathBuf::from("/home/test");
        assert_eq!(
            expand_with_home("/var/log/", Some(&home)).unwrap(),
            PathBuf::from("/var/log")
        );
        assert_eq!(
            expand_with_home("/", Some(&home)).unwrap(),
            PathBuf::from("/")
        );
        assert_eq!(
            expand_with_home("  /tmp  ", Some(&home)).unwrap(),
            PathBuf::from("/tmp")
        );
    }

    #[test]
    fn empty_is_rejected() {
        let home = PathBuf::from("/home/test");
        assert!(matches!(
            expand_with_home("   ", Some(&home)).unwrap_err(),
            ApiError::BadRequest(_)
        ));
    }

    #[test]
    fn tilde_without_home_errors_internal() {
        assert!(matches!(
            expand_with_home("~/foo", None).unwrap_err(),
            ApiError::Internal(_)
        ));
    }

    #[test]
    fn non_tilde_paths_dont_need_home() {
        assert_eq!(
            expand_with_home("/abs/path", None).unwrap(),
            PathBuf::from("/abs/path")
        );
    }
}
