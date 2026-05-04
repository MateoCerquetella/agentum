use std::net::SocketAddr;

use agentum_core::Status;
use agentum_server::ServeOptions;
use anyhow::Result;

pub async fn run(addr: SocketAddr, cert_addr: SocketAddr, tls: bool, no_resume: bool) -> Result<()> {
    let (store, db_path) = super::open_store().await?;
    tracing::info!(?db_path, %addr, %cert_addr, tls, "store opened");

    // Boot everything: resume stopped/idle sessions that have a known tool.
    if !no_resume {
        resume_sessions(&store).await;
    }

    agentum_server::serve(
        ServeOptions {
            addr,
            cert_addr,
            tls,
        },
        store,
    )
    .await
}

/// Bring up any sessions that aren't currently running.
async fn resume_sessions(store: &agentum_store::Store) {
    let sessions = match store.list_sessions(None).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("could not list sessions for auto-resume: {e}");
            return;
        }
    };

    let to_resume: Vec<_> = sessions
        .into_iter()
        .filter(|s| matches!(s.status, Status::Idle | Status::Stopped))
        .collect();

    if to_resume.is_empty() {
        return;
    }

    tracing::info!(count = to_resume.len(), "resuming sessions");

    for session in to_resume {
        let name = session.name.clone();
        match crate::commands::up::run(name.clone()).await {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(session = %name, "could not resume: {e}");
            }
        }
    }
}
