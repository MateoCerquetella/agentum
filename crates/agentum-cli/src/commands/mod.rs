pub mod auth;
pub mod board;
pub mod clip_agent;
pub mod config;
pub mod doctor;
pub mod down;
pub mod hosts;
pub mod keys;
pub mod kill;
pub mod ls;
pub mod new;
pub mod open;
pub mod orchestration;
pub mod profiles;
pub mod rm;
pub mod send;
pub mod serve;
pub mod status;
pub mod tail;
pub mod terminal_control;
pub mod worktree;
pub mod terminal;
pub mod uninstall;
pub mod up;
pub mod update;

use agentum_store::Store;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub(crate) async fn open_store() -> Result<(Store, PathBuf)> {
    agentum_store::open_default()
        .await
        .context("failed to open agentum database")
}
