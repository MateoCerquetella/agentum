//! Per-status required-field matrix for board cards.
//!
//! Slice 1: matrix is a compile-time const. Slice 2 adds per-server
//! overrides on top — see `agentum-server::rules` for the lookup glue.
//! Custom columns (anything not in the match arms below) bypass the
//! const gate but can be opted in via overrides.

use serde::{Deserialize, Serialize};

/// Fields a card must satisfy to *enter* the column. The `done` OR-clause
/// (`session_id` OR `>=1 comment`) is encoded as a single synthetic field
/// `SessionOrComment`; the validator resolves the disjunction.
///
/// Serde renames are explicit per variant so the wire vocabulary matches
/// `as_missing_key()` exactly — `rename_all = "snake_case"` would not
/// produce `session_id_or_comment` from `SessionOrComment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequiredField {
    #[serde(rename = "title")]
    Title,
    #[serde(rename = "lbl")]
    Lbl,
    #[serde(rename = "workdir")]
    Workdir,
    #[serde(rename = "tool")]
    Tool,
    #[serde(rename = "claimed_by")]
    ClaimedBy,
    #[serde(rename = "session_id_or_comment")]
    SessionOrComment,
}

impl RequiredField {
    /// JSON-array key for the `missing[]` payload returned on a 400.
    /// `&'static str` so callers don't allocate.
    pub fn as_missing_key(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Lbl => "lbl",
            Self::Workdir => "workdir",
            Self::Tool => "tool",
            Self::ClaimedBy => "claimed_by",
            Self::SessionOrComment => "session_id_or_comment",
        }
    }

    /// Inverse of [`as_missing_key`]. Returns `None` for unknown strings so
    /// the store can skip-and-warn on rows that pin a removed variant
    /// (forward-compat policy — see the architecture file's risk #3).
    pub fn from_missing_key(s: &str) -> Option<Self> {
        match s {
            "title" => Some(Self::Title),
            "lbl" => Some(Self::Lbl),
            "workdir" => Some(Self::Workdir),
            "tool" => Some(Self::Tool),
            "claimed_by" => Some(Self::ClaimedBy),
            "session_id_or_comment" => Some(Self::SessionOrComment),
            _ => None,
        }
    }
}

/// Snapshot the validator needs. Built by the handler from the existing
/// row (PATCH) or zeroed (POST), then merged with the incoming patch.
#[derive(Debug, Clone, Default)]
pub struct TransitionCtx<'a> {
    pub title: Option<&'a str>,
    pub lbl: Option<&'a str>,
    pub workdir: Option<&'a str>,
    pub tool: Option<&'a str>,
    pub claimed_by: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub has_comment: bool,
}

/// Empty slice => no gate (custom columns passthrough).
pub fn required_fields_for(status: &str) -> &'static [RequiredField] {
    use RequiredField::*;
    match status {
        "todo" => &[Title, Lbl],
        "doing" => &[Title, Lbl, Workdir, Tool, ClaimedBy],
        "done" => &[Title, Lbl, SessionOrComment],
        _ => &[],
    }
}

/// Validate `ctx` against an arbitrary required-field slice. The caller
/// chooses the source (const matrix or DB override); this function knows
/// nothing about column names. Returns `Err(missing_keys)` on gate failure.
pub fn validate_against(
    required: &[RequiredField],
    ctx: &TransitionCtx<'_>,
) -> Result<(), Vec<&'static str>> {
    if required.is_empty() {
        return Ok(());
    }

    let mut missing: Vec<&'static str> = Vec::new();
    for field in required {
        let present = match field {
            RequiredField::Title => is_set(ctx.title),
            RequiredField::Lbl => is_set(ctx.lbl),
            RequiredField::Workdir => is_set(ctx.workdir),
            RequiredField::Tool => is_set(ctx.tool),
            RequiredField::ClaimedBy => is_set(ctx.claimed_by),
            // OR-clause: either an attached session OR at least one
            // comment on the parent row. Preserves the manual-close path
            // ("won't fix", "dup of AG-12") for cards that never ran an
            // agent session.
            RequiredField::SessionOrComment => is_set(ctx.session_id) || ctx.has_comment,
        };
        if !present {
            missing.push(field.as_missing_key());
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Returns `Err(missing_keys)` on gate failure, `Ok(())` on pass or
/// custom-column passthrough. Thin shim over [`validate_against`] that
/// pins the required slice to the slice-1 const matrix — kept so slice-1
/// callsites and tests don't change.
pub fn validate_transition(
    target_status: &str,
    ctx: &TransitionCtx<'_>,
) -> Result<(), Vec<&'static str>> {
    validate_against(required_fields_for(target_status), ctx)
}

/// Treat `None` and whitespace-only strings as absent. Trim is the
/// minimum sanity check — the rest is up to the wire layer.
fn is_set(v: Option<&str>) -> bool {
    matches!(v, Some(s) if !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_todo_pass<'a>() -> TransitionCtx<'a> {
        TransitionCtx {
            title: Some("t"),
            lbl: Some("feat"),
            ..Default::default()
        }
    }

    #[test]
    fn todo_pass() {
        assert!(validate_transition("todo", &ctx_todo_pass()).is_ok());
    }

    #[test]
    fn todo_fail_missing_lbl() {
        let ctx = TransitionCtx {
            title: Some("t"),
            ..Default::default()
        };
        let err = validate_transition("todo", &ctx).unwrap_err();
        assert_eq!(err, vec!["lbl"]);
    }

    #[test]
    fn doing_pass() {
        let ctx = TransitionCtx {
            title: Some("t"),
            lbl: Some("feat"),
            workdir: Some("/tmp"),
            tool: Some("claude"),
            claimed_by: Some("alice"),
            ..Default::default()
        };
        assert!(validate_transition("doing", &ctx).is_ok());
    }

    #[test]
    fn doing_fail_missing_three_fields_in_order() {
        let ctx = TransitionCtx {
            title: Some("t"),
            lbl: Some("feat"),
            ..Default::default()
        };
        let err = validate_transition("doing", &ctx).unwrap_err();
        // Order matters: the validator walks `required_fields_for` left
        // to right, so the missing[] array always has a deterministic
        // shape the dashboard can rely on.
        assert_eq!(err, vec!["workdir", "tool", "claimed_by"]);
    }

    #[test]
    fn done_pass_via_session() {
        let ctx = TransitionCtx {
            title: Some("t"),
            lbl: Some("feat"),
            session_id: Some("abc"),
            ..Default::default()
        };
        assert!(validate_transition("done", &ctx).is_ok());
    }

    #[test]
    fn done_pass_via_comment() {
        let ctx = TransitionCtx {
            title: Some("t"),
            lbl: Some("feat"),
            has_comment: true,
            ..Default::default()
        };
        assert!(validate_transition("done", &ctx).is_ok());
    }

    #[test]
    fn done_fail_neither_session_nor_comment() {
        let ctx = TransitionCtx {
            title: Some("t"),
            lbl: Some("feat"),
            ..Default::default()
        };
        let err = validate_transition("done", &ctx).unwrap_err();
        assert_eq!(err, vec!["session_id_or_comment"]);
    }

    #[test]
    fn custom_column_passthrough() {
        // Any status not in the matrix returns Ok regardless of ctx —
        // user-introduced columns like `blocked` or `review` keep working
        // without a code change.
        let ctx = TransitionCtx::default();
        assert!(validate_transition("blocked", &ctx).is_ok());
        assert!(validate_transition("review", &ctx).is_ok());
        assert!(validate_transition("anything-goes", &ctx).is_ok());
    }

    #[test]
    fn whitespace_only_is_absent() {
        // Defensive: `Some("   ")` is treated as missing so a client
        // can't sneak past the gate with an empty payload.
        let ctx = TransitionCtx {
            title: Some("   "),
            lbl: Some("feat"),
            ..Default::default()
        };
        let err = validate_transition("todo", &ctx).unwrap_err();
        assert_eq!(err, vec!["title"]);
    }

    #[test]
    fn from_missing_key_roundtrip_all_variants() {
        // Every known variant must round-trip through the
        // as_missing_key / from_missing_key pair — the store relies on
        // this symmetry to parse JSON rows back into typed enums.
        for f in [
            RequiredField::Title,
            RequiredField::Lbl,
            RequiredField::Workdir,
            RequiredField::Tool,
            RequiredField::ClaimedBy,
            RequiredField::SessionOrComment,
        ] {
            assert_eq!(RequiredField::from_missing_key(f.as_missing_key()), Some(f));
        }
    }

    #[test]
    fn from_missing_key_unknown_is_none() {
        // Unknown strings return None so the store can skip-and-warn
        // instead of failing the whole row.
        assert!(RequiredField::from_missing_key("wat").is_none());
        assert!(RequiredField::from_missing_key("").is_none());
        assert!(RequiredField::from_missing_key("TITLE").is_none()); // case-sensitive
    }

    #[test]
    fn serde_roundtrip_uses_wire_strings() {
        // The wire format must produce `"session_id_or_comment"`, not
        // `"SessionOrComment"` (the variant ident) or anything else.
        let json = serde_json::to_string(&RequiredField::SessionOrComment).unwrap();
        assert_eq!(json, "\"session_id_or_comment\"");
        let back: RequiredField = serde_json::from_str("\"title\"").unwrap();
        assert_eq!(back, RequiredField::Title);
    }

    #[test]
    fn validate_against_with_explicit_slice() {
        // Smoke: pass an explicit required slice that diverges from the
        // const matrix and confirm the validator honors it. Mirrors how
        // the override path in slice 2 calls this.
        let ctx = TransitionCtx {
            title: Some("t"),
            lbl: Some("feat"),
            ..Default::default()
        };
        // Empty slice => unconditional pass.
        assert!(validate_against(&[], &ctx).is_ok());
        // Title-only requirement => pass.
        assert!(validate_against(&[RequiredField::Title], &ctx).is_ok());
        // Demand workdir on a ctx that has none => fail.
        let err = validate_against(&[RequiredField::Workdir], &ctx).unwrap_err();
        assert_eq!(err, vec!["workdir"]);
    }
}
