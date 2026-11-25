//! The hub's own settings (CERQ_* keys, `GET /api/settings` → fields): one row per field of a
//! tab, each with the control its type asks for, a Reset to the default, and the save bar.
use super::Ctx;
use agentum_ui::prelude::*;

const KINDS: [&str; 9] = ["personal", "to_reply", "receipt", "calendar", "notification", "otp", "newsletter", "marketing", "cold_email"];

#[component]
pub fn Fields(cx: Ctx, section: &'static str) -> impl IntoView {
    let fields = move || cx.d.with(|d| d.as_ref().and_then(|d| d["fields"].as_array().map(|a| a.iter().filter(|f| f["section"] == section).cloned().collect::<Vec<_>>())).unwrap_or_default());
    // Your model: list the server's models to pick from.
    if section == "model" {
        let url = cx.val("CERQ_MAIL_LLM_URL");
        spawn_local(async move {
            if let Ok(r) = api_post::<Value>("/settings/test-model", &json!({"url": url})).await {
                if r["ok"] == true {
                    cx.models.set(r["models"].as_array().into_iter().flatten().filter_map(|m| m.as_str().map(str::to_string)).collect());
                }
            }
        });
    }
    view! {
        {move || fields().into_iter().map(|f| {
            let key = f["key"].as_str().unwrap_or("").to_string();
            let base = f["base"].as_str().unwrap_or("").to_string();
            let k2 = key.clone();
            let b2 = base.clone();
            view! {
                <div class="set-row">
                    <div class="set-label">{f["label"].as_str().unwrap_or("").to_string()}{f["unit"].as_str().filter(|u| !u.is_empty()).map(|u| view! { <span class="subdue">" ("{u.to_string()}")"</span> })}</div>
                    <div class="set-ctl"><Control cx=cx f=f.clone()/></div>
                    <Show when={ let k = key.clone(); let b = base.clone(); move || cx.val(&k) != b }>
                        <button class="btn ghost small" title=format!("Default: {}", if b2.is_empty() { "(empty)" } else { &b2 }) on:click={ let k = k2.clone(); let b = b2.clone(); move |_| cx.set(&k, b.clone()) }>"Reset"</button>
                    </Show>
                </div>
            }
        }).collect_view()}
    }
}

#[component]
fn Control(cx: Ctx, f: Value) -> impl IntoView {
    let key = f["key"].as_str().unwrap_or("").to_string();
    let ty = f["type"].as_str().unwrap_or("text").to_string();
    let k = key.clone();
    let v = Signal::derive(move || cx.val(&k));
    let k = key.clone();
    let set = Callback::new(move |x: String| cx.set(&k, x));
    let set = move |x: String| set.run(x);
    let multi = move |opts: Vec<(String, String)>| {
        view! {
            <div class="row" style="flex-wrap:wrap">
                {opts.into_iter().map(|(o, label)| {
                    let o2 = o.clone();
                    let on = Signal::derive(move || v.get().split(',').any(|x| x.trim() == o2));
                    let o3 = o.clone();
                    view! {
                        <button type="button" class=move || if on.get() { "chip on" } else { "chip" } aria-pressed=move || on.get().to_string() on:click=move |_| {
                            let mut cur: Vec<String> = v.get_untracked().split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
                            if let Some(i) = cur.iter().position(|x| *x == o3) { cur.remove(i); } else { cur.push(o3.clone()); }
                            set(cur.join(","));
                        }>{label}</button>
                    }
                }).collect_view()}
            </div>
        }
        .into_any()
    };
    let select = move |opts: Vec<(String, String)>, width: &'static str| {
        view! {
            <select style=format!("width:{width}") on:change=move |e| set(event_target_value(&e))>
                {opts.into_iter().map(|(o, label)| { let o2 = o.clone(); view! { <option value=o selected=move || v.get() == o2>{label}</option> } }).collect_view()}
            </select>
        }
        .into_any()
    };
    match ty.as_str() {
        "select" => select(f["options"].as_array().into_iter().flatten().filter_map(|o| o.as_str()).map(|o| (o.to_string(), o.to_string())).collect(), "auto"),
        "time" => view! { <input type="time" prop:value=v on:input=move |e| set(event_target_value(&e))/> }.into_any(),
        "number" => {
            let (min, max) = (f["min"].as_f64().map(|n| n.to_string()), f["max"].as_f64().map(|n| n.to_string()));
            view! { <input type="number" step="any" min=min max=max style="width:140px" prop:value=v on:input=move |e| set(event_target_value(&e))/> }.into_any()
        }
        "calendar" => {
            let mut opts = vec![(String::new(), "First synced calendar".to_string())];
            cx.d.with_untracked(|d| {
                for c in d.as_ref().and_then(|d| d["calendars"].as_array()).into_iter().flatten().filter(|c| c["read_only"] != true) {
                    opts.push((c["id"].as_str().unwrap_or("").to_string(), c["name"].as_str().unwrap_or("").to_string()));
                }
            });
            select(opts, "auto")
        }
        "accounts" => {
            let mut accts: Vec<String> = cx.d.with_untracked(|d| d.as_ref().and_then(|d| d["calendars"].as_array()).into_iter().flatten().filter_map(|c| c["account"].as_str().filter(|a| !a.is_empty()).map(str::to_string)).collect());
            accts.sort();
            accts.dedup();
            multi(accts.into_iter().map(|a| (a.clone(), a)).collect())
        }
        "kinds" => multi(KINDS.iter().map(|k| (k.to_string(), k.replace('_', " "))).collect()),
        "model" => view! {
            <select style="width:280px" on:change=move |e| set(event_target_value(&e))>
                <option value="" selected=move || v.get().is_empty()>"—"</option>
                {move || {
                    let cur = v.get();
                    let mut all: Vec<String> = cx.models.get();
                    if !cur.is_empty() && !all.contains(&cur) { all.insert(0, cur.clone()); }
                    all.into_iter().map(|m| { let sel = m == cur; view! { <option value=m.clone() selected=sel>{m.clone()}</option> } }).collect_view()
                }}
            </select>
        }.into_any(),
        "url" => {
            let test = RwSignal::new(String::new());
            view! {
                <div class="row">
                    <input class="grow" prop:value=v on:input=move |e| set(event_target_value(&e))/>
                    <button class="btn" on:click=move |_| {
                        let url = v.get_untracked();
                        test.set("Testing…".into());
                        spawn_local(async move {
                            match api_post::<Value>("/settings/test-model", &json!({"url": url})).await {
                                Ok(r) if r["ok"] == true => {
                                    let models: Vec<String> = r["models"].as_array().into_iter().flatten().filter_map(|m| m.as_str().map(str::to_string)).collect();
                                    test.set(format!("Found {} models", models.len()));
                                    cx.models.set(models);
                                }
                                Ok(r) => test.set(r["error"].as_str().unwrap_or("It didn't answer.").to_string()),
                                Err(e) => test.set(e.message),
                            }
                        });
                    }>"Test"</button>
                    <Show when=move || !test.get().is_empty()><span class="small muted">{move || test.get()}</span></Show>
                </div>
            }.into_any()
        }
        "place" => {
            let q = RwSignal::new(String::new());
            let places = RwSignal::new(Vec::<Value>::new());
            let find = move || {
                let term = q.get_untracked();
                spawn_local(async move {
                    match api_get::<Value>(&format!("/settings/geocode?q={}", js_sys::encode_uri_component(&term))).await {
                        Ok(r) => places.set(r["places"].as_array().cloned().unwrap_or_default()),
                        Err(e) => toast_bad(e.message),
                    }
                });
            };
            let name_of = |p: &Value| ["name", "region", "country"].iter().filter_map(|k| p[*k].as_str().filter(|s| !s.is_empty())).collect::<Vec<_>>().join(", ");
            view! {
                <div class="col">
                    <form class="row" on:submit=move |e| { e.prevent_default(); find() }>
                        <input class="grow" placeholder=move || { let x = v.get(); if x.is_empty() { "City".to_string() } else { x } } prop:value=q on:input=move |e| q.set(event_target_value(&e))/>
                        <button class="btn" disabled=move || { q.get().trim().len() < 2 }><Icon name="search" size=13/>" Find"</button>
                    </form>
                    <Show when=move || !v.get().is_empty()><span class="small subdue">"Now: "{move || v.get()}</span></Show>
                    {move || places.get().into_iter().map(|p| {
                        let name = name_of(&p);
                        let tz = p["tz"].as_str().unwrap_or("").to_string();
                        let n2 = name.clone();
                        view! {
                            <button class="btn ghost" style="justify-content:flex-start" on:click=move |_| {
                                cx.set("CERQ_BRIEF_PLACE", n2.clone());
                                cx.set("CERQ_BRIEF_LAT", p["lat"].to_string());
                                cx.set("CERQ_BRIEF_LON", p["lon"].to_string());
                                if let Some(tz) = p["tz"].as_str().filter(|t| !t.is_empty()) { cx.set("CERQ_TZ", tz.to_string()); }
                                places.set(vec![]);
                                q.set(String::new());
                            }>{name}" "<span class="subdue small">{tz}</span></button>
                        }
                    }).collect_view()}
                </div>
            }.into_any()
        }
        _ => view! { <input style="width:100%" prop:value=v on:input=move |e| set(event_target_value(&e))/> }.into_any(),
    }
}
