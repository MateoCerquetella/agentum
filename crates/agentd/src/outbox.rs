//! Thread events on their way to the hub.
//!
//! Every thread's events are numbered from 1 with no gaps and kept (in memory and in SQLite) until
//! the hub acks them. The hub connection pulls what it hasn't sent yet; after a reconnect it starts
//! again from the last ack, so everything unacked is resent in order.
use crate::db::Db;
use crate::util::now_ms;
use agentum_proto::{Event, EventKind, ThreadId};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use tokio::sync::Notify;

#[derive(Default)]
struct Log {
    /// Unacked events, in order.
    events: VecDeque<Event>,
    /// The last seq assigned.
    last: u64,
    /// Everything up to here is stored by the hub.
    acked: u64,
    /// Everything up to here went out on the current connection.
    sent: u64,
}

pub struct Outbox {
    logs: Mutex<HashMap<ThreadId, Log>>,
    /// Woken when there is something new to send.
    pub notify: Notify,
    db: Db,
}

impl Outbox {
    pub fn new(db: Db) -> Outbox {
        Outbox { logs: Mutex::new(HashMap::new()), notify: Notify::new(), db }
    }

    /// Restore a thread's log from the database (at startup).
    pub fn restore(&self, thread: &str, last: u64, acked: u64, events: Vec<Event>) {
        let mut logs = self.logs.lock().unwrap();
        let last = events.last().map(|e| e.seq).unwrap_or(0).max(last);
        logs.insert(thread.to_string(), Log { events: events.into(), last, acked, sent: acked });
    }

    /// Make sure the thread has a log; numbering continues after `last_seq` (the hub's) or our own.
    pub fn ensure(&self, thread: &str, last_seq: u64) {
        let mut logs = self.logs.lock().unwrap();
        let log = logs.entry(thread.to_string()).or_default();
        if last_seq > log.last {
            // The hub has more than we know of: continue after it.
            log.last = last_seq;
            log.acked = log.acked.max(last_seq);
            log.sent = log.sent.max(last_seq);
            log.events.retain(|e| e.seq > last_seq);
        }
    }

    /// Append an event; returns its seq.
    pub fn push(&self, thread: &str, kind: EventKind) -> u64 {
        let ev = {
            let mut logs = self.logs.lock().unwrap();
            let log = logs.entry(thread.to_string()).or_default();
            log.last += 1;
            let ev = Event { seq: log.last, ts: now_ms(), kind };
            log.events.push_back(ev.clone());
            ev
        };
        self.db.insert_event(thread, &ev);
        self.notify.notify_one();
        ev.seq
    }

    /// The hub stored everything of `thread` up to `seq`.
    pub fn ack(&self, thread: &str, seq: u64) {
        let mut logs = self.logs.lock().unwrap();
        if let Some(log) = logs.get_mut(thread) {
            if seq <= log.acked {
                return;
            }
            log.acked = seq.min(log.last);
            log.sent = log.sent.max(log.acked);
            while log.events.front().is_some_and(|e| e.seq <= seq) {
                log.events.pop_front();
            }
            drop(logs);
            self.db.ack(thread, seq);
        }
    }

    /// Events not yet sent on this connection, per thread, in order (at most `max` per thread).
    pub fn take_unsent(&self, max: usize) -> Vec<(ThreadId, Vec<Event>)> {
        let mut logs = self.logs.lock().unwrap();
        let mut out = Vec::new();
        for (id, log) in logs.iter_mut() {
            if log.sent >= log.last {
                continue;
            }
            let batch: Vec<Event> = log.events.iter().filter(|e| e.seq > log.sent).take(max).cloned().collect();
            if let Some(last) = batch.last() {
                log.sent = last.seq;
                out.push((id.clone(), batch));
            }
        }
        out
    }

    /// Is anything still waiting to be sent?
    pub fn has_unsent(&self) -> bool {
        self.logs.lock().unwrap().values().any(|l| l.sent < l.last)
    }

    /// A new connection: everything unacked must go out again.
    pub fn rewind(&self) {
        for log in self.logs.lock().unwrap().values_mut() {
            log.sent = log.acked;
        }
        self.notify.notify_one();
    }

    pub fn last_seq(&self, thread: &str) -> u64 {
        self.logs.lock().unwrap().get(thread).map(|l| l.last).unwrap_or(0)
    }

    pub fn acked(&self, thread: &str) -> u64 {
        self.logs.lock().unwrap().get(thread).map(|l| l.acked).unwrap_or(0)
    }

    pub fn unacked(&self, thread: &str) -> usize {
        self.logs.lock().unwrap().get(thread).map(|l| l.events.len()).unwrap_or(0)
    }

    /// Threads with unacked events.
    pub fn pending_threads(&self) -> Vec<ThreadId> {
        self.logs.lock().unwrap().iter().filter(|(_, l)| !l.events.is_empty()).map(|(k, _)| k.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outbox() -> Outbox {
        let dir = std::env::temp_dir().join(format!("agentd-ob-{}", crate::util::random_hex(4)));
        Outbox::new(Db::open(&dir.join("agentd.db")).unwrap())
    }
    fn n(t: &str) -> EventKind {
        EventKind::Notice { text: t.into() }
    }

    #[test]
    fn numbering_ack_and_rewind() {
        let o = outbox();
        o.ensure("t", 0);
        assert_eq!(o.push("t", n("a")), 1);
        assert_eq!(o.push("t", n("b")), 2);
        let b = o.take_unsent(100);
        assert_eq!(b[0].1.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2]);
        assert!(o.take_unsent(100).is_empty());
        o.push("t", n("c"));
        o.ack("t", 1);
        // Reconnect: 2 and 3 go again, in order.
        o.rewind();
        let b = o.take_unsent(100);
        assert_eq!(b[0].1.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(o.unacked("t"), 2);
    }

    #[test]
    fn continues_after_the_hubs_last_seq() {
        let o = outbox();
        o.ensure("t", 41);
        assert_eq!(o.push("t", n("a")), 42);
        // A smaller hint never goes back.
        o.ensure("t", 3);
        assert_eq!(o.push("t", n("b")), 43);
    }

    #[test]
    fn batches_are_capped() {
        let o = outbox();
        for i in 0..5 {
            o.push("t", n(&i.to_string()));
        }
        assert_eq!(o.take_unsent(2)[0].1.len(), 2);
        assert_eq!(o.take_unsent(2)[0].1[0].seq, 3);
        assert!(o.has_unsent());
    }
}
