//! Side chats: which belong to a thread, and which have gone quiet.
use agentum_proto::api::ThreadSummary;
use agentum_proto::Ms;

/// Archived after this long with nothing new.
pub const QUIET_MS: Ms = 24 * 3_600_000;

/// A thread's side chats, newest activity first.
pub fn of(threads: &[ThreadSummary], parent: &str) -> Vec<ThreadSummary> {
    let mut v: Vec<ThreadSummary> = threads.iter().filter(|t| t.parent.as_deref() == Some(parent)).cloned().collect();
    v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    v
}

/// Open side chats with nothing new for a day (and nothing running or asking).
pub fn quiet(threads: &[ThreadSummary], now: Ms) -> Vec<String> {
    threads
        .iter()
        .filter(|t| t.parent.is_some() && !t.archived && !t.pinned && !t.state.busy() && t.asking.is_none() && now - t.updated_at > QUIET_MS)
        .map(|t| t.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_proto::{AgentKind, Attention, Permission, ThreadState};

    fn t(id: &str, parent: Option<&str>, updated_at: Ms) -> ThreadSummary {
        ThreadSummary {
            id: id.into(),
            machine: "m".into(),
            agent: AgentKind::ClaudeCode,
            model: None,
            effort: None,
            permission: Permission::Ask,
            project: "/p".into(),
            cwd: "/p".into(),
            branch: None,
            title: String::new(),
            state: ThreadState::Idle,
            attention: Attention::Settled,
            unread: false,
            pinned: false,
            archived: false,
            order: 0.0,
            section: None,
            parent: parent.map(str::to_string),
            created_at: 0,
            updated_at,
            preview: String::new(),
            asking: None,
            queued: 0,
            usage: None,
            cost_usd: 0.0,
            last_seq: 0,
            snoozed_until: None,
            archived_at: None,
        }
    }

    #[test]
    fn quiet_side_chats() {
        let now = 10 * QUIET_MS;
        let mut busy = t("busy", Some("p"), 0);
        busy.state = ThreadState::Running;
        let mut gone = t("gone", Some("p"), 0);
        gone.archived = true;
        let all = vec![t("old", Some("p"), now - QUIET_MS - 1), t("new", Some("p"), now - 1000), t("top", None, 0), busy, gone];
        assert_eq!(quiet(&all, now), ["old"]);
        assert_eq!(of(&all, "p").iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["new", "old", "busy", "gone"]);
    }
}
