//! `agentum board` — the planner agent's output surface (D-05).
//!
//! Two subcommands:
//! - `add-goal` → POST /api/board/goals → prints AG-key
//! - `add-card` → POST /api/board → prints AG-key, then POST /api/board/links
//!   for each `--blocks` entry
//!
//! Authentication is read from `credentials.toml` via the trust layer.
//! No token ever appears in process args or env vars (D-08, T-05-01, T-05-02).

pub mod add_card;
pub mod add_goal;
pub mod client;

use anyhow::Result;

use crate::cli::BoardCmd;

/// Dispatch a `BoardCmd` variant to the appropriate subcommand handler.
pub async fn run(cmd: BoardCmd) -> Result<()> {
    match cmd {
        BoardCmd::AddGoal {
            title,
            body,
            workdir,
            profile,
        } => add_goal::run(title, body, workdir, profile).await,
        BoardCmd::AddCard {
            parent_goal,
            title,
            body,
            key,
            blocks,
            lbl,
            profile,
        } => add_card::run(parent_goal, title, body, key, blocks, lbl, profile).await,
    }
}
