//! `agentum board add-goal` — create a goal card and print its AG-key.
//!
//! This is the planner agent's output surface for goal-level cards (D-05).
//! The printed AG-key on stdout is what the planner agent parses from its
//! pane scrollback to chain subsequent `add-card` calls.

use anyhow::Result;

use super::client::BoardClient;

/// Create a goal card on the board via the named profile's daemon.
///
/// Prints the new card's AG-key to stdout (one line, nothing else) so the
/// planner agent can capture it from its pane scrollback. All human-readable
/// messages go to stderr.
pub async fn run(
    title: String,
    body: Option<String>,
    workdir: Option<String>,
    profile: String,
) -> Result<()> {
    let client = BoardClient::new(&profile)?;
    let resp = client
        .post_goal(&title, body.as_deref(), workdir.as_deref())
        .await?;

    // The server wraps the created item under a `goal` key in the response.
    let key = resp
        .get("goal")
        .and_then(|g| g.get("key"))
        .and_then(|k| k.as_str())
        .ok_or_else(|| anyhow::anyhow!("server response missing .goal.key: {resp}"))?;

    println!("{key}");
    Ok(())
}
