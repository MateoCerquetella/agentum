//! Usage's web part: the page at /p/usage. Each agent's rate-limit windows per account (from the
//! hub's overview, live), then tokens and cost per day for the last 14 days (in this browser's
//! time zone) and per project, from the hub part's sums.
use super::Stat;
use agentum_plugin::web::{NavEntry, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::{put_limits, AgentLimits, Overview, WsOut};
use agentum_proto::{AgentKind, RateLimit};
use agentum_ui::fmt::tokens;
use agentum_ui::prelude::*;
use std::collections::BTreeMap;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn nav(&self) -> Option<NavEntry> {
        Some(NavEntry { path: "/p/usage", title: "Usage", icon: "bar-chart", view: page })
    }
}

const DAYS: i64 = 14;
const HOUR: i64 = 3_600_000;

#[derive(Clone, PartialEq)]
struct Row {
    h: i64,
    agent: String,
    machine: String,
    project: String,
    s: Stat,
}

fn rows(v: &Value) -> Vec<Row> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|r| Row {
            h: r["h"].as_i64().unwrap_or(0),
            agent: r["agent"].as_str().unwrap_or("").into(),
            machine: r["machine"].as_str().unwrap_or("").into(),
            project: r["project"].as_str().unwrap_or("").into(),
            s: serde_json::from_value(r["s"].clone()).unwrap_or_default(),
        })
        .collect()
}

fn date(ms: f64) -> js_sys::Date {
    js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms))
}
/// Local midnight of the day `ms` falls in.
fn midnight(ms: f64) -> f64 {
    let d = date(ms);
    d.set_hours(0);
    d.set_minutes(0);
    d.set_seconds(0);
    d.set_milliseconds(0);
    d.get_time()
}
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEK: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

fn agent_label(id: &str) -> String {
    AgentKind::parse(id).map(|a| a.label().to_string()).unwrap_or_else(|| id.to_string())
}
fn window_label(w: &str) -> String {
    match w {
        "5h" => "5-hour".into(),
        "weekly" | "7d" => "Weekly".into(),
        "opus-weekly" => "Opus weekly".into(),
        "sonnet-weekly" => "Sonnet weekly".into(),
        "plan" => "Plan (month)".into(),
        "on-demand" => "On-demand spend".into(),
        w => {
            let w = w.replace(['-', '_'], " ");
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}
fn resets(at: Option<i64>) -> String {
    let Some(at) = at else { return String::new() };
    let left = (at - now_ms()) / 60_000;
    if left <= 0 {
        return "resetting".into();
    }
    if left < 60 {
        return format!("resets in {left}m");
    }
    if left < 24 * 60 {
        return format!("resets in {}h {}m", left / 60, left % 60);
    }
    let d = date(at as f64);
    format!("resets {} {:02}:{:02}", WEEK[d.get_day() as usize % 7], d.get_hours(), d.get_minutes())
}
fn money(c: f64) -> String {
    if c == 0.0 {
        "$0".into()
    } else if c < 0.001 {
        "<$0.001".into()
    } else if c < 1.0 {
        format!("${c:.3}")
    } else if c < 100.0 {
        format!("${c:.2}")
    } else {
        format!("${c:.0}")
    }
}
fn tone(pct: f64) -> &'static str {
    if pct >= 90.0 { "bad" } else if pct >= 70.0 { "warn" } else { "ok" }
}
fn base(p: &str) -> String {
    p.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(p).to_string()
}

fn page() -> AnyView {
    view! { <UsagePage/> }.into_any()
}

#[component]
fn UsagePage() -> impl IntoView {
    let limits = RwSignal::new(Vec::<AgentLimits>::new());
    let names = RwSignal::new(BTreeMap::<String, String>::new());
    let hours = RwSignal::new(Vec::<Row>::new());
    let total = RwSignal::new(Vec::<Row>::new());
    let loaded = RwSignal::new(false);
    let by_cost = RwSignal::new(false);
    let load = move || {
        spawn_local(async move {
            match rpc::<Value>("usage", "summary", &json!({})).await {
                Ok(v) => {
                    hours.set(rows(&v["hours"]));
                    total.set(rows(&v["total"]));
                    // Windows the hub kept from before a restart, where the live ones have none.
                    let kept: Vec<AgentLimits> = serde_json::from_value(v["limits"].clone()).unwrap_or_default();
                    limits.update(|all| {
                        for k in kept {
                            if !all.iter().any(|x| x.agent == k.agent && x.key() == k.key()) {
                                all.push(k);
                            }
                        }
                    });
                }
                Err(e) => toast_bad(e.message),
            }
            loaded.set(true);
        });
    };
    spawn_local(async move {
        if let Ok(o) = api_get::<Overview>("/code/overview").await {
            names.set(o.machines.iter().map(|m| (m.id.clone(), m.name.clone())).collect());
            limits.update(|all| {
                for l in o.limits.into_iter().rev() {
                    put_limits(all, l);
                }
            });
        }
    });
    load();
    subscribe(&["plugin:usage", "threads"]);
    // Many turns can end together: one reload a second at most.
    let pending = StoredValue::new_local(None::<gloo_timers::callback::Timeout>);
    on_ws(move |m| match m {
        WsOut::Limits { limits: l } => limits.update(|all| put_limits(all, l.clone())),
        WsOut::Plugin { plugin, payload } if plugin == "usage" && payload["changed"] == true => {
            if pending.with_value(|p| p.is_none()) {
                pending.set_value(Some(gloo_timers::callback::Timeout::new(1000, move || {
                    pending.set_value(None);
                    load();
                })));
            }
        }
        _ => {}
    });
    // A clock for "resets in …".
    let tick = RwSignal::new(0u32);
    let clock = StoredValue::new_local(Some(gloo_timers::callback::Interval::new(30_000, move || tick.update(|t| *t += 1))));
    on_cleanup(move || clock.update_value(|c| drop(c.take())));

    let machine_name = move |id: &str| names.with(|n| n.get(id).cloned()).unwrap_or_else(|| id.to_string());

    // The last 14 local days: (midnight, totals).
    let days = Memo::new(move |_| {
        let today = midnight(now_ms() as f64);
        let starts: Vec<f64> = (0..DAYS).rev().map(|i| midnight(today - i as f64 * 86_400_000.0 + 3_600_000.0 * 2.0)).collect();
        let mut out: Vec<(f64, Stat)> = starts.iter().map(|d| (*d, Stat::default())).collect();
        hours.with(|hs| {
            for r in hs {
                let ms = (r.h * HOUR) as f64;
                if let Some(i) = starts.iter().rposition(|d| *d <= ms) {
                    out[i].1.merge(&r.s);
                }
            }
        });
        out
    });
    let since = move || days.with(|d| d.first().map(|x| x.0).unwrap_or(0.0)) as i64;
    let sum14 = Memo::new(move |_| days.with(|d| d.iter().fold(Stat::default(), |mut a, (_, s)| { a.merge(s); a })));
    // Per (machine, project): the last 14 days and all time.
    let projects = Memo::new(move |_| {
        let from = since();
        let mut m: BTreeMap<(String, String), (Stat, Stat)> = BTreeMap::new();
        hours.with(|hs| {
            for r in hs.iter().filter(|r| r.h * HOUR >= from) {
                m.entry((r.machine.clone(), r.project.clone())).or_default().0.merge(&r.s);
            }
        });
        total.with(|ts| {
            for r in ts {
                m.entry((r.machine.clone(), r.project.clone())).or_default().1.merge(&r.s);
            }
        });
        let mut v: Vec<_> = m.into_iter().collect();
        v.sort_by(|a, b| (b.1 .0.cost, b.1 .0.tokens(), b.1 .1.cost).partial_cmp(&(a.1 .0.cost, a.1 .0.tokens(), a.1 .1.cost)).unwrap_or(std::cmp::Ordering::Equal));
        v
    });
    let agents = Memo::new(move |_| {
        let from = since();
        let mut m: BTreeMap<String, Stat> = BTreeMap::new();
        hours.with(|hs| {
            for r in hs.iter().filter(|r| r.h * HOUR >= from) {
                m.entry(r.agent.clone()).or_default().merge(&r.s);
            }
        });
        m.into_iter().collect::<Vec<_>>()
    });

    view! {
        <div class="usage-page">
            <section class="usage-sec">
                <h3 class="usage-head">"Limits"</h3>
                {move || {
                    tick.track();
                    let l = limits.get();
                    if l.is_empty() {
                        return view! { <p class="small subdue">"No limits reported yet. Claude Code, Codex and Cursor report theirs (per account) as their turns run or when asked."</p> }.into_any();
                    }
                    view! {
                        <div class="usage-limits">
                            {l.into_iter().map(|a| view! {
                                <div class="usage-limit">
                                    <div class="row usage-limit-head">
                                        <Icon name="bot" size=14/>
                                        <b>{a.agent.label()}</b>
                                        <span class="small subdue grow">{match &a.account_label {
                                            // An account: its name, and the machines signed in to it.
                                            Some(who) => format!("{who} · {} · {}", if a.machines.is_empty() { machine_name(&a.machine) } else { a.machines.iter().map(|m| machine_name(m)).collect::<Vec<_>>().join(", ") }, ago_ms(a.at)),
                                            None => format!("on {} · {}", machine_name(&a.machine), ago_ms(a.at)),
                                        }}</span>
                                    </div>
                                    {a.limits.iter().map(window).collect_view()}
                                </div>
                            }).collect_view()}
                        </div>
                    }.into_any()
                }}
            </section>

            <section class="usage-sec">
                <div class="row" style="gap:8px">
                    <h3 class="usage-head grow">"Last 14 days"</h3>
                    <div class="seg usage-seg" role="group" aria-label="Chart by">
                        <button aria-pressed=move || (!by_cost.get()).to_string() on:click=move |_| by_cost.set(false)>"Tokens"</button>
                        <button aria-pressed=move || by_cost.get().to_string() on:click=move |_| by_cost.set(true)>"Cost"</button>
                    </div>
                </div>
                <div class="usage-metrics">
                    <div class="usage-metric"><small>"Tokens"</small><b>{move || tokens(sum14.get().tokens())}</b>
                        <span class="small subdue">{move || { let s = sum14.get(); format!("{} in · {} out · {} cached", tokens(s.input), tokens(s.output), tokens(s.cache_read + s.cache_write)) }}</span></div>
                    <div class="usage-metric"><small>"Cost"</small><b>{move || money(sum14.get().cost)}</b><span class="small subdue">"as the agents report it"</span></div>
                    <div class="usage-metric"><small>"Turns"</small><b>{move || sum14.get().turns.to_string()}</b></div>
                </div>
                {move || {
                    let d = days.get();
                    let cost = by_cost.get();
                    let val = |s: &Stat| if cost { s.cost } else { s.tokens() as f64 };
                    let max = d.iter().map(|(_, s)| val(s)).fold(0.0f64, f64::max);
                    view! {
                        <div class="usage-bars" role="img" aria-label="Usage per day">
                            {d.into_iter().enumerate().map(|(i, (day, s))| {
                                let v = val(&s);
                                let pct = if max > 0.0 { (v / max * 100.0).max(if v > 0.0 { 2.0 } else { 0.0 }) } else { 0.0 };
                                let dt = date(day);
                                let title = format!("{} {} {}: {} tokens · {} · {} turns", WEEK[dt.get_day() as usize % 7], MONTHS[dt.get_month() as usize % 12], dt.get_date(), tokens(s.tokens()), money(s.cost), s.turns);
                                let today = i as i64 == DAYS - 1;
                                view! {
                                    <div class=if today { "usage-day today" } else { "usage-day" } title=title>
                                        <span class="usage-val">{if v > 0.0 { if cost { money(v) } else { tokens(v as u64) } } else { String::new() }}</span>
                                        <span class="usage-col"><i style=format!("height:{pct:.1}%")/></span>
                                        <small>{dt.get_date()}</small>
                                        <small class="subdue">{WEEK[dt.get_day() as usize % 7].chars().next().map(String::from)}</small>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    }
                }}
            </section>

            <section class="usage-sec">
                <h3 class="usage-head">"By project"</h3>
                {move || {
                    let p = projects.get();
                    if p.is_empty() {
                        return view! { <p class="small subdue">{move || if loaded.get() { "No turns yet." } else { "Adding up…" }}</p> }.into_any();
                    }
                    view! {
                        <div class="usage-table"><table>
                            <thead><tr><th>"Project"</th><th class="num">"Turns"</th><th class="num">"Tokens"</th><th class="num">"Cost"</th><th class="num">"All time"</th></tr></thead>
                            <tbody>
                                {p.into_iter().map(|((machine, project), (d14, all))| view! {
                                    <tr>
                                        <td><span class="hi" title=project.clone()>{base(&project)}</span><span class="small subdue">{format!(" · {}", machine_name(&machine))}</span></td>
                                        <td class="num">{d14.turns}</td>
                                        <td class="num">{tokens(d14.tokens())}</td>
                                        <td class="num">{money(d14.cost)}</td>
                                        <td class="num subdue">{format!("{} · {}", tokens(all.tokens()), money(all.cost))}</td>
                                    </tr>
                                }).collect_view()}
                            </tbody>
                        </table></div>
                    }.into_any()
                }}
            </section>

            <Show when=move || agents.with(|a| a.len() > 1)>
                <section class="usage-sec">
                    <h3 class="usage-head">"By agent"</h3>
                    <div class="usage-metrics">
                        {move || agents.get().into_iter().map(|(a, s)| view! {
                            <div class="usage-metric"><small>{agent_label(&a)}</small><b>{money(s.cost)}</b><span class="small subdue">{format!("{} tokens · {} turns", tokens(s.tokens()), s.turns)}</span></div>
                        }).collect_view()}
                    </div>
                </section>
            </Show>
        </div>
    }
}

fn window(w: &RateLimit) -> impl IntoView + use<> {
    let pct = w.used_percent.clamp(0.0, 100.0);
    view! {
        <div class=format!("usage-win {}", tone(pct))>
            <span class="usage-win-name">{window_label(&w.window)}</span>
            <span class="usage-win-bar" role="meter" aria-valuemin="0" aria-valuemax="100" aria-valuenow=format!("{pct:.0}")><i style=format!("width:{pct:.1}%")/></span>
            <span class="usage-win-pct">{format!("{pct:.0}%")}</span>
            <span class="small subdue usage-win-reset">{resets(w.resets_at)}</span>
        </div>
    }
}
