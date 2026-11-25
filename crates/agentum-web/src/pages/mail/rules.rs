//! Mail › Rules: the list (with quick-start presets), the editor (preview, apply now, delete, save)
//! and the activity log with Undo.
use super::inbox::Row;
use super::lib::*;
use super::{use_mail, List};
use agentum_ui::prelude::*;
use std::collections::BTreeSet;

fn presets() -> Vec<(&'static str, Value)> {
    vec![
        ("Newsletters → archive", json!({"category": "newsletter"}), json!([{"type": "archive"}])),
        ("Receipts → label Recibos", json!({"category": "receipt"}), json!([{"type": "label", "label": "Recibos"}])),
        ("Marketing → archive + read", json!({"category": "marketing"}), json!([{"type": "archive"}, {"type": "mark_read"}])),
        ("Codes → label", json!({"category": "otp"}), json!([{"type": "label", "label": "Codes"}])),
    ]
    .into_iter()
    .map(|(n, c, a)| (n, json!({"name": n, "conditions": c, "actions": a})))
    .collect()
}
fn every(s: i64) -> String {
    if s < 120 { format!("{s} seconds") } else { format!("{} minutes", (s as f64 / 60.0).round()) }
}

#[component]
pub fn Rules() -> impl IntoView {
    let c = use_mail();
    let rules = RwSignal::new(None::<Vec<Value>>);
    let log = RwSignal::new(Vec::<Value>::new());
    let err = RwSignal::new(String::new());
    let edit = RwSignal::new(None::<Value>);
    let load = move || {
        spawn_local(async move {
            let (r, l) = (api_get::<Value>("/mail/rules").await, api_get::<Value>("/mail/log?limit=60").await);
            match (r, l) {
                (Ok(r), Ok(l)) => {
                    let _ = rules.try_set(Some(r["rules"].as_array().cloned().unwrap_or_default()));
                    let _ = log.try_set(l["log"].as_array().cloned().unwrap_or_default());
                }
                (Err(e), _) | (_, Err(e)) => {
                    let _ = err.try_set(e.message);
                }
            }
        });
    };
    Effect::new(move |_| {
        c.ver.track();
        load();
    });
    let toggle = move |r: Value, on: bool| {
        let id = r["id"].clone();
        let flip = move |v: bool| rules.update(|x| if let Some(x) = x { for y in x.iter_mut().filter(|y| y["id"] == id) { y["enabled"] = json!(v) } });
        flip(on);
        spawn_local(async move {
            let mut body = r.clone();
            body["enabled"] = json!(on);
            match api_post::<Value>("/mail/rules", &body).await {
                Ok(_) => toast(if on { "Rule on" } else { "Rule off" }),
                Err(e) => {
                    toast_bad(e.message);
                    flip(!on);
                }
            }
        });
    };
    let undo = move |id: String| {
        spawn_local(async move {
            match api_post::<Value>("/mail/undo", &json!({"id": id})).await {
                Ok(r) if r["ok"] == true => {
                    toast("Undone");
                    load();
                }
                Ok(r) => toast_bad(r["error"].as_str().unwrap_or("Could not undo").to_string()),
                Err(e) => toast_bad(e.message),
            }
        });
    };
    view! {
        {move || {
            if let Some(r) = edit.get() {
                return view! { <Editor r=r on_close=Callback::new(move |changed: bool| { edit.set(None); if changed { load() } })/> }.into_any();
            }
            if !err.get().is_empty() && rules.with(Option::is_none) {
                return view! { <div class="empty err">{err.get()}</div> }.into_any();
            }
            let Some(list) = rules.get() else { return view! { <div class="empty">"Loading…"</div> }.into_any() };
            let interval = c.meta.with(|m| m.as_ref().map(|m| m.interval).filter(|i| *i > 0).unwrap_or(300));
            view! {
                <div class="ml-rules">
                    <div class="ml-bar">
                        <button type="button" class="btn primary" on:click=move |_| edit.set(Some(json!({"name": ""})))><Icon name="plus" size=14/>"New rule"</button>
                        <span class="subdue small">{format!("Rules run on new mail every {}. Preview first; everything is logged and can be undone.{}", every(interval), if c.llm() { "" } else { " AI instructions activate once your own model is connected." })}</span>
                    </div>
                    <div class="ml-bar"><span class="subdue">"Quick start:"</span>
                        {presets().into_iter().map(|(n, r)| view! { <button type="button" class="btn ghost" on:click=move |_| edit.set(Some(r.clone()))>{n}</button> }).collect_view()}
                    </div>
                    <ul class="ml-rulelist">
                        {if list.is_empty() {
                            view! { <li class="empty">"No rules yet. Start with a quick-start above or make your own."</li> }.into_any()
                        } else {
                            list.into_iter().map(|r| {
                                let (r1, r2) = (r.clone(), r.clone());
                                let on = r["enabled"] == true;
                                view! {
                                    <li>
                                        <label class="ml-switch" title=if on { "Runs automatically" } else { "Off" }><input type="checkbox" prop:checked=on on:change=move |e| toggle(r1.clone(), event_target_checked(&e))/><span/></label>
                                        <span class="hi">{r["name"].as_str().unwrap_or("").to_string()}</span>
                                        <span class="subdue grow truncate">{describe(&r)}</span>
                                        <button type="button" class="btn ghost" on:click=move |_| edit.set(Some(r2.clone()))><Icon name="edit" size=13/>"Edit"</button>
                                    </li>
                                }
                            }).collect_view().into_any()
                        }}
                    </ul>
                    <h3>"Activity"</h3>
                    <ul class="ml-log">
                        {move || {
                            let l = log.get();
                            if l.is_empty() {
                                return view! { <li class="empty">"Nothing has run yet."</li> }.into_any();
                            }
                            l.into_iter().map(|e| {
                                let id = e["id"].as_str().map(str::to_string).unwrap_or_else(|| e["id"].to_string());
                                let undone = e["undone"] == true || e["undone"].as_i64().is_some_and(|n| n > 0);
                                let act = e["action"].as_str().unwrap_or("").to_string();
                                let dst = e["dst"].as_str().filter(|d| !d.is_empty()).map(|d| format!(" → {d}")).unwrap_or_default();
                                view! {
                                    <li class=if undone { "undone" } else { "" }>
                                        <span class="subdue">{ago(e["ts"].as_i64().unwrap_or(0))}</span><b>{e["rule"].as_str().unwrap_or("").to_string()}</b>
                                        <span>{format!("{}{dst}", action_label(&act))}</span>
                                        <span class="grow truncate subdue">{e["subject"].as_str().unwrap_or("").to_string()}</span>
                                        {if e["status"] == "error" {
                                            view! { <span class="err" title=e["detail"].as_str().unwrap_or("").to_string()>"error"</span> }.into_any()
                                        } else if undone {
                                            view! { <span class="subdue">"undone"</span> }.into_any()
                                        } else {
                                            view! { <button type="button" class="btn ghost small" on:click=move |_| undo(id.clone())><Icon name="undo" size=13/>"Undo"</button> }.into_any()
                                        }}
                                    </li>
                                }
                            }).collect_view().into_any()
                        }}
                    </ul>
                </div>
            }.into_any()
        }}
    }
}

#[derive(Clone, PartialEq)]
struct Act {
    kind: String,
    value: String,
}

#[component]
fn Editor(r: Value, on_close: Callback<bool>) -> impl IntoView {
    let c = use_mail();
    let cond = r["conditions"].clone();
    let g = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    let name = RwSignal::new(g(&r, "name"));
    let operator = RwSignal::new(if r["operator"] == "OR" { "OR" } else { "AND" }.to_string());
    let from = RwSignal::new(g(&cond, "from"));
    let to = RwSignal::new(g(&cond, "to"));
    let subject = RwSignal::new(g(&cond, "subject"));
    let category = RwSignal::new(g(&cond, "category"));
    let instructions = RwSignal::new(g(&r, "instructions"));
    let acts0: Vec<Act> = r["actions"].as_array().filter(|a| !a.is_empty()).map(|a| a.iter().map(|x| Act { kind: g(x, "type"), value: if x["label"].is_string() { g(x, "label") } else { g(x, "folder") } }).collect()).unwrap_or_else(|| vec![Act { kind: "archive".into(), value: String::new() }]);
    let acts = RwSignal::new(acts0);
    let accts0: BTreeSet<String> = match r["accounts"].as_array().filter(|a| !a.is_empty()) {
        Some(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        None => c.accounts().into_iter().map(|a| a.id).collect(),
    };
    let accts = RwSignal::new(accts0);
    let enabled = RwSignal::new(r["enabled"] == true);
    // None: nothing; Some(Err): loading ("") or an error; Some(Ok): the matches.
    let preview = RwSignal::new(None::<Result<(Vec<Msg>, u64), String>>);
    let busy = RwSignal::new(false);
    let noop: List = RwSignal::new(Some(vec![]));
    c.pane("editor", Signal::derive(|| 1), Callback::new(move |_| on_close.run(false)), None);
    let has_id = !r["id"].is_null();
    let r0 = r.clone();
    let rule = move || -> Value {
        let n = c.accounts().len();
        let a = accts.get_untracked();
        json!({
            "id": r0["id"], "created": r0["created"], "name": name.get_untracked(), "operator": operator.get_untracked(), "enabled": enabled.get_untracked(), "instructions": instructions.get_untracked(),
            "conditions": {"from": from.get_untracked(), "to": to.get_untracked(), "subject": subject.get_untracked(), "category": category.get_untracked()},
            "actions": acts.get_untracked().into_iter().map(|a| match a.kind.as_str() {
                "label" => json!({"type": a.kind, "label": a.value.trim()}),
                "move" => json!({"type": a.kind, "folder": a.value.trim()}),
                _ => json!({"type": a.kind}),
            }).collect::<Vec<_>>(),
            // all ticked = every account, now and later
            "accounts": if a.len() == n { vec![] } else { a.into_iter().collect::<Vec<_>>() },
        })
    };
    let rule2 = rule.clone();
    let do_preview = move |_| {
        preview.set(Some(Err(String::new())));
        let mut body = rule2();
        body["ai"] = json!(c.llm());
        spawn_local(async move {
            let r = api_post::<Value>("/mail/rules/preview", &body).await;
            let _ = preview.try_set(Some(r.map(|v| (Msg::list(&v["matches"]), v["scanned"].as_u64().unwrap_or(0))).map_err(|e| e.message)));
        });
    };
    let rule3 = rule.clone();
    let apply = move |_| {
        let x = rule3();
        spawn_local(async move {
            if !confirm("Apply this rule to your inbox now?", format!("{}\nIt acts on the latest 300 inbox emails that match. Everything is logged and can be undone.", describe(&x)), "Apply now", false).await {
                return;
            }
            busy.set(true);
            match api_post::<Value>("/mail/rules/apply", &x).await {
                Ok(res) => {
                    let n = res["errors"].as_array().map_or(0, |e| e.len());
                    let t = format!("Applied {} action(s){}", res["applied"].as_u64().unwrap_or(0), if n > 0 { format!(", {n} errors") } else { String::new() });
                    if n > 0 { toast_bad(t) } else { toast(t) }
                }
                Err(e) => toast_bad(e.message),
            }
            let _ = busy.try_set(false);
        });
    };
    let rname = g(&r, "name");
    let rid = r["id"].clone();
    let del = move |_| {
        let (rname, rid) = (rname.clone(), rid.clone());
        spawn_local(async move {
            if !confirm(format!("Delete the rule \"{rname}\"?"), "", "Delete", true).await {
                return;
            }
            match api_post::<Value>("/mail/rules/delete", &json!({"id": rid})).await {
                Ok(_) => on_close.run(true),
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let save = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let x = rule();
        spawn_local(async move {
            match api_post::<Value>("/mail/rules", &x).await {
                Ok(_) => {
                    toast("Rule saved");
                    on_close.run(true);
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let field = |label: &'static str, sig: RwSignal<String>, ph: &'static str| view! {
        <label>{label}" "<input prop:value=sig on:input=move |e| sig.set(event_target_value(&e)) placeholder=ph/></label>
    };
    view! {
        <div class="ml-editor">
            <div class="ml-pbar">
                <button type="button" class="btn ghost" on:click=move |_| on_close.run(false)><Icon name="chevronL" size=14/>"Rules"</button>
                <span class="hi">{if has_id { "Edit rule" } else { "New rule" }}</span>
            </div>
            <form class="ml-form" on:submit=save>
                <label>"Name "<input prop:value=name on:input=move |e| name.set(event_target_value(&e)) required placeholder="e.g. Newsletters to archive"/></label>
                <fieldset>
                    <legend>"When an email matches "
                        <select class="pick-field pick-inline" aria-label="Match" on:change=move |e| operator.set(event_target_value(&e))>
                            <option value="AND" selected=move || operator.get() == "AND">"all"</option>
                            <option value="OR" selected=move || operator.get() == "OR">"any"</option>
                        </select>" of:"
                    </legend>
                    {field("From contains", from, "acquire.com | noreply@turo.com")}
                    {field("To contains", to, "optional")}
                    {field("Subject contains", subject, "factura | invoice")}
                    <label>"Category "
                        <select class="pick-field" aria-label="Category" on:change=move |e| category.set(event_target_value(&e))>
                            <option value="" selected=move || category.get().is_empty()>"any"</option>
                            {CAT.iter().map(|(k, l)| view! { <option value=*k selected=move || category.get() == *k>{*l}</option> }).collect_view()}
                        </select>
                    </label>
                    <label>"AI instructions "<textarea rows="2" prop:value=instructions on:input=move |e| instructions.set(event_target_value(&e)) placeholder="e.g. Emails from recruiters or about job offers" title=move || if c.llm() { String::new() } else { "Needs your own model connected".into() }/></label>
                    <p class="subdue small">"Separate alternatives with | . Matching is case-insensitive."</p>
                </fieldset>
                <fieldset>
                    <legend>"Then"</legend>
                    {move || acts.get().into_iter().enumerate().map(|(i, a)| {
                        let kind = a.kind.clone();
                        view! {
                            <div class="row">
                                <span style="min-width:170px">
                                    <select class="pick-field" aria-label="Action" on:change=move |e| { let v = event_target_value(&e); acts.update(|x| x[i].kind = v) }>
                                        {RULE_ACTIONS.iter().map(|k| view! { <option value=*k selected=kind == *k>{action_label(k).to_string()}</option> }).collect_view()}
                                    </select>
                                </span>
                                {(a.kind == "label" || a.kind == "move").then(|| view! {
                                    <input prop:value=a.value.clone() on:change=move |e| { let v = event_target_value(&e); acts.update(|x| x[i].value = v) } placeholder=if a.kind == "label" { "label" } else { "folder" }/>
                                })}
                                <button type="button" class="icon-btn" title="Remove" aria-label="Remove" on:click=move |_| acts.update(|x| { x.remove(i); })><Icon name="x" size=13/></button>
                            </div>
                        }
                    }).collect_view()}
                    <div><button type="button" class="btn ghost" on:click=move |_| acts.update(|x| x.push(Act { kind: "mark_read".into(), value: String::new() }))><Icon name="plus" size=13/>"Add action"</button></div>
                </fieldset>
                <fieldset>
                    <legend>"Accounts"</legend>
                    <div class="row" style="flex-wrap:wrap">
                        {move || c.accounts().into_iter().map(|a| { let id = a.id.clone(); let id2 = a.id.clone(); view! {
                            <label class="row"><input type="checkbox" prop:checked=move || accts.with(|s| s.contains(&id)) on:change=move |e| { let on = event_target_checked(&e); let id = id2.clone(); accts.update(|s| { if on { s.insert(id); } else { s.remove(&id); } }) }/>" "{a.name}</label>
                        }}).collect_view()}
                    </div>
                </fieldset>
                <div class="ml-formbar">
                    <button type="button" class="btn" on:click=do_preview><Icon name="eye" size=14/>"Preview matches"</button>
                    <button type="button" class="btn" disabled=move || busy.get() on:click=apply>{move || if busy.get() { "Applying…" } else { "Apply to inbox now" }}</button>
                    <label class="row"><input type="checkbox" prop:checked=enabled on:change=move |e| enabled.set(event_target_checked(&e))/>" Run automatically"</label>
                    <span class="grow"/>
                    {has_id.then(|| view! { <button type="button" class="btn danger" on:click=del.clone()><Icon name="trash" size=14/>"Delete"</button> })}
                    <button type="submit" class="btn primary">"Save"</button>
                </div>
            </form>
            {move || match preview.get() {
                None => ().into_any(),
                Some(Err(e)) if e.is_empty() => view! { <div class="empty">"Checking recent mail…"</div> }.into_any(),
                Some(Err(e)) => view! { <div class="empty err">{e}</div> }.into_any(),
                Some(Ok((ms, scanned))) => view! {
                    <p class="subdue small">{format!("{} of the last {scanned} emails match. Nothing was changed.", ms.len())}</p>
                    <ul class="ml-rows">{ms.iter().map(|m| view! { <Row t=single(m) set=noop selectable=false actions=false/> }).collect_view()}</ul>
                }.into_any(),
            }}
        </div>
    }
}
