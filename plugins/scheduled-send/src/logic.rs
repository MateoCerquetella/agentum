//! Messages waiting to be sent, kept in the plugin's storage on the hub.
use agentum_proto::Ms;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Scheduled {
    pub id: String,
    pub thread: String,
    pub text: String,
    /// When to send it.
    pub at: Ms,
    #[serde(default)]
    pub created: Ms,
    /// Failed sends so far (the machine was offline…); it retries a minute later.
    #[serde(default)]
    pub tries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Give up after this many failed sends.
pub const MAX_TRIES: u32 = 10;

/// Can this be scheduled now?
pub fn check(text: &str, at: Ms, now: Ms) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("Type a message first.".into());
    }
    if at < now + 5_000 {
        return Err("Pick a time in the future.".into());
    }
    if at > now + 366 * 86_400_000 {
        return Err("Pick a time within a year.".into());
    }
    Ok(())
}

/// Split the queue into what is due now (oldest first) and the rest.
pub fn take_due(queue: Vec<Scheduled>, now: Ms) -> (Vec<Scheduled>, Vec<Scheduled>) {
    let (mut due, rest): (Vec<_>, Vec<_>) = queue.into_iter().partition(|s| s.at <= now);
    due.sort_by_key(|s| s.at);
    (due, rest)
}

/// After a failed send: try again in a minute, or give up (None).
pub fn retry(mut s: Scheduled, error: String, now: Ms) -> Option<Scheduled> {
    s.tries += 1;
    if s.tries >= MAX_TRIES {
        return None;
    }
    s.error = Some(error);
    s.at = now + 60_000;
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str, at: Ms) -> Scheduled {
        Scheduled { id: id.into(), thread: "t_1".into(), text: "hi".into(), at, created: 0, tries: 0, error: None }
    }

    #[test]
    fn due_and_retries() {
        let (due, rest) = take_due(vec![s("b", 200), s("a", 100), s("c", 900)], 500);
        assert_eq!(due.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(rest.len(), 1);
        let r = retry(s("a", 100), "offline".into(), 1_000).unwrap();
        assert_eq!((r.tries, r.at, r.error.as_deref()), (1, 61_000, Some("offline")));
        let mut last = s("z", 0);
        last.tries = MAX_TRIES - 1;
        assert!(retry(last, "offline".into(), 0).is_none());
    }

    #[test]
    fn checks() {
        assert!(check("  ", 10_000_000, 0).is_err());
        assert!(check("go", 1_000, 0).is_err());
        assert!(check("go", 60_000, 0).is_ok());
    }
}
