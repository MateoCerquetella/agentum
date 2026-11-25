//! Agentum's shared vocabulary.
//!
//! - [`thread`]: what a coding thread is and the events it is made of. Every agent (Claude Code,
//!   Codex, OpenCode) is translated by agentd into these same events.
//! - [`machine`]: the protocol between the hub and agentd on each machine (JSON over one
//!   WebSocket that agentd opens to the hub).
//! - [`api`]: what the hub serves the web app for the Code section (REST bodies and the
//!   messages on the browser's WebSocket).
//! - [`dotenv`]: reading and editing `.env` files (the binaries' own, and projects' secrets).
//!
//! Everything here is plain serde: it compiles for the hub, agentd and wasm32 alike.
pub mod api;
pub mod dotenv;
pub mod machine;
pub mod thread;

pub use thread::*;

/// Milliseconds since the Unix epoch, the only time format on the wire.
pub type Ms = i64;

/// The version of the machine protocol; the hub refuses an agentd that speaks another.
pub const PROTOCOL: u32 = 1;

#[cfg(test)]
mod tests {
    use super::machine::*;
    use super::*;
    use serde_json::json;

    fn round<T: serde::Serialize + serde::de::DeserializeOwned>(v: &T) -> serde_json::Value {
        let j = serde_json::to_value(v).unwrap();
        let back: T = serde_json::from_value(j.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), j);
        j
    }

    #[test]
    fn wire_shapes() {
        let r = round(&FromAgentd::Reply { id: 7, result: Reply::Ok(json!({"a": 1})) });
        assert_eq!(r, json!({"t": "reply", "id": 7, "ok": {"a": 1}}));
        let r = round(&FromAgentd::Reply { id: 8, result: Reply::Err("no".into()) });
        assert_eq!(r, json!({"t": "reply", "id": 8, "err": "no"}));
        let r = round(&ToAgentd::Req { id: 1, req: Req::Git(GitReq::Status { cwd: "/r".into() }) });
        assert_eq!(r, json!({"t": "req", "id": 1, "req": {"op": "git", "git": "status", "cwd": "/r"}}));
        let r = round(&ToAgentd::Req { id: 2, req: Req::Term(TermReq::Open { term: "x".into(), cwd: "/".into(), cols: 80, rows: 24, cmd: None }) });
        assert_eq!(r["req"]["do"], "open");
        let e = Event { seq: 3, ts: 9, kind: EventKind::Delta { item: "m1".into(), text: "hi".into() } };
        assert_eq!(round(&e), json!({"seq": 3, "ts": 9, "k": "delta", "item": "m1", "text": "hi"}));
        assert_eq!(round(&AgentKind::ClaudeCode), json!("claude-code"));
        let r = round(&ToAgentd::Req { id: 3, req: Req::Surf(SurfReq::Ensure { project: "/r".into(), idle_hours: None }) });
        assert_eq!(r["req"], json!({"op": "surf", "surf": "ensure", "project": "/r", "idle_hours": null}));
        let s = round(&FromAgentd::Sock(SockMsg { sock: "s1".into(), text: Some("{}".into()), data: None }));
        assert_eq!(s, json!({"t": "sock", "sock": "s1", "text": "{}"}));
        assert_eq!(round(&ToAgentd::SockClose { sock: "s1".into() }), json!({"t": "sock-close", "sock": "s1"}));
        // An older hub's spec (no instructions) still reads.
        let spec: ThreadSpec = serde_json::from_value(json!({"agent": "codex", "cwd": "/r"})).unwrap();
        assert_eq!(spec.instructions, None);
        let w = round(&api::WsIn::Sub { ch: vec!["threads".into()] });
        assert_eq!(w, json!({"t": "sub", "ch": ["threads"]}));
        let r = round(&Req::Secrets(SecretsReq::Set { dir: "/r".into(), key: "K".into(), value: "v".into() }));
        assert_eq!(r, json!({"op": "secrets", "secrets": "set", "dir": "/r", "key": "K", "value": "v"}));
        assert_eq!(format!("{:?}", Req::Secrets(SecretsReq::Set { dir: "/r".into(), key: "K".into(), value: "hunter2".into() })), "Secrets(SecretsReq(set K in /r/.env))");
    }

    #[test]
    fn old_peers_still_parse() {
        // A spec and a hello from before `instructions` and `features`.
        let s: ThreadSpec = serde_json::from_value(json!({"agent": "codex", "cwd": "/r"})).unwrap();
        assert_eq!(s.instructions, None);
        assert!(!serde_json::to_string(&s).unwrap().contains("instructions"));
        let h: Hello = serde_json::from_value(json!({"protocol": 1, "machine": "m", "version": "v", "hostname": "h", "os": "o", "arch": "a", "home": "/h", "agents": [], "projects": [], "threads": []})).unwrap();
        assert!(h.features.is_empty());
        // A usage answer and a stored report from before accounts.
        let u: UsageReply = serde_json::from_value(json!({"limits": [{"window": "5h", "used_percent": 3.0}], "plan": null})).unwrap();
        assert_eq!((u.limits.len(), u.account), (1, None));
        let l: api::AgentLimits = serde_json::from_value(json!({"agent": "codex", "machine": "m", "limits": [], "at": 1})).unwrap();
        assert_eq!((l.key(), l.machines.len()), ("m", 0));
        assert!(!serde_json::to_string(&l).unwrap().contains("account"));
        assert_eq!(round(&AgentKind::Cursor), json!("cursor"));
        assert_eq!(AgentKind::parse("cursor"), Some(AgentKind::Cursor));
    }

    #[test]
    fn limits_by_account() {
        use api::{AgentLimits, limits_for, put_limits};
        let l = |machine: &str, account: Option<&str>, machines: &[&str], at| AgentLimits {
            agent: AgentKind::ClaudeCode,
            machine: machine.into(),
            limits: vec![],
            at,
            plan: None,
            account: account.map(str::to_string),
            account_label: None,
            machines: machines.iter().map(|m| m.to_string()).collect(),
        };
        let mut all = vec![l("jupiter", None, &[], 1), l("workbox", None, &[], 1)];
        // jupiter and mars report the personal account: jupiter's old report goes.
        put_limits(&mut all, l("jupiter", Some("me"), &["jupiter", "mars"], 2));
        assert_eq!(all.iter().map(|x| x.key()).collect::<Vec<_>>(), ["me", "workbox"]);
        put_limits(&mut all, l("workbox", Some("work"), &["workbox"], 3));
        assert_eq!(all.iter().map(|x| x.key()).collect::<Vec<_>>(), ["work", "me"]);
        assert_eq!(limits_for(&all, AgentKind::ClaudeCode, "mars").and_then(|x| x.account.as_deref()), Some("me"));
        assert_eq!(limits_for(&all, AgentKind::ClaudeCode, "workbox").and_then(|x| x.account.as_deref()), Some("work"));
        assert!(limits_for(&all, AgentKind::Codex, "workbox").is_none());
        // mars signs in to the work account: it leaves the personal one.
        put_limits(&mut all, l("mars", Some("work"), &["workbox", "mars"], 4));
        assert_eq!(all.iter().find(|x| x.key() == "me").unwrap().machines, ["jupiter"]);
        // And jupiter too: nobody is on the personal account any more.
        put_limits(&mut all, l("jupiter", Some("work"), &["workbox", "mars", "jupiter"], 5));
        assert_eq!(all.len(), 1);
    }
}
