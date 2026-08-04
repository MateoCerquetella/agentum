//! `/api/host/metrics` — live host CPU / RAM snapshot for the dashboard.
//!
//! Two surfaces:
//!
//! - `GET /api/host/metrics` — point-in-time JSON. Cheap; refreshes
//!   `sysinfo`'s system state on each call. Suitable for an initial
//!   render before the WS catches up.
//! - `host.metrics` events on the broadcast bus — fired every
//!   [`HOST_METRICS_INTERVAL`] from a background ticker started by
//!   [`spawn_ticker`]. The existing `/api/events` WS fans these out to
//!   every connected dashboard so the UI gets a smooth real-time
//!   stream without each tab opening its own poller.

use std::time::Duration;

use agentum_core::Event;
use axum::Json;
use axum::Router;
use axum::routing::get;
use serde::Serialize;
use serde_json::json;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::sync::broadcast;

use crate::AppState;

/// How often the ticker pushes `host.metrics` onto the bus. 2 s is
/// enough resolution for a sparkline at 60 px width while keeping the
/// per-day event volume well under what the bus capacity tolerates.
pub const HOST_METRICS_INTERVAL: Duration = Duration::from_secs(2);

/// Wire shape of a single sample. `cpu_pct` averages all logical cores
/// (matches what most ops dashboards display); per-core values land in
/// `cores` for clients that want to render a heatmap.
#[derive(Serialize, Clone, Debug)]
pub struct HostMetrics {
    /// Aggregate CPU utilisation in percent (0-100).
    pub cpu_pct: f32,
    /// Per-core utilisation, in the order `sysinfo` reports them.
    pub cores: Vec<f32>,
    /// Used RAM in bytes (i.e. `total - available`, not `total - free`).
    pub mem_used: u64,
    pub mem_total: u64,
    /// Used swap in bytes.
    pub swap_used: u64,
    pub swap_total: u64,
    /// Logical CPU count, surfaced so the client can dimension its
    /// per-core grid without a second roundtrip.
    pub cpu_count: usize,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/host/metrics", get(metrics))
}

async fn metrics() -> Json<HostMetrics> {
    Json(sample_now())
}

/// Refresh + sample a `sysinfo::System` snapshot. CPU% requires two
/// reads spaced by `MINIMUM_CPU_UPDATE_INTERVAL` (~200 ms); we wait for
/// it inside this fn so the very first GET still returns sensible
/// numbers instead of a uniform 0%.
fn sample_now() -> HostMetrics {
    let refresh = RefreshKind::new()
        .with_cpu(CpuRefreshKind::new().with_cpu_usage())
        .with_memory(MemoryRefreshKind::new().with_ram().with_swap());
    let mut sys = System::new_with_specifics(refresh);
    // First read primes per-core counters; the second read after the
    // mandated minimum interval is the one that actually has %.
    sys.refresh_cpu_specifics(CpuRefreshKind::new().with_cpu_usage());
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_cpu_specifics(CpuRefreshKind::new().with_cpu_usage());
    sys.refresh_memory();

    let cores: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
    let cpu_pct = if cores.is_empty() {
        0.0
    } else {
        cores.iter().sum::<f32>() / cores.len() as f32
    };

    HostMetrics {
        cpu_pct,
        cpu_count: cores.len(),
        cores,
        mem_used: sys.used_memory(),
        mem_total: sys.total_memory(),
        swap_used: sys.used_swap(),
        swap_total: sys.total_swap(),
    }
}

/// Spawn the background ticker that pushes `host.metrics` events onto
/// the bus. Call once from `serve()`. The ticker holds its own
/// long-lived `System` to avoid re-allocating the per-CPU buffers each
/// tick — sysinfo's whole reason for being a stateful type.
pub fn spawn_ticker(bus: broadcast::Sender<Event>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let refresh = RefreshKind::new()
            .with_cpu(CpuRefreshKind::new().with_cpu_usage())
            .with_memory(MemoryRefreshKind::new().with_ram().with_swap());
        let mut sys = System::new_with_specifics(refresh);
        // Prime CPU counters once so the first emitted tick has %.
        sys.refresh_cpu_specifics(CpuRefreshKind::new().with_cpu_usage());

        loop {
            // An async sleep makes the long-lived ticker cancellable when an
            // embedded server's runtime shuts down. A blocking infinite loop
            // here prevented the terminal application from exiting because
            // Tokio waits for spawn_blocking tasks during runtime teardown.
            tokio::time::sleep(HOST_METRICS_INTERVAL).await;
            // No subscribers → don't bother sampling. `receiver_count`
            // is cheap (atomic load); skipping the syscall when no
            // dashboards are open keeps the daemon at zero idle cost.
            if bus.receiver_count() == 0 {
                continue;
            }
            sys.refresh_cpu_specifics(CpuRefreshKind::new().with_cpu_usage());
            sys.refresh_memory();
            let cores: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
            let cpu_pct = if cores.is_empty() {
                0.0
            } else {
                cores.iter().sum::<f32>() / cores.len() as f32
            };
            let snap = HostMetrics {
                cpu_pct,
                cpu_count: cores.len(),
                cores,
                mem_used: sys.used_memory(),
                mem_total: sys.total_memory(),
                swap_used: sys.used_swap(),
                swap_total: sys.total_swap(),
            };
            let _ = bus.send(Event::new("host.metrics").with_payload(json!(snap)));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ticker_task_is_cancellable() {
        let (bus, _) = broadcast::channel(1);
        let handle = spawn_ticker(bus);
        tokio::task::yield_now().await;

        handle.abort();
        let result = tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("ticker cancellation timed out")
            .expect_err("aborted ticker unexpectedly completed normally");
        assert!(result.is_cancelled());
    }
}
