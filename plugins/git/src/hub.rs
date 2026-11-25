//! Git's hub part: a thread's stat (for the header badge), refreshed after every turn.
use super::{numstat, Stat};
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::ThreadSummary;
use agentum_proto::machine::{GitReq, Req};
use agentum_proto::{Event, EventKind};
use serde_json::{json, Value};

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

/// The uncommitted diff's size and the branch's place against its upstream.
async fn stat(cx: &Ctx, t: &ThreadSummary) -> Result<Stat, String> {
    let diff = cx.machine(&t.machine, Req::Git(GitReq::Diff { cwd: t.cwd.clone(), target: None, path: None })).await?;
    let mut s = numstat(diff["diff"].as_str().unwrap_or(""));
    if let Ok(st) = cx.machine(&t.machine, Req::Git(GitReq::Status { cwd: t.cwd.clone() })).await {
        s.branch = st["branch"].as_str().map(str::to_string);
        s.ahead = st["ahead"].as_u64().unwrap_or(0) as u32;
        s.behind = st["behind"].as_u64().unwrap_or(0) as u32;
    }
    Ok(s)
}

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    async fn rpc(&self, cx: &Ctx, method: &str, input: Value) -> Result<Value, String> {
        match method {
            // {thread} → Stat
            "stat" => {
                let id = input["thread"].as_str().ok_or("thread?")?;
                let t = cx.thread(id).await.ok_or("no such thread")?;
                let s = stat(cx, &t).await?;
                Ok(json!(s))
            }
            _ => Err(format!("no method {method}")),
        }
    }

    async fn on_event(&self, cx: &Ctx, t: &ThreadSummary, e: &Event) {
        if !matches!(e.kind, EventKind::TurnDone { .. }) {
            return;
        }
        if let Ok(s) = stat(cx, t).await {
            cx.publish(json!({"thread": t.id, "stat": s}));
        }
    }
}
