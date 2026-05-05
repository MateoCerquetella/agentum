//! In-memory per-IP rate limiter for sensitive auth endpoints.
//!
//! Token-bucket-ish: each key (peer IP) gets at most N attempts in W seconds.
//! Once over the limit, further attempts return `Retry::Denied(retry_after)`
//! until the window slides off.
//!
//! Single-process, single-tenant — sufficient for the threat model
//! (defeating online password guessing). Distributed setups would need a
//! shared store; agentum doesn't run in those.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub enum Decision {
    Allowed,
    Denied { retry_after: Duration },
}

pub struct RateLimiter {
    capacity: usize,
    window: Duration,
    inner: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new(capacity: usize, window: Duration) -> Self {
        Self {
            capacity,
            window,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Record an attempt and return whether it's allowed. Each call
    /// counts whether allowed or denied — the limiter is meant to be hit
    /// once per request.
    pub fn check(&self, key: IpAddr) -> Decision {
        let now = Instant::now();
        let mut map = self.inner.lock().expect("ratelimit mutex poisoned");

        // Opportunistic cleanup of stale keys — keeps memory bounded if
        // many distinct IPs hit the box.
        if map.len() > 1024 {
            map.retain(|_, q| {
                q.back()
                    .is_some_and(|&t| now.duration_since(t) < self.window)
            });
        }

        let q = map.entry(key).or_default();
        while let Some(&front) = q.front() {
            if now.duration_since(front) >= self.window {
                q.pop_front();
            } else {
                break;
            }
        }
        if q.len() >= self.capacity {
            // Retry-After ≈ window minus age of oldest hit
            let oldest = *q.front().expect("non-empty after capacity check");
            let elapsed = now.duration_since(oldest);
            let retry_after = self.window.saturating_sub(elapsed);
            return Decision::Denied { retry_after };
        }
        q.push_back(now);
        Decision::Allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn allows_up_to_capacity_then_denies() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        for _ in 0..3 {
            assert!(matches!(rl.check(ip), Decision::Allowed));
        }
        assert!(matches!(rl.check(ip), Decision::Denied { .. }));
    }

    #[test]
    fn separate_ips_have_independent_buckets() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        let a = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert!(matches!(rl.check(a), Decision::Allowed));
        assert!(matches!(rl.check(b), Decision::Allowed));
        assert!(matches!(rl.check(a), Decision::Denied { .. }));
    }
}
