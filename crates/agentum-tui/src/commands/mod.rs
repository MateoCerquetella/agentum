pub mod terminal;
pub mod up;

use agentum_store::Store;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub(crate) async fn open_store() -> Result<(Store, PathBuf)> {
    agentum_store::open_default()
        .await
        .context("failed to open agentum database")
}
