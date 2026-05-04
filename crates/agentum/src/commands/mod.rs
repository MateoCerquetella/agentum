pub mod auth;
pub mod doctor;
pub mod down;
pub mod kill;
pub mod ls;
pub mod new;
pub mod serve;
pub mod up;

use agentum_store::Store;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub(crate) async fn open_store() -> Result<(Store, PathBuf)> {
    agentum_store::open_default()
        .await
        .context("failed to open agentum database")
}
