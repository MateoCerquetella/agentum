//! `agentum board add-card` — create a card under a goal and print its AG-key.
//!
//! This is the planner agent's output surface for execution cards (D-05).
//! The `--blocks` flag accepts a comma-separated list of symbolic keys that
//! this card must finish before its dependents can start.
//!
//! Forward-reference note (D-06): the planner is expected to emit dependency
//! targets before their dependents in v1. If `--blocks` references a key not
//! yet created, this CLI exits non-zero (code 5) and the planner agent should
//! retry after creating the target. A real buffered resolver is deferred to v2.

use anyhow::Result;

use super::client::{BoardClient, validate_symbolic_key};

/// Create an execution card under a goal.
///
/// The `key` is a short symbolic identifier used in `--blocks` references.
/// It gets prepended to the body as `"key: <key>\n\n"` so the server's
/// symbolic-key resolution (routes/board_links.rs::resolve_key) can find it
/// by scanning the body prefix without a separate DB column.
pub async fn run(
    parent_goal: String,
    title: String,
    body: Option<String>,
    key: String,
    blocks: Option<String>,
    lbl: Option<String>,
    profile: String,
) -> Result<()> {
    // Validate the card's own key before any network call (T-05-04).
    validate_symbolic_key(&key)?;

    // Parse and validate each --blocks entry up front. This surfaces malformed
    // keys before we've created the card, so the planner doesn't need to
    // clean up a partially-committed state.
    let blocks_keys: Vec<String> = blocks
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();
    for bk in &blocks_keys {
        validate_symbolic_key(bk)?;
    }

    let client = BoardClient::new(&profile)?;

    let parent_id = client.resolve_parent_goal_id(&parent_goal).await?;

    let body_with_key = build_card_body(&key, body.as_deref());

    let payload = serde_json::json!({
        "title": title,
        "body": body_with_key,
        "lbl": lbl,
        "status": "todo",
        "parent_goal_id": parent_id,
    });

    let resp = client.post_board_item(payload).await?;

    let new_key = resp
        .get("key")
        .and_then(|k| k.as_str())
        .ok_or_else(|| anyhow::anyhow!("server response missing .key: {resp}"))?;

    // Post dependency links. Each post_link_symbolic call exits 5 if the
    // server can't resolve `blocked_key` — the planner reads the exit code
    // and retries after creating the missing target (D-06).
    for blocked_key in &blocks_keys {
        client
            .post_link_symbolic(parent_id, &key, blocked_key, "blocks")
            .await?;
    }

    println!("{new_key}");
    Ok(())
}

/// Build the card body with the mandatory `key: <key>` prefix.
///
/// The `key: <key>\n\n` prefix is mandatory; the server's symbolic-key
/// resolution in routes/board_links.rs::resolve_key reads it by scanning the
/// first line of the body. Without the prefix, `--blocks` references from
/// other cards to this one cannot be resolved server-side.
fn build_card_body(key: &str, body: Option<&str>) -> String {
    match body {
        Some(b) if !b.is_empty() => format!("key: {key}\n\n{b}"),
        _ => format!("key: {key}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_card_body_with_body_includes_blank_line_separator() {
        let result = build_card_body("foo", Some("the body"));
        assert_eq!(result, "key: foo\n\nthe body");
    }

    #[test]
    fn build_card_body_without_body_emits_key_line_only() {
        let result = build_card_body("foo", None);
        assert_eq!(result, "key: foo\n");
    }

    #[test]
    fn build_card_body_with_empty_body_emits_key_line_only() {
        // An empty string body should behave like None — no blank separator
        // dangling at the end.
        let result = build_card_body("bar", Some(""));
        assert_eq!(result, "key: bar\n");
    }
}
