//! Usage's hub part: tokens and cost from every thread's TurnDone events, by hour and by
//! (agent, machine, project), kept in the plugin's storage and brought up to date incrementally
//! (each thread remembers the last event folded in).
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::ThreadSummary;
use agentum_proto::{Event, EventKind, Usage};
use super::Stat;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::sync::Mutex;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

const KEY: &str = "agg";
const LIMITS: &str = "limits";
const HOUR: i64 = 3_600_000;
/// Hours kept (the page shows 14 days in the viewer's time zone).
const KEEP_HOURS: i64 = 24 * 35;

impl Stat {
    fn add(&mut self, u: Option<&Usage>, cost: Option<f64>) {
        if let Some(u) = u {
            self.input += u.input;
            self.output += u.output;
            self.cache_read += u.cache_read;
            self.cache_write += u.cache_write;
        }
        self.cost += cost.unwrap_or(0.0);
        self.turns += 1;
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct Agg {
    /// Thread → the last event seq folded in.
    #[serde(default)]
    seen: BTreeMap<String, u64>,
    /// Hour (ms / 3 600 000) → "agent\tmachine\tproject" → totals.
    #[serde(default)]
    hours: BTreeMap<i64, BTreeMap<String, Stat>>,
    /// "agent\tmachine\tproject" → totals since the start.
    #[serde(default)]
    total: BTreeMap<String, Stat>,
}

impl Agg {
    fn fold(&mut self, t: &ThreadSummary, e: &Event) {
        if let EventKind::TurnDone { usage, cost_usd, .. } = &e.kind {
            let key = format!("{}\t{}\t{}", t.agent.id(), t.machine, t.project);
            self.hours.entry(e.ts.div_euclid(HOUR)).or_default().entry(key.clone()).or_default().add(usage.as_ref(), *cost_usd);
            self.total.entry(key).or_default().add(usage.as_ref(), *cost_usd);
        }
    }
    fn prune(&mut self, now_ms: i64) {
        let oldest = now_ms.div_euclid(HOUR) - KEEP_HOURS;
        self.hours.retain(|h, _| *h >= oldest);
    }
}

/// The aggregate, cached in memory once loaded; every change goes through this lock.
static AGG: Mutex<Option<Agg>> = Mutex::const_new(None);

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Fold the events of `t` after the last one seen. True if anything changed.
async fn catch_up(cx: &Ctx, agg: &mut Agg, t: &ThreadSummary) -> bool {
    let seen = agg.seen.get(&t.id).copied().unwrap_or(0);
    if t.last_seq <= seen {
        return false;
    }
    let mut last = seen;
    for e in cx.events(&t.id, seen).await {
        last = last.max(e.seq);
        agg.fold(t, &e);
    }
    // Deltas aren't stored, so the summary's last_seq may be past the last stored event: every
    // event up to it was stored before the summary said so.
    agg.seen.insert(t.id.clone(), last.max(t.last_seq));
    true
}

async fn load(cx: &Ctx, slot: &mut Option<Agg>) {
    if slot.is_none() {
        *slot = Some(cx.kv_get(KEY).await.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default());
    }
}

async fn save(cx: &Ctx, agg: &mut Agg) {
    agg.prune(now_ms());
    if let Ok(v) = serde_json::to_value(&*agg) {
        cx.kv_set(KEY, v).await;
    }
}

fn split(key: &str) -> (String, String, String) {
    let mut p = key.splitn(3, '\t');
    let mut next = || p.next().unwrap_or("").to_string();
    (next(), next(), next())
}

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    async fn on_event(&self, cx: &Ctx, t: &ThreadSummary, e: &Event) {
        // The newest windows per agent and machine, kept so the page has them after a restart.
        if let EventKind::RateLimits { limits } = &e.kind {
            let _g = AGG.lock().await;
            let mut all = cx.kv_get(LIMITS).await.and_then(|v| v.as_object().cloned()).unwrap_or_default();
            all.insert(format!("{}\t{}", t.agent.id(), t.machine), json!({ "agent": t.agent, "machine": t.machine, "limits": limits, "at": e.ts }));
            cx.kv_set(LIMITS, Value::Object(all)).await;
            return;
        }
        if !matches!(e.kind, EventKind::TurnDone { .. }) {
            return;
        }
        let mut slot = AGG.lock().await;
        load(cx, &mut slot).await;
        let agg = slot.as_mut().unwrap();
        if catch_up(cx, agg, t).await {
            save(cx, agg).await;
            cx.publish(json!({ "changed": true }));
        }
    }

    /// `summary` → `{hours: [{h, agent, machine, project, s}], total: [{agent, machine, project, s}],
    /// limits: [AgentLimits as last reported]}`
    /// (hours of the last 15 days; the page groups them into its own days).
    async fn rpc(&self, cx: &Ctx, method: &str, _input: Value) -> Result<Value, String> {
        if method != "summary" {
            return Err(format!("no method {method}"));
        }
        let threads = cx.threads().await;
        let mut slot = AGG.lock().await;
        load(cx, &mut slot).await;
        let agg = slot.as_mut().unwrap();
        let mut changed = false;
        for t in &threads {
            changed |= catch_up(cx, agg, t).await;
        }
        let before = agg.seen.len();
        agg.seen.retain(|id, _| threads.iter().any(|t| &t.id == id));
        if changed || agg.seen.len() != before {
            save(cx, agg).await;
        }
        let from = now_ms().div_euclid(HOUR) - 24 * 15;
        let hours: Vec<Value> = agg
            .hours
            .range(from..)
            .flat_map(|(h, m)| {
                m.iter().map(move |(k, s)| {
                    let (agent, machine, project) = split(k);
                    json!({ "h": h, "agent": agent, "machine": machine, "project": project, "s": s })
                })
            })
            .collect();
        let total: Vec<Value> = agg
            .total
            .iter()
            .map(|(k, s)| {
                let (agent, machine, project) = split(k);
                json!({ "agent": agent, "machine": machine, "project": project, "s": s })
            })
            .collect();
        let limits: Vec<Value> = cx.kv_get(LIMITS).await.and_then(|v| v.as_object().cloned()).unwrap_or_default().into_values().collect();
        Ok(json!({ "hours": hours, "total": total, "limits": limits }))
    }
}
