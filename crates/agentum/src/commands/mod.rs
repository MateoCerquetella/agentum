pub mod auth;
pub mod config;
pub mod doctor;
pub mod down;
pub mod hosts;
pub mod keys;
pub mod kill;
pub mod ls;
pub mod new;
pub mod open;
pub mod rm;
pub mod send;
pub mod serve;
pub mod tail;
pub mod terminal;
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
