//! Does a stopped turn look finished? Read from its last message and the plan.
use agentum_proto::{Event, EventKind, Ms};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Send the nudge; why.
    Nudge(&'static str),
    /// Leave it; why.
    Leave(&'static str),
}

/// A thread's Nudger state, kept on the hub.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ThreadNudge {
    pub on: bool,
    pub count: u32,
    /// What it last decided, in words.
    #[serde(default)]
    pub why: Option<String>,
    /// It last decided to nudge.
    #[serde(default)]
    pub nudged: bool,
    #[serde(default)]
    pub at: Option<Ms>,
}

/// Asking the person something: leave it to them.
const ASKS: &[&str] = &[
    "let me know", "would you like", "do you want", "should i ", "shall i ", "want me to", "which would you", "which do you", "which one", "your call", "up to you",
    "please confirm", "can you confirm", "could you confirm", "please provide", "please share", "can you share", "i need you to", "need your", "waiting for you",
    "waiting on you", "wait for your", "your preference", "which option", "how would you like", "if you want", "if you'd like", "if you like", "your approval",
];
/// Saying it will go on.
const GOES_ON: &[&str] = &[
    "i'll continue", "i will continue", "let me continue", "continuing with", "continuing to", "next, i'll", "next i'll", "next, i will", "now i'll", "now i will",
    "i'll now", "i will now", "now let me", "let me now", "i'll proceed", "i will proceed", "proceeding to", "proceeding with", "i'll keep going", "i'll move on",
    "moving on to", "the next step is", "next step:", "then i'll", "i'm going to", "i am going to", "i still need to", "still need to", "remaining steps",
    "remaining tasks", "not done yet", "to be continued", "i'll start", "i'll go ahead", "i'll get started", "i'll work on", "i'll fix", "i'll update", "i'll add",
    "i'll run", "i'll check", "i'll implement", "i'll write", "i'll look", "i'll try",
];

/// The tail of a message, lower-cased, markdown dressing off: its last paragraph (at most 400
/// characters) and its last line.
fn tail(text: &str) -> (String, String) {
    let t = text.trim();
    let para = t.rsplit("\n\n").map(str::trim).find(|p| !p.is_empty() && !p.starts_with("```")).unwrap_or(t);
    let para: String = para.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
    let clean = |s: &str| s.to_lowercase().replace(['*', '_', '`'], "").replace('’', "'");
    let line = t.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("");
    let line = clean(line).trim_end_matches(|c: char| c.is_whitespace() || "\"')]>".contains(c) || (!c.is_ascii() && !c.is_alphanumeric() && c != '…')).to_string();
    (clean(&para), line)
}

/// Does a message (and the plan) look like the agent stopped before it was done?
pub fn judge(text: &str, plan_open: bool) -> Verdict {
    let (para, line) = tail(text);
    if line.ends_with('?') {
        return Verdict::Leave("it asked you something");
    }
    if ASKS.iter().any(|p| para.contains(p)) {
        return Verdict::Leave("it's waiting on your answer");
    }
    if plan_open {
        return Verdict::Nudge("its plan has open steps");
    }
    if text.trim().is_empty() {
        return Verdict::Leave("it said nothing");
    }
    if text.matches("```").count() % 2 == 1 {
        return Verdict::Nudge("it stopped mid-sentence");
    }
    if GOES_ON.iter().any(|p| para.contains(p)) || line.starts_with("let me ") || line.contains(". let me ") {
        return Verdict::Nudge("it said it would go on");
    }
    if [':', '…', ',', ';', '-'].iter().any(|c| line.ends_with(*c)) || line.ends_with("...") {
        return Verdict::Nudge("it stopped mid-sentence");
    }
    Verdict::Leave("it looks finished")
}

/// Judge the turn that ended at `done_seq`: its last message, and the newest plan.
pub fn assess(events: &[Event], done_seq: u64) -> Verdict {
    let upto: Vec<&Event> = events.iter().filter(|e| e.seq <= done_seq).collect();
    let start = upto.iter().rposition(|e| matches!(e.kind, EventKind::TurnStarted { .. })).unwrap_or(0);
    let said = upto[start..].iter().rev().find_map(|e| match &e.kind {
        EventKind::Message { text, .. } => Some(text.as_str()),
        _ => None,
    });
    let plan_open = upto
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::Plan { entries } => Some(entries.iter().any(|p| p.status != "done")),
            _ => None,
        })
        .unwrap_or(false);
    judge(said.unwrap_or(""), plan_open)
}

/// Did the thread move on after the turn ended (a message from the person, a new turn)?
pub fn moved_on(events: &[Event], done_seq: u64) -> bool {
    events.iter().any(|e| e.seq > done_seq && matches!(e.kind, EventKind::User { .. } | EventKind::TurnStarted { .. } | EventKind::Ask { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_proto::{PlanEntry, StopReason};
    use Verdict::*;

    #[test]
    fn questions_are_left_to_you() {
        assert_eq!(judge("I found two ways. Which do you prefer?", false), Leave("it asked you something"));
        assert_eq!(judge("Should I also update the docs? 🙂", true), Leave("it asked you something"));
        assert_eq!(judge("Done with the parser.\n\nLet me know if you want the tests too.", false), Leave("it's waiting on your answer"));
        assert_eq!(judge("**Want me to push it?**", false), Leave("it asked you something"));
    }

    #[test]
    fn unfinished_work_is_nudged() {
        assert_eq!(judge("Fixed the first bug. Next, I'll look at the second one.", false), Nudge("it said it would go on"));
        assert_eq!(judge("Now let me run the tests.", false), Nudge("it said it would go on"));
        assert_eq!(judge("The remaining files are:", false), Nudge("it stopped mid-sentence"));
        assert_eq!(judge("Here is the change:\n\n```rust\nfn main() {", false), Nudge("it stopped mid-sentence"));
        assert_eq!(judge("All set.", true), Nudge("its plan has open steps"));
        assert_eq!(judge("", true), Nudge("its plan has open steps"));
    }

    #[test]
    fn finished_work_is_left() {
        assert_eq!(judge("All tests pass. The fix is in `src/lib.rs`.", false), Leave("it looks finished"));
        assert_eq!(judge("Done! Summary:\n\n- fixed the parser\n- added tests\n\nEverything builds.", false), Leave("it looks finished"));
        assert_eq!(judge("", false), Leave("it said nothing"));
    }

    fn ev(seq: u64, kind: EventKind) -> Event {
        Event { seq, ts: seq as i64, kind }
    }

    #[test]
    fn reads_the_turn_that_ended() {
        let plan = |s: &str| EventKind::Plan { entries: vec![PlanEntry { text: "a".into(), status: "done".into() }, PlanEntry { text: "b".into(), status: s.into() }] };
        let msg = |t: &str| EventKind::Message { item: "m".into(), text: t.into() };
        let done = EventKind::TurnDone { turn: 1, stop: StopReason::End, usage: None, cost_usd: None, duration_ms: 0 };
        let evs = vec![
            ev(1, EventKind::TurnStarted { turn: 1 }),
            ev(2, plan("pending")),
            ev(3, msg("Next I'll wire it up.")),
            ev(4, done.clone()),
            ev(5, EventKind::TurnStarted { turn: 2 }),
            ev(6, plan("done")),
            ev(7, msg("Everything works now.")),
            ev(8, done),
        ];
        assert_eq!(assess(&evs, 4), Nudge("its plan has open steps"));
        assert_eq!(assess(&evs, 8), Leave("it looks finished"));
        assert!(moved_on(&evs, 4));
        assert!(!moved_on(&evs, 8));
    }
}
