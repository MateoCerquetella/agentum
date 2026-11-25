//! What a hover card knows beyond the thread's summary: since when the thread is in its state,
//! and the agent's last message at more length than the sidebar's one line.
use agentum_proto::{Event, EventKind, Ms, ThreadState};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Card {
    /// When the latest turn started.
    #[serde(default)]
    pub turn_started: Option<Ms>,
    /// When the latest turn ended (None while it runs).
    #[serde(default)]
    pub turn_done: Option<Ms>,
    /// When the thread entered the state it is in now.
    #[serde(default)]
    pub state_at: Option<Ms>,
    /// The agent's last message, up to 400 characters on one line.
    #[serde(default)]
    pub said: Option<String>,
}

/// Read a thread's recent events (oldest first).
pub fn card_of(events: &[Event]) -> Card {
    let mut c = Card::default();
    let mut last_state = None;
    for e in events {
        match &e.kind {
            EventKind::TurnStarted { .. } => {
                c.turn_started = Some(e.ts);
                c.turn_done = None;
            }
            EventKind::TurnDone { .. } => c.turn_done = Some(e.ts),
            EventKind::State { state } if last_state != Some(*state) => {
                last_state = Some(*state);
                c.state_at = Some(e.ts);
            }
            EventKind::Message { text, .. } if !text.trim().is_empty() => c.said = Some(clip(text, 400)),
            _ => {}
        }
    }
    c
}

/// One line, at most `n` characters.
pub fn clip(s: &str, n: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        return one;
    }
    let mut t: String = one.chars().take(n).collect();
    t.push('…');
    t
}

/// "45s", "12m", "3h 5m", "2d".
pub fn span(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        let (h, m) = (s / 3600, (s % 3600) / 60);
        if m > 0 { format!("{h}h {m}m") } else { format!("{h}h") }
    } else {
        format!("{}d", s / 86_400)
    }
}

/// The card's state line: (the dot's class, the words). `updated_at` stands in when the events
/// don't say.
pub fn state_line(state: ThreadState, c: &Card, updated_at: Ms, now: Ms) -> (&'static str, String) {
    let since = |at: Option<Ms>| span(now - at.unwrap_or(updated_at));
    match state {
        ThreadState::Starting => ("working", "Starting".into()),
        ThreadState::Running => ("working", format!("Working for {}", since(c.turn_started.or(c.state_at)))),
        ThreadState::Waiting => ("asks", format!("Waiting on you for {}", since(c.state_at))),
        ThreadState::Idle => ("idle", format!("Idle · finished {} ago", since(c.turn_done.or(c.state_at)))),
        ThreadState::Error => ("error", format!("Failed {} ago", since(c.turn_done.or(c.state_at)))),
        ThreadState::Interrupted => ("idle", format!("Stopped {} ago", since(c.state_at.or(c.turn_done)))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_proto::StopReason;

    fn ev(seq: u64, ts: Ms, kind: EventKind) -> Event {
        Event { seq, ts, kind }
    }

    #[test]
    fn reads_the_latest_turn() {
        let evs = vec![
            ev(1, 1_000, EventKind::State { state: ThreadState::Running }),
            ev(2, 1_100, EventKind::TurnStarted { turn: 1 }),
            ev(3, 2_000, EventKind::Message { item: "a".into(), text: "First\n\nanswer".into() }),
            ev(4, 3_000, EventKind::TurnDone { turn: 1, stop: StopReason::End, usage: None, cost_usd: None, duration_ms: 0 }),
            ev(5, 3_000, EventKind::State { state: ThreadState::Idle }),
            ev(6, 3_500, EventKind::State { state: ThreadState::Idle }),
        ];
        let c = card_of(&evs);
        assert_eq!(c.turn_started, Some(1_100));
        assert_eq!(c.turn_done, Some(3_000));
        assert_eq!(c.state_at, Some(3_000), "a repeated state keeps the first time");
        assert_eq!(c.said.as_deref(), Some("First answer"));
        assert_eq!(state_line(ThreadState::Idle, &c, 0, 3_000 + 5 * 60_000).1, "Idle · finished 5m ago");

        let mut running = evs.clone();
        running.push(ev(7, 10_000, EventKind::TurnStarted { turn: 2 }));
        running.push(ev(8, 10_000, EventKind::State { state: ThreadState::Running }));
        let c = card_of(&running);
        assert_eq!(c.turn_done, None);
        assert_eq!(state_line(ThreadState::Running, &c, 0, 10_000 + 3_700_000), ("working", "Working for 1h 1m".to_string()));
    }

    #[test]
    fn falls_back_to_updated_at() {
        let c = Card::default();
        assert_eq!(state_line(ThreadState::Interrupted, &c, 1_000, 31_000).1, "Stopped 30s ago");
        assert_eq!(span(3 * 86_400_000), "3d");
        assert_eq!(clip("a  b\nc", 3), "a b…");
    }
}
