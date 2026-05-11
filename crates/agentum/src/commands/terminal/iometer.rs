//! Sliding-window byte counter for the active WS terminal stream.
//!
//! The TUI talks to the daemon over a bidirectional WebSocket — pane
//! bytes come in as binary frames, keystrokes / resizes go out the
//! other way. The user calls this "the SSH" colloquially even though
//! it's not literally `sshd`. From a "how fast is my pipe" perspective
//! it behaves the same: an interactive byte stream over an encrypted
//! TCP connection. This meter renders that throughput on the status bar.
//!
//! Implementation: a fixed-capacity ring of `(instant, bytes_in,
//! bytes_out)` samples. Old samples beyond `WINDOW` age out lazily on
//! every read. Rates are computed across the surviving span so the
//! display reads true within ~100ms of activity stopping rather than
//! lingering at a stale value.

use std::time::{Duration, Instant};

/// How much history we average over. 1.5s is short enough that bursts
/// (claude streaming a paragraph) read as bursty, and long enough that
/// idle keystrokes don't blink the meter back to 0 between frames.
const WINDOW: Duration = Duration::from_millis(1500);

/// Hard cap on stored samples. Each `record_*` call appends one; over a
/// busy second we see ~30 frames + a couple keystrokes. 256 is far above
/// that ceiling and keeps the ring trivially small in memory.
const MAX_SAMPLES: usize = 256;

#[derive(Clone, Copy)]
struct Sample {
    at: Instant,
    bytes_in: u64,
    bytes_out: u64,
}

pub struct IoMeter {
    samples: Vec<Sample>,
    total_in: u64,
    total_out: u64,
}

impl IoMeter {
    pub fn new() -> Self {
        Self {
            samples: Vec::with_capacity(MAX_SAMPLES),
            total_in: 0,
            total_out: 0,
        }
    }

    pub fn record_in(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        self.push(n as u64, 0);
        self.total_in = self.total_in.saturating_add(n as u64);
    }

    pub fn record_out(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        self.push(0, n as u64);
        self.total_out = self.total_out.saturating_add(n as u64);
    }

    /// Reset both rate samples and lifetime totals. Called when the user
    /// switches sessions so the meter reflects the new stream rather
    /// than carrying credit from the previous one.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.total_in = 0;
        self.total_out = 0;
    }

    /// Bytes/sec received over the sliding window. 0 once the window
    /// has fully drained.
    pub fn rate_in(&self) -> f64 {
        self.rate(|s| s.bytes_in)
    }

    /// Bytes/sec sent over the sliding window.
    pub fn rate_out(&self) -> f64 {
        self.rate(|s| s.bytes_out)
    }

    pub fn total_in(&self) -> u64 {
        self.total_in
    }

    pub fn total_out(&self) -> u64 {
        self.total_out
    }

    fn push(&mut self, bin: u64, bout: u64) {
        let now = Instant::now();
        // Trim before pushing so the cap really caps. Cheap because the
        // ring is bounded and sorted by timestamp.
        let cutoff = now.checked_sub(WINDOW).unwrap_or(now);
        self.samples.retain(|s| s.at >= cutoff);
        if self.samples.len() >= MAX_SAMPLES {
            self.samples.remove(0);
        }
        self.samples.push(Sample {
            at: now,
            bytes_in: bin,
            bytes_out: bout,
        });
    }

    fn rate(&self, pick: impl Fn(&Sample) -> u64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let now = Instant::now();
        let cutoff = now.checked_sub(WINDOW).unwrap_or(now);
        // Sum live samples only.
        let mut total: u64 = 0;
        let mut earliest: Option<Instant> = None;
        for s in &self.samples {
            if s.at < cutoff {
                continue;
            }
            total = total.saturating_add(pick(s));
            earliest = Some(earliest.map_or(s.at, |e| e.min(s.at)));
        }
        if total == 0 {
            return 0.0;
        }
        // Span = max(now - earliest, 1ms) to avoid divide-by-tiny when
        // multiple samples land in the same microsecond. Cap the span
        // at WINDOW so a single recent sample doesn't read as a decade
        // of throughput.
        let span = match earliest {
            Some(t) => now
                .saturating_duration_since(t)
                .max(Duration::from_millis(1)),
            None => return 0.0,
        };
        let span = span.min(WINDOW);
        total as f64 / span.as_secs_f64()
    }
}

impl Default for IoMeter {
    fn default() -> Self {
        Self::new()
    }
}

/// Compact human-readable rate. 0 reads as `—` so an idle pane doesn't
/// scream `0 B/s` at the user. Three significant figures + a left-pad
/// to a constant 7-char width so the chip and its neighbors don't
/// jitter as values flip between `—`, `10 B/s`, and `1.0 K/s`.
pub fn fmt_rate(bps: f64) -> String {
    const WIDTH: usize = 7;
    if bps < 1.0 {
        return format!("{:<WIDTH$}", "—");
    }
    let (val, unit) = if bps < 1024.0 {
        (bps, "B/s")
    } else if bps < 1024.0 * 1024.0 {
        (bps / 1024.0, "K/s")
    } else if bps < 1024.0 * 1024.0 * 1024.0 {
        (bps / (1024.0 * 1024.0), "M/s")
    } else {
        (bps / (1024.0 * 1024.0 * 1024.0), "G/s")
    };
    let raw = if val < 10.0 {
        format!("{val:.1} {unit}")
    } else {
        format!("{val:.0} {unit}")
    };
    format!("{raw:<WIDTH$}")
}

/// Compact human-readable byte total (no `/s`). Used for the lifetime
/// counters displayed when the meter is in `verbose` mode.
pub fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    let (val, unit) = if b < 1024.0 {
        (b, "B")
    } else if b < 1024.0 * 1024.0 {
        (b / 1024.0, "KB")
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        (b / (1024.0 * 1024.0), "MB")
    } else {
        (b / (1024.0 * 1024.0 * 1024.0), "GB")
    };
    if val < 10.0 {
        format!("{val:.1}{unit}")
    } else {
        format!("{val:.0}{unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn empty_meter_reports_zero() {
        let m = IoMeter::new();
        assert_eq!(m.rate_in(), 0.0);
        assert_eq!(m.rate_out(), 0.0);
        assert_eq!(m.total_in(), 0);
        assert_eq!(m.total_out(), 0);
    }

    #[test]
    fn records_separate_in_out_totals() {
        let mut m = IoMeter::new();
        m.record_in(1024);
        m.record_out(64);
        assert_eq!(m.total_in(), 1024);
        assert_eq!(m.total_out(), 64);
        // Rates should be non-zero immediately after a record, even
        // before the window's edge has been reached.
        assert!(m.rate_in() > 0.0);
        assert!(m.rate_out() > 0.0);
    }

    #[test]
    fn rate_drops_after_window_drains() {
        let mut m = IoMeter::new();
        m.record_in(4096);
        assert!(m.rate_in() > 0.0);
        // Sleep just past the window so the only sample ages out.
        sleep(WINDOW + Duration::from_millis(50));
        assert_eq!(m.rate_in(), 0.0);
    }

    #[test]
    fn fmt_rate_scales() {
        assert_eq!(fmt_rate(0.0).trim(), "—");
        assert_eq!(fmt_rate(0.4).trim(), "—");
        assert!(fmt_rate(50.0).contains("B/s"));
        assert!(fmt_rate(2_048.0).contains("K/s"));
        assert!(fmt_rate(5_000_000.0).contains("M/s"));
    }

    #[test]
    fn fmt_rate_is_constant_width() {
        // Chip layout depends on this — the whole point of padding.
        let widths = [
            fmt_rate(0.0).chars().count(),
            fmt_rate(50.0).chars().count(),
            fmt_rate(999.0).chars().count(),
            fmt_rate(2_048.0).chars().count(),
            fmt_rate(15_728_640.0).chars().count(),
            fmt_rate(2_147_483_648.0).chars().count(),
        ];
        assert!(widths.iter().all(|&w| w == 7), "got widths: {widths:?}");
    }

    #[test]
    fn reset_clears_totals_and_rates() {
        let mut m = IoMeter::new();
        m.record_in(1000);
        m.record_out(2000);
        m.reset();
        assert_eq!(m.total_in(), 0);
        assert_eq!(m.total_out(), 0);
        assert_eq!(m.rate_in(), 0.0);
        assert_eq!(m.rate_out(), 0.0);
    }
}
