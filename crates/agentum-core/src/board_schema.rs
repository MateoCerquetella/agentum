//! Per-status required-field matrix for board cards.
//!
//! Slice 1: matrix is a compile-time const. Custom columns (anything not
//! in the match arms below) bypass the gate. See
//! `.planning/specs/2026-05-19-typed-kanban-card-schemas.md` for the
//! settled decisions.

/// Fields a card must satisfy to *enter* the column. The `done` OR-clause
/// (`session_id` OR `>=1 comment`) is encoded as a single synthetic field
/// `SessionOrComment`; the validator resolves the disjunction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredField {
    Title,
    Lbl,
    Workdir,
    Tool,
    ClaimedBy,
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

/// Returns `Err(missing_keys)` on gate failure, `Ok(())` on pass or
/// custom-column passthrough. `&'static str` keys map directly into
/// the JSON `missing` array — no allocation.
pub fn validate_transition(
    target_status: &str,
    ctx: &TransitionCtx<'_>,
) -> Result<(), Vec<&'static str>> {
    let required = required_fields_for(target_status);
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
}
