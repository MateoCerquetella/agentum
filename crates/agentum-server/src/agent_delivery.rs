//! Shared delivery primitives for interactive terminal agents.
//!
//! These helpers belong to the general session runtime. SDD providers use
//! isolated process adapters and never depend on an interactive terminal.

use std::time::{Duration, Instant};

use agentum_core::{Event, HostKind, LOCAL_HOST_ID, Session, Status};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::AppState;

const AGENT_BOOT_DELAY: Duration = Duration::from_secs(3);
const SUBMIT_DELAY: Duration = Duration::from_millis(600);

/// How a wait for one interactive agent turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettleOutcome {
    Settled,
    Crashed,
    TimedOut,
}

fn repl_pane_is_ready(tool: &str, pane: &str) -> bool {
    if pane.contains("bypass permissions on")
        || pane.contains("shift+tab to cycle")
        || pane.contains("? for shortcuts")
    {
        return true;
    }
    tool.eq_ignore_ascii_case("codex")
        && pane.contains("/model to change")
        && !pane.contains("starting mcp server")
}

async fn await_repl_ready(state: &AppState, session: &Session) -> bool {
    let host = match state
        .store
        .get_host(session.host_id.unwrap_or(LOCAL_HOST_ID))
        .await
    {
        Ok(Some(host)) => host,
        _ => return false,
    };
    if !matches!(host.kind, HostKind::Local) {
        tokio::time::sleep(AGENT_BOOT_DELAY).await;
        return false;
    }
    let target = session
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&session.name));
    let mut trusted = false;
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(700)).await;
        let pane = match crate::host_runtime::capture_pane_visible(&host, &target).await {
            Ok(pane) => pane.to_lowercase(),
            Err(_) => continue,
        };
        if !trusted && (pane.contains("trust this folder") || pane.contains("do you trust")) {
            let _ = crate::host_runtime::send_keys(&host, &target, "", true).await;
            trusted = true;
            tokio::time::sleep(Duration::from_millis(900)).await;
            continue;
        }
        if repl_pane_is_ready(&session.tool, &pane) {
            tokio::time::sleep(Duration::from_millis(400)).await;
            return true;
        }
    }
    false
}

/// Wait for the interactive composer, paste bytes in bounded tmux chunks, and
/// submit with a separate Enter so multiline prompts execute reliably.
pub(crate) async fn inject_prompt(
    state: &AppState,
    session: &Session,
    prompt: &str,
) -> anyhow::Result<bool> {
    let ready = await_repl_ready(state, session).await;
    let host = state
        .store
        .get_host(session.host_id.unwrap_or(LOCAL_HOST_ID))
        .await?
        .ok_or_else(|| anyhow::anyhow!("session host missing"))?;
    let target = session
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&session.name));
    crate::host_runtime::send_bytes(&host, &target, prompt.as_bytes())
        .await
        .map_err(|error| anyhow::anyhow!("send prompt failed: {error}"))?;
    tokio::time::sleep(SUBMIT_DELAY).await;
    crate::host_runtime::send_keys(&host, &target, "", true)
        .await
        .map_err(|error| anyhow::anyhow!("submit Enter failed: {error}"))?;
    Ok(ready)
}

/// Persist the terminal state before removing its pane so the watchdog cannot
/// misclassify an intentional stop as a crash.
pub(crate) async fn teardown_session(state: &AppState, session: &Session) {
    let host = match state
        .store
        .get_host(session.host_id.unwrap_or(LOCAL_HOST_ID))
        .await
    {
        Ok(Some(host)) => host,
        _ => return,
    };
    if matches!(host.kind, HostKind::Local | HostKind::Ssh { .. }) {
        let target = session
            .tmux_target
            .clone()
            .unwrap_or_else(|| agentum_tmux::target_for(&session.name));
        let _ = state
            .store
            .update_status_and_target(session.id, Status::Stopped, None)
            .await;
        let _ = crate::host_runtime::kill_session(&host, &target).await;
    }
}

/// Wait for an idle/finished event after a grace period, or classify a
/// terminal stop/crash and timeout explicitly.
pub(crate) async fn wait_for_settle(
    bus: &broadcast::Sender<Event>,
    session_id: Uuid,
    grace: Duration,
    timeout: Duration,
) -> SettleOutcome {
    let mut receiver = bus.subscribe();
    let start = Instant::now();
    let mut settled_early = false;
    loop {
        if settled_early && start.elapsed() >= grace {
            return SettleOutcome::Settled;
        }
        let Some(remaining) = timeout.checked_sub(start.elapsed()) else {
            return SettleOutcome::TimedOut;
        };
        let wait = if settled_early {
            grace
                .checked_sub(start.elapsed())
                .unwrap_or(Duration::ZERO)
                .min(remaining)
        } else {
            remaining
        };
        match tokio::time::timeout(wait, receiver.recv()).await {
            Err(_) if start.elapsed() >= timeout => return SettleOutcome::TimedOut,
            Err(_) => continue,
            Ok(Ok(event)) if event.session_id != Some(session_id) => continue,
            Ok(Ok(event)) => match event.kind.as_str() {
                "agent.awaiting_input" | "agent.finished" => {
                    if event
                        .payload
                        .get("initial")
                        .and_then(|value| value.as_bool())
                        == Some(true)
                    {
                        continue;
                    }
                    if start.elapsed() >= grace {
                        return SettleOutcome::Settled;
                    }
                    settled_early = true;
                }
                "session.crashed" | "session.stopped" => return SettleOutcome::Crashed,
                _ => {}
            },
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => return SettleOutcome::Settled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_detection_is_tool_aware() {
        assert!(repl_pane_is_ready("claude", "? for shortcuts"));
        assert!(repl_pane_is_ready("codex", "/model to change"));
        assert!(!repl_pane_is_ready(
            "codex",
            "starting mcp server /model to change"
        ));
    }
}
