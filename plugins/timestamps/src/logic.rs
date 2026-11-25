//! What a turn cost, pinned to its last message, and how a time reads.
use agentum_proto::{Event, EventKind};
use std::collections::BTreeMap;

/// The seq of each turn's last assistant message → that turn's cost (turns with a known cost).
pub fn turn_costs(events: &[Event]) -> BTreeMap<u64, f64> {
    let mut out = BTreeMap::new();
    let mut last = None;
    for e in events {
        match &e.kind {
            EventKind::TurnStarted { .. } => last = None,
            EventKind::Message { .. } => last = Some(e.seq),
            EventKind::TurnDone { cost_usd, .. } => {
                if let (Some(seq), Some(c)) = (last.take(), cost_usd.filter(|c| *c > 0.0)) {
                    out.insert(seq, c);
                }
            }
            _ => {}
        }
    }
    out
}

/// "14:05", or "2:05 PM" on a 12-hour clock.
pub fn hm(h: u32, m: u32, twelve: bool) -> String {
    if !twelve {
        return format!("{h:02}:{m:02}");
    }
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h12}:{m:02} {}", if h < 12 { "AM" } else { "PM" })
}

/// "$0.004", "$0.25", "$1.40".
pub fn money(c: f64) -> String {
    if c < 0.1 { format!("${c:.3}") } else { format!("${c:.2}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_proto::StopReason;

    fn ev(seq: u64, kind: EventKind) -> Event {
        Event { seq, ts: seq as i64, kind }
    }
    fn done(seq: u64, cost: Option<f64>) -> Event {
        ev(seq, EventKind::TurnDone { turn: 1, stop: StopReason::End, usage: None, cost_usd: cost, duration_ms: 0 })
    }
    fn msg(seq: u64) -> Event {
        ev(seq, EventKind::Message { item: format!("m{seq}"), text: "hi".into() })
    }

    #[test]
    fn cost_goes_on_the_turns_last_message() {
        let evs = vec![
            ev(1, EventKind::TurnStarted { turn: 1 }),
            msg(2),
            msg(3),
            done(4, Some(0.012)),
            ev(5, EventKind::TurnStarted { turn: 2 }),
            done(6, Some(0.5)), // no message in this turn
            ev(7, EventKind::TurnStarted { turn: 3 }),
            msg(8),
            done(9, None),
            msg(10),
            done(11, Some(0.2)),
        ];
        let c = turn_costs(&evs);
        assert_eq!(c.get(&3), Some(&0.012));
        assert_eq!(c.get(&2), None);
        assert_eq!(c.get(&8), None);
        assert_eq!(c.get(&10), Some(&0.2));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn clocks() {
        assert_eq!(hm(14, 5, false), "14:05");
        assert_eq!(hm(14, 5, true), "2:05 PM");
        assert_eq!(hm(0, 7, true), "12:07 AM");
        assert_eq!(hm(12, 0, true), "12:00 PM");
        assert_eq!(money(0.0042), "$0.004");
        assert_eq!(money(1.4), "$1.40");
    }
}
