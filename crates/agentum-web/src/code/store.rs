//! The Code section's live state: machines, thread summaries and limits, kept current by the
//! WebSocket ("threads" and "machines" channels). One copy for the whole page.
use agentum_proto::api::{Overview, ThreadSummary, WsOut};
use agentum_proto::Attention;
use agentum_ui::prelude::*;
use agentum_ui::ws::connected;

thread_local! {
    static OV: RwSignal<Option<Overview>> = RwSignal::new(None);
    static STARTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static VISITED: RwSignal<Vec<String>> = RwSignal::new(agentum_ui::store::get("agentum.visited").map(|s| s.split(',').filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default());
}

pub fn overview() -> RwSignal<Option<Overview>> {
    start();
    OV.with(|o| *o)
}

fn load() {
    spawn_local(async {
        if let Ok(o) = api_get::<Overview>("/code/overview").await {
            OV.with(|s| s.set(Some(o)));
        }
    });
}

/// Fetch once and follow the hub (refetches after a reconnect, in case something was missed).
pub fn start() {
    if STARTED.with(|s| s.replace(true)) {
        return;
    }
    let owner = Owner::new_root(None);
    owner.with(|| {
        subscribe(&["threads", "machines"]);
        on_ws(|m| {
            let ov = OV.with(|o| *o);
            match m {
                WsOut::Thread { summary } => ov.update(|o| {
                    if let Some(o) = o {
                        match o.threads.iter_mut().find(|t| t.id == summary.id) {
                            Some(t) => *t = summary.clone(),
                            None => o.threads.insert(0, summary.clone()),
                        }
                    }
                }),
                WsOut::ThreadGone { id } => ov.update(|o| {
                    if let Some(o) = o {
                        o.threads.retain(|t| &t.id != id)
                    }
                }),
                WsOut::Machine { machine } => ov.update(|o| {
                    if let Some(o) = o {
                        match o.machines.iter_mut().find(|x| x.id == machine.id) {
                            Some(x) => *x = machine.clone(),
                            None => o.machines.push(machine.clone()),
                        }
                    }
                }),
                WsOut::Limits { limits } => ov.update(|o| {
                    if let Some(o) = o {
                        o.limits.retain(|l| !(l.agent == limits.agent && l.machine == limits.machine));
                        o.limits.insert(0, limits.clone());
                    }
                }),
                _ => {}
            }
        });
        let c = connected();
        Effect::new(move |prev: Option<u32>| {
            let n = c.get();
            if prev.is_some() && n > 0 {
                load();
            }
            n
        });
    });
    std::mem::forget(owner);
    load();
}

/// One thread's summary, live.
pub fn thread(id: impl Fn() -> String + Send + Sync + 'static) -> Signal<Option<ThreadSummary>> {
    let ov = overview();
    Signal::derive(move || {
        let id = id();
        ov.with(|o| o.as_ref().and_then(|o| o.threads.iter().find(|t| t.id == id).cloned()))
    })
}

/// Thread ids in the order the sidebar shows them (for ⌘1–9 and ⌘⇧[ ]).
pub fn sidebar_order(threads: &[ThreadSummary]) -> Vec<String> {
    let open: Vec<&ThreadSummary> = threads.iter().filter(|t| !t.archived && t.parent.is_none()).collect();
    let mut out: Vec<String> = vec![];
    let mut push = |mut v: Vec<&ThreadSummary>| {
        v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out.extend(v.into_iter().map(|t| t.id.clone()));
    };
    push(open.iter().copied().filter(|t| t.pinned).collect());
    for a in [Attention::NeedsYou, Attention::Ready, Attention::Working, Attention::Settled] {
        push(open.iter().copied().filter(|t| !t.pinned && t.attention == a).collect());
    }
    out
}

/// "3 machines · 2 working".
pub fn machines_line() -> String {
    OV.with(|o| {
        o.with(|o| match o {
            None => "Connecting…".into(),
            Some(o) => {
                let on = o.machines.iter().filter(|m| m.online).count();
                let working = o.threads.iter().filter(|t| t.attention == Attention::Working).count();
                let needs = o.threads.iter().filter(|t| t.attention == Attention::NeedsYou && !t.archived).count();
                let mut s = format!("{on}/{} machines online", o.machines.len());
                if working > 0 {
                    s += &format!(" · {working} working");
                }
                if needs > 0 {
                    s += &format!(" · {needs} need you");
                }
                s
            }
        })
    })
}

/// Threads by when they were last opened here (oldest first).
pub fn visited() -> Vec<String> {
    VISITED.with(|v| v.get_untracked())
}
pub fn note_visit(id: &str) {
    VISITED.with(|v| {
        v.update(|l| {
            l.retain(|x| x != id);
            l.push(id.to_string());
            if l.len() > 30 {
                l.remove(0);
            }
        });
        agentum_ui::store::set("agentum.visited", &v.get_untracked().join(","));
    });
}
