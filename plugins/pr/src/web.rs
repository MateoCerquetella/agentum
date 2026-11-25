//! The PR plugin's web part: the PR tab (state, review, checks, merge) and the sidebar badge.
use super::{check_counts, check_state};
use agentum_plugin::web::{RailTab, ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::{ThreadSummary, WsOut};
use agentum_ui::prelude::*;
use std::collections::HashMap;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

static TABS: [RailTab; 1] = [RailTab { id: "pr", title: "Pull request", icon: "github", view: tab }];

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn rail_tabs(&self) -> &'static [RailTab] {
        &TABS
    }
    fn thread_row(&self, t: &ThreadSummary) -> Option<AnyView> {
        listen();
        let id = t.id.clone();
        Some(
            view! {
                {move || badges().with(|b| b.get(&id).cloned()).filter(|b| !b.is_null()).map(|b| {
                    let checks = b["checks"].as_str().unwrap_or("none").to_string();
                    let state = b["state"].as_str().unwrap_or("").to_lowercase();
                    let mark = match checks.as_str() { "pass" => "✓", "fail" => "✗", "pending" => "●", _ => "" };
                    let title = format!("PR #{} · {}{}", b["number"], state, match checks.as_str() { "pass" => " · checks passed", "fail" => " · checks failed", "pending" => " · checks running", _ => "" });
                    view! {
                        <span class=format!("pr-row pr-{state} pr-c-{checks}") title=title>
                            {format!("#{}", b["number"])}
                            {(!mark.is_empty()).then(|| view! { <span class="pr-mark">{mark}</span> })}
                        </span>
                    }
                })}
            }
            .into_any(),
        )
    }
    fn css(&self) -> Option<&'static str> {
        Some(include_str!("../style.css"))
    }
}

thread_local! {
    /// Every thread's badge (from `badges`, then `plugin:pr`).
    static BADGES: RwSignal<HashMap<String, Value>> = global(HashMap::new());
    /// The latest PR seen per thread ({pr, at}), for open tabs.
    static PRS: RwSignal<HashMap<String, Value>> = global(HashMap::new());
    static LISTENING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn badges() -> RwSignal<HashMap<String, Value>> {
    BADGES.with(|b| *b)
}
fn prs() -> RwSignal<HashMap<String, Value>> {
    PRS.with(|b| *b)
}

/// Load the badges once, then follow `plugin:pr`, for the page's whole life.
fn listen() {
    if LISTENING.with(|l| l.replace(true)) {
        return;
    }
    let owner = Owner::new_root(None);
    owner.with(|| {
        subscribe(&["plugin:pr"]);
        on_ws(|m| {
            let WsOut::Plugin { plugin, payload } = m else { return };
            if plugin != "pr" {
                return;
            }
            let Some(t) = payload["thread"].as_str() else { return };
            badges().update(|b| {
                b.insert(t.to_string(), payload["badge"].clone());
            });
            if payload.get("pr").is_some() {
                prs().update(|p| {
                    p.insert(t.to_string(), json!({"pr": payload["pr"], "at": payload["at"]}));
                });
            }
        });
        spawn_local(async {
            if let Ok(v) = rpc::<HashMap<String, Value>>("pr", "badges", &json!({})).await {
                badges().update(|b| {
                    for (k, x) in v {
                        b.entry(k).or_insert(x);
                    }
                });
            }
        });
    });
    std::mem::forget(owner);
}

fn tab(cx: ThreadCx) -> AnyView {
    view! { <PrTab cx=cx/> }.into_any()
}

fn review_text(r: &str) -> Option<(&'static str, &'static str)> {
    match r {
        "APPROVED" => Some(("Approved", "pass")),
        "CHANGES_REQUESTED" => Some(("Changes requested", "fail")),
        "REVIEW_REQUIRED" => Some(("Review required", "pending")),
        _ => None,
    }
}

#[component]
fn PrTab(cx: ThreadCx) -> impl IntoView {
    listen();
    let id = cx.id.clone();
    let loading = RwSignal::new(false);
    let err = RwSignal::new(None::<String>);
    let merging = RwSignal::new(false);
    let method = RwSignal::new(agentum_ui::store::get("agentum.pr.method").filter(|m| ["squash", "merge", "rebase"].contains(&m.as_str())).unwrap_or_else(|| "squash".into()));
    let load = {
        let id = id.clone();
        Callback::new(move |refresh: bool| {
            let id = id.clone();
            loading.set(true);
            spawn_local(async move {
                let r = rpc::<Value>("pr", "get", &json!({"thread": id, "refresh": refresh})).await;
                if err.is_disposed() {
                    return;
                }
                match r {
                    Ok(v) => {
                        err.set(None);
                        prs().update(|p| {
                            p.insert(id, v);
                        });
                    }
                    Err(e) => err.set(Some(e.message)),
                }
                loading.set(false);
            });
        })
    };
    load.run(false);
    let data = {
        let id = id.clone();
        Memo::new(move |_| prs().with(|p| p.get(&id).cloned()))
    };
    let pr = move || data.get().map(|d| d["pr"].clone()).filter(|p| !p.is_null());
    let merge = {
        let id = id.clone();
        move |_| {
            let Some(p) = pr() else { return };
            let id = id.clone();
            let m = method.get_untracked();
            spawn_local(async move {
                let how = match m.as_str() { "merge" => "a merge commit", "rebase" => "rebasing its commits", _ => "squashing it into one commit" };
                if !confirm(&format!("Merge #{}?", p["number"]), &format!("“{}” goes into its base branch by {how}.", p["title"].as_str().unwrap_or("")), "Merge", false).await {
                    return;
                }
                if merging.is_disposed() {
                    return;
                }
                merging.set(true);
                match rpc::<Value>("pr", "merge", &json!({"thread": id, "method": m})).await {
                    Ok(v) => {
                        toast(format!("Merged #{}.", p["number"]));
                        if v.get("pr").is_some() {
                            prs().update(|x| {
                                x.insert(id, json!({"pr": v["pr"], "at": v["at"]}));
                            });
                        }
                    }
                    Err(e) => toast_bad(e.message),
                }
                if !merging.is_disposed() {
                    merging.set(false);
                }
            });
        }
    };
    view! {
        <div class="prx">
            <div class="prx-bar">
                <b>"Pull request"</b>
                <span class="small subdue">{move || data.get().and_then(|d| d["at"].as_i64()).map(|at| format!("checked {}", ago_ms(at))).unwrap_or_default()}</span>
                <span class="grow"></span>
                <button class="icon-btn" title="Ask GitHub again" on:click=move |_| load.run(true)>
                    <Icon name="refresh" size=14 class=Signal::derive(move || if loading.get() { "spin".to_string() } else { String::new() })/>
                </button>
            </div>
            <div class="prx-body">
                {move || err.get().map(|e| view! { <div class="err small prx-err">{e}</div> })}
                {move || match (data.get(), pr()) {
                    (None, _) => (err.with(|e| e.is_none())).then(|| view! { <div class="empty small">"Asking GitHub…"</div> }.into_any()),
                    (Some(_), None) => Some(view! {
                        <div class="empty small col" style="align-items:center;gap:4px">
                            <span>"No pull request for this branch."</span>
                            <span class="subdue">{move || cx.summary.with(|s| s.as_ref().and_then(|s| s.branch.clone())).map(|b| format!("Push {b} and open one (gh pr create), then refresh.")).unwrap_or_default()}</span>
                        </div>
                    }.into_any()),
                    (Some(_), Some(p)) => Some(pr_view(p).into_any()),
                }}
                {move || pr().filter(|p| p["state"] == "OPEN").map(|p| {
                    let conflicting = p["mergeable"] == "CONFLICTING";
                    view! {
                        <div class="prx-merge">
                            <select class="prx-pick" title="How to merge" on:change=move |e| { let v = event_target_value(&e); agentum_ui::store::set("agentum.pr.method", &v); method.set(v) }>
                                {[("squash", "Squash and merge"), ("merge", "Merge commit"), ("rebase", "Rebase and merge")].into_iter().map(|(v, l)| view! { <option value=v selected=move || method.get() == v>{l}</option> }).collect_view()}
                            </select>
                            <button class="btn primary prx-sm" disabled=move || merging.get() || conflicting || p["isDraft"].as_bool().unwrap_or(false) on:click=merge.clone()>
                                {move || if merging.get() { "Merging…" } else { "Merge" }}
                            </button>
                            {conflicting.then(|| view! { <span class="small err">"Has conflicts with its base"</span> })}
                        </div>
                    }
                })}
            </div>
        </div>
    }
}

fn pr_view(p: Value) -> impl IntoView {
    let state = p["state"].as_str().unwrap_or("").to_string();
    let draft = p["isDraft"].as_bool().unwrap_or(false);
    let label = if draft && state == "OPEN" { "Draft".to_string() } else { state.chars().next().map(|c| c.to_string() + &state[1..].to_lowercase()).unwrap_or_default() };
    let (pass, fail, pending) = check_counts(&p);
    let checks: Vec<Value> = p["statusCheckRollup"].as_array().cloned().unwrap_or_default();
    let url = p["url"].as_str().unwrap_or("").to_string();
    let review = review_text(p["reviewDecision"].as_str().unwrap_or(""));
    view! {
        <div class="prx-head">
            <div class="row" style="gap:6px;align-items:baseline">
                <span class=format!("prx-state prx-{}", if draft && state == "OPEN" { "draft".to_string() } else { state.to_lowercase() })>{label}</span>
                <a class="prx-num" href=url.clone() target="_blank" rel="noopener">{format!("#{}", p["number"])}</a>
            </div>
            <a class="prx-title" href=url.clone() target="_blank" rel="noopener">{p["title"].as_str().unwrap_or("").to_string()}</a>
            <div class="row small subdue" style="gap:10px;flex-wrap:wrap">
                <span><Icon name="branch" size=12/>" "{p["headRefName"].as_str().unwrap_or("").to_string()}</span>
                {review.map(|(t, c)| view! { <span class=format!("prx-c-{c}")>{t}</span> })}
                <a href=url target="_blank" rel="noopener">"Open on GitHub ↗"</a>
            </div>
        </div>
        <div class="prx-checks">
            <div class="row small" style="gap:10px;padding:6px 10px;border-bottom:1px solid var(--border)">
                <b>"Checks"</b>
                {(pass > 0).then(|| view! { <span class="prx-c-pass">{format!("{pass} passed")}</span> })}
                {(fail > 0).then(|| view! { <span class="prx-c-fail">{format!("{fail} failed")}</span> })}
                {(pending > 0).then(|| view! { <span class="prx-c-pending">{format!("{pending} running")}</span> })}
                {checks.is_empty().then(|| view! { <span class="subdue">"None"</span> })}
            </div>
            {checks.into_iter().map(|c| {
                let s = check_state(&c);
                let name = c["name"].as_str().or(c["context"].as_str()).unwrap_or("check").to_string();
                let wf = c["workflowName"].as_str().filter(|w| !w.is_empty()).map(|w| format!("{w} · "));
                let link = c["detailsUrl"].as_str().or(c["targetUrl"].as_str()).unwrap_or("").to_string();
                let mark = match s { "pass" => "✓", "fail" => "✗", "pending" => "●", _ => "–" };
                view! {
                    <a class="prx-check" href=link target="_blank" rel="noopener">
                        <span class=format!("prx-mark prx-c-{s}")>{mark}</span>
                        <span class="truncate grow"><span class="subdue">{wf}</span>{name}</span>
                    </a>
                }
            }).collect_view()}
        </div>
    }
}
