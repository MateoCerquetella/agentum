//! Machines: the computers your coding agents run on. Add one (its install command shows once,
//! with the token in it), pick the main one (the default for new threads), remove one.
use agentum_proto::api::Machine;
use agentum_ui::prelude::*;

fn copy(text: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.navigator().clipboard().write_text(text);
        toast("Copied.");
    }
}

#[component]
pub fn Machines() -> impl IntoView {
    let list = RwSignal::new(None::<Vec<Machine>>);
    let name = RwSignal::new(String::new());
    let adding = RwSignal::new(false);
    let fresh = RwSignal::new(None::<Value>);
    let load = move || {
        spawn_local(async move {
            match api_get::<Vec<Machine>>("/code/machines").await {
                Ok(m) => list.set(Some(m)),
                Err(e) => toast_bad(e.message),
            }
        })
    };
    load();
    // Online / offline changes arrive with the Code section's live overview.
    crate::code::store::start();
    let ov = crate::code::store::overview();
    let seen = Memo::new(move |_| ov.with(|o| o.as_ref().map(|o| o.machines.iter().map(|m| (m.id.clone(), m.online)).collect::<Vec<_>>())));
    Effect::new(move |prev: Option<()>| {
        seen.track();
        if prev.is_some() {
            load();
        }
    });
    let add = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let n = name.get_untracked().trim().to_string();
        if n.is_empty() {
            return;
        }
        adding.set(true);
        spawn_local(async move {
            match api_post::<Value>("/code/machines", &json!({"name": n})).await {
                Ok(r) => {
                    fresh.set(Some(r));
                    name.set(String::new());
                    load();
                }
                Err(e) => toast_bad(e.message),
            }
            adding.set(false);
        });
    };
    let act = move |path: String, method: &'static str| {
        spawn_local(async move {
            let empty = json!({});
            match api::<Value>(&path, method, (method == "POST").then_some(&empty)).await {
                Ok(_) => load(),
                Err(e) => toast_bad(e.message),
            }
        })
    };
    view! {
        <div class="col" style="gap:14px">
            {move || fresh.get().map(|r| {
                let cmd = r["install"].as_str().unwrap_or("").to_string();
                let c2 = cmd.clone();
                view! {
                    <div class="card col ag-install" style="padding:14px;gap:8px">
                        <div class="row"><Icon name="terminal" size=15/><b class="grow">{format!("Install agentd on “{}”", r["id"].as_str().unwrap_or(""))}</b>
                            <button class="icon-btn" title="Done" aria-label="Done" on:click=move |_| fresh.set(None)><Icon name="x"/></button></div>
                        <div class="small muted">"Run this on the machine. It holds the machine's token, which shows only now: copy it before you close this."</div>
                        <div class="row ag-cmd"><code class="grow">{cmd}</code>
                            <button class="btn" on:click=move |_| copy(&c2)><Icon name="copy" size=13/>"Copy"</button></div>
                    </div>
                }
            })}
            <form class="row" on:submit=add>
                <input class="grow" placeholder="A name for the new machine (laptop, mars…)" maxlength="40" prop:value=name on:input=move |e| name.set(event_target_value(&e))/>
                <button class="btn primary" disabled=move || adding.get() || name.get().trim().is_empty()><Icon name="plus" size=13/>"Add machine"</button>
            </form>
            {move || match list.get() {
                None => view! { <div class="muted small">"Loading…"</div> }.into_any(),
                Some(v) if v.is_empty() => view! { <div class="muted small">"No machines yet. Add one, then run its install command there."</div> }.into_any(),
                Some(v) => v.into_iter().map(|m| {
                    let (id1, id2, id3) = (m.id.clone(), m.id.clone(), m.name.clone());
                    let detail = [m.hostname.clone(), [m.os.clone(), m.arch.clone()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" "), (!m.version.is_empty()).then(|| format!("agentd {}", m.version)).unwrap_or_default()]
                        .into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                    let seen = if m.online { "online".to_string() } else { m.last_seen.map(|t| format!("seen {} ago", ago_ms(t))).unwrap_or_else(|| "never connected".into()) };
                    let agents: Vec<String> = m.agents.iter().filter(|a| a.installed).map(|a| a.agent.label().to_string()).collect();
                    view! {
                        <div class="set-row ag-machine" style="align-items:center">
                            <span class="ag-dot" style=if m.online { "background:var(--positive)" } else { "background:var(--subdue)" } title=seen.clone()/>
                            <div class="grow col" style="gap:2px;min-width:0">
                                <div class="row" style="gap:6px"><span class="hi">{m.name.clone()}</span>
                                    {m.main.then(|| view! { <span class="chip on">"main"</span> })}
                                    <span class="small subdue">{seen}</span></div>
                                <div class="small subdue truncate">{detail}</div>
                                <div class="small muted truncate">{format!("{} projects", m.projects.len())}{(!agents.is_empty()).then(|| format!(" · {}", agents.join(", ")))}</div>
                            </div>
                            {(!m.main).then(|| view! { <button class="btn ghost small" title="The default machine for new threads" on:click=move |_| act(format!("/code/machines/{}/main", id1), "POST")>"Make main"</button> })}
                            <button class="btn ghost small danger" on:click=move |_| {
                                let (id, name) = (id2.clone(), id3.clone());
                                spawn_local(async move {
                                    if confirm(format!("Remove “{name}”?"), "Its agentd can't connect again until you add it anew. Its threads stay here.", "Remove", true).await {
                                        act(format!("/code/machines/{id}"), "DELETE");
                                    }
                                });
                            }>"Remove"</button>
                        </div>
                    }
                }).collect_view().into_any(),
            }}
        </div>
    }
}
