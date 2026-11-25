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
        let w = round(&api::WsIn::Sub { ch: vec!["threads".into()] });
        assert_eq!(w, json!({"t": "sub", "ch": ["threads"]}));
    }
}
