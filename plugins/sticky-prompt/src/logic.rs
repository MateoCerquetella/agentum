//! The person's last prompt in a thread.
use agentum_proto::{Event, EventKind, Ms};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Prompt {
    pub seq: u64,
    pub text: String,
    pub ts: Ms,
}

/// The newest message the person sent (queued ones count: they are what was asked last).
pub fn last_prompt(events: &[Event]) -> Option<Prompt> {
    events.iter().rev().find_map(|e| match &e.kind {
        EventKind::User { text, .. } if !text.trim().is_empty() => Some(Prompt { seq: e.seq, text: text.trim().to_string(), ts: e.ts }),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_user_message() {
        let u = |seq: u64, t: &str| Event { seq, ts: seq as i64 * 10, kind: EventKind::User { text: t.into(), images: vec![], queued: false } };
        let m = |seq: u64| Event { seq, ts: 0, kind: EventKind::Message { item: "x".into(), text: "ok".into() } };
        assert_eq!(last_prompt(&[u(1, "first"), m(2), u(3, "  second \n"), m(4)]), Some(Prompt { seq: 3, text: "second".into(), ts: 30 }));
        assert_eq!(last_prompt(&[m(1), u(2, "  ")]), None);
    }
}
