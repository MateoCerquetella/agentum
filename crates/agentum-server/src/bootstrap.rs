//! First-run bootstrap PIN.
//!
//! When `agentum serve` boots and the user table is empty, we generate a
//! one-time numeric PIN and print it to the host TTY. The dashboard's
//! registration form must echo it back via `X-Bootstrap-PIN` (or a JSON
//! field — the route accepts both). This closes the LAN race where the
//! first attacker to hit `/api/auth/register` claims the admin slot.
//!
//! The PIN lives in process memory only. It's consumed (zeroed) on the
//! first successful registration. If the operator restarts the server
//! before registering, a fresh PIN is printed.

use std::sync::Mutex;

use rand::Rng;

/// 8-digit decimal PIN. Numeric-only is friendlier for typing on a phone
/// than base32, and 10^8 ≈ 27 bits — fine for a one-shot single-use secret
/// that exists for ~minutes.
pub fn generate_pin() -> String {
    // Zero-padded so it's always 8 chars; humans tend to drop leading zeros.
    format!("{:08}", rand::rng().random_range(0..100_000_000u32))
}

/// Process-memory holder for the active bootstrap PIN. `None` means
/// bootstrap is closed (a user already exists, or registration completed
/// this boot).
#[derive(Default)]
pub struct BootstrapPin {
    inner: Mutex<Option<String>>,
}

impl BootstrapPin {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the active PIN (overwrites any previous value).
    pub fn set(&self, pin: String) {
        *self.inner.lock().expect("pin mutex poisoned") = Some(pin);
    }

    /// Wipe the PIN — call this after first-user registration succeeds,
    /// or when the user table becomes non-empty by other means.
    pub fn clear(&self) {
        *self.inner.lock().expect("pin mutex poisoned") = None;
    }

    /// Constant-time compare against `candidate`. Returns true only when
    /// a PIN is set and matches exactly.
    pub fn verify(&self, candidate: &str) -> bool {
        let guard = self.inner.lock().expect("pin mutex poisoned");
        let Some(active) = guard.as_deref() else {
            return false;
        };
        constant_time_eq(active.as_bytes(), candidate.as_bytes())
    }

    /// True when bootstrap is currently armed (a PIN is set).
    pub fn is_armed(&self) -> bool {
        self.inner.lock().expect("pin mutex poisoned").is_some()
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_is_eight_digits() {
        for _ in 0..50 {
            let p = generate_pin();
            assert_eq!(p.len(), 8);
            assert!(p.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn verify_matches_only_when_armed() {
        let b = BootstrapPin::new();
        assert!(!b.is_armed());
        assert!(!b.verify("12345678"));

        b.set("12345678".into());
        assert!(b.is_armed());
        assert!(b.verify("12345678"));
        assert!(!b.verify("12345679"));
        assert!(!b.verify("1234567"));
        assert!(!b.verify(""));

        b.clear();
        assert!(!b.is_armed());
        assert!(!b.verify("12345678"));
    }
}
