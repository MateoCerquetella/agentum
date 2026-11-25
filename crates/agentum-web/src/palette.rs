//! ⌘K: jump to a page, a thread, a mail, an event, a note — or ask the Secretary.
//! Empty, it lists threads by when you last opened them (the one open now last), so ⌘K ⏎
//! flips back to the previous thread. Prefixes: `>` commands, `#` threads, `@` machines.
use crate::code::store::{overview, visited};
use crate::pages;
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;

#[derive(Clone)]
struct Hit {
    icon: &'static str,
    title: String,
    sub: String,
    go: Callback<()>,
}

thread_local! {
    static SHOWN: RwSignal<bool> = agentum_ui::global(false);
}

/// ⌘K, open or not (the sidebar's search button opens it too).
pub fn shown() -> RwSignal<bool> {
    SHOWN.with(|s| *s)
}

#[component]
pub fn Palette(on_close: Callback<()>) -> impl IntoView {
    let q = RwSignal::new(String::new());
    let sel = RwSignal::new(0usize);
    let res = RwSignal::new(Value::Null);
    let input = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        if let Some(i) = input.get() {
            let _ = i.focus();
        }
    });
    Effect::new(move |_| {
        let s = q.get();
        if s.trim().len() < 2 || s.starts_with('>') {
            res.set(Value::Null);
            return;
        }
        let enc = js_sys::encode_uri_component(s.trim_start_matches(['#', '@']));
        spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(150).await;
            if q.get_untracked() != s {
                return;
            }
            let mut out = api_get::<Value>(&format!("/search?q={enc}")).await.unwrap_or(json!({}));
            if let Ok(h) = api_get::<Value>(&format!("/code/search?q={enc}")).await {
                out["threads"] = h;
            }
            res.set(out);
        });
    });
    let ov = overview();
    let hits = Signal::derive(move || {
        let s = q.get();
        let lq = s.to_lowercase();
        let mut v: Vec<Hit> = vec![];
        let go = |page: &'static str, at: Where, params: Vec<(String, String)>| Callback::new(move |_| {
            let p: Vec<(&str, &str)> = params.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
            navigate(page, at, &p)
        });
        let threads = ov.with(|o| o.as_ref().map(|o| o.threads.clone()).unwrap_or_default());
        if lq.is_empty() {
            // Recently opened threads first (the open one last), then pages.
            let seen = visited();
            for id in seen.iter().rev().skip(1).chain(seen.last()) {
                if let Some(t) = threads.iter().find(|t| &t.id == id) {
                    v.push(Hit { icon: "bot", title: t.title.clone(), sub: format!("{} · {}", t.machine, t.agent.label()), go: go("code", Where::Replace, vec![("thread".into(), t.id.clone())]) });
                }
            }
        }
        let cmd = lq.strip_prefix('>');
        if cmd.is_none() && !lq.starts_with('#') && !lq.starts_with('@') {
            for d in pages::PAGES.iter().chain(crate::plugins::pages().iter()) {
                if lq.is_empty() || d.title.to_lowercase().contains(&lq) {
                    v.push(Hit { icon: d.icon, title: d.title.to_string(), sub: "Open".into(), go: go(d.id, Where::Replace, vec![]) });
                }
            }
        }
        let tq = lq.trim_start_matches('#');
        if !tq.is_empty() && !lq.starts_with('@') && cmd.is_none() {
            for t in threads.iter().filter(|t| t.title.to_lowercase().contains(tq) || t.project.to_lowercase().contains(tq)).take(8) {
                v.push(Hit { icon: "bot", title: t.title.clone(), sub: format!("{} · {}", t.machine, t.agent.label()), go: go("code", Where::Replace, vec![("thread".into(), t.id.clone())]) });
            }
        }
        if let Some(c) = cmd {
            let c = c.trim();
            for (title, icon, page, params) in [("New thread", "plus", "code", vec![("thread", "")]), ("New private chat", "lock", "chat", vec![("chat", "")]), ("Settings", "settings", "settings", vec![]), ("Plugins", "puzzle", "settings", vec![("tab", "plugins")]), ("Machines", "server", "settings", vec![("tab", "machines")])] {
                if c.is_empty() || title.to_lowercase().contains(c) {
                    v.push(Hit { icon, title: title.into(), sub: "Command".into(), go: go(page, Where::Replace, params.into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()) });
                }
            }
        }
        if let Some(m) = lq.strip_prefix('@') {
            for mach in ov.with(|o| o.as_ref().map(|o| o.machines.clone()).unwrap_or_default()).into_iter().filter(|x| x.name.to_lowercase().contains(m)) {
                v.push(Hit { icon: "server", title: format!("New thread on {}", mach.name), sub: if mach.online { "online".into() } else { "offline".into() }, go: go("code", Where::Replace, vec![("thread".into(), String::new()), ("machine".into(), mach.id.clone())]) });
            }
        }
        if !s.trim().is_empty() && cmd.is_none() {
            v.push(Hit { icon: "lock", title: format!("Ask the Secretary: {s}"), sub: String::new(), go: go("chat", Where::Right, vec![("q".into(), s.clone())]) });
        }
        res.with(|r| {
            for h in r["threads"].as_array().into_iter().flatten().take(8) {
                let (id, seq) = (h["thread"].as_str().unwrap_or("").to_string(), h["seq"].to_string());
                let excerpt = h["excerpt"].as_str().unwrap_or("").replace("<mark>", "").replace("</mark>", "");
                v.push(Hit { icon: "bot", title: h["title"].as_str().unwrap_or("Thread").into(), sub: excerpt, go: go("code", Where::Replace, vec![("thread".into(), id), ("at".into(), seq)]) });
            }
            for m in r["mail"].as_array().into_iter().flatten() {
                let msg = format!("{}:{}", m["account"].as_str().unwrap_or(""), m["uid"]);
                v.push(Hit { icon: "mail", title: m["subject"].as_str().unwrap_or("(no subject)").into(), sub: m["from_name"].as_str().or(m["from_addr"].as_str()).unwrap_or("").into(),
                    go: go("mail", Where::Replace, vec![("msg".into(), msg), ("folder".into(), m["folder"].as_str().unwrap_or("INBOX").into())]) });
            }
            for e in r["events"].as_array().into_iter().flatten() {
                v.push(Hit { icon: "calendar", title: e["title"].as_str().unwrap_or("").into(), sub: e["when"].as_str().unwrap_or("").into(), go: go("calendar", Where::Replace, vec![("date".into(), e["date"].as_str().unwrap_or("").into())]) });
            }
            for c in r["chats"].as_array().into_iter().flatten() {
                v.push(Hit { icon: "chat", title: c["name"].as_str().unwrap_or("Chat").into(), sub: c["snippet"].as_str().unwrap_or("").into(), go: go("chat", Where::Replace, vec![("chat".into(), c["id"].as_str().unwrap_or("").into())]) });
            }
            for n in r["notes"].as_array().into_iter().flatten() {
                v.push(Hit { icon: "notes", title: n["title"].as_str().unwrap_or("New note").into(), sub: if n["note_type"] == "todo" { "To-do".into() } else { "Note".into() },
                    go: go("notes", Where::Replace, if n["file"].is_null() { vec![] } else { vec![("note".into(), n["id"].as_str().unwrap_or("").into())] }) });
            }
        });
        v
    });
    let pick = move |i: usize| {
        if let Some(h) = hits.get_untracked().get(i) {
            h.go.run(());
            on_close.run(());
        }
    };
    view! {
        <div class="dialog-bg" style="align-items:flex-start;padding-top:12vh" on:mousedown=move |e| if e.target() == e.current_target() { on_close.run(()) }>
            <div class="dialog" style="padding:8px;gap:4px;width:min(600px,94vw)">
                <input node_ref=input prop:value=q placeholder="Threads, mail, events, notes… > commands · # threads · @ machines" style="font-size:14px;padding:10px 12px"
                    on:input=move |e| { q.set(event_target_value(&e)); sel.set(0) }
                    on:keydown=move |e| match e.key().as_str() {
                        "Escape" => on_close.run(()),
                        "ArrowDown" => { e.prevent_default(); sel.update(|s| *s = (*s + 1).min(hits.get_untracked().len().saturating_sub(1))) }
                        "ArrowUp" => { e.prevent_default(); sel.update(|s| *s = s.saturating_sub(1)) }
                        "Enter" => pick(sel.get_untracked()),
                        _ => {}
                    }/>
                <div style="max-height:55vh;overflow-y:auto">
                    {move || hits.get().into_iter().enumerate().map(|(i, h)| view! {
                        <button class=move || if sel.get() == i { "nav on" } else { "nav" } on:mouseenter=move |_| sel.set(i) on:click=move |_| pick(i)>
                            <Icon name=h.icon/><span class="grow truncate hi">{h.title.clone()}</span>
                            {(!h.sub.is_empty()).then(|| view! { <span class="small subdue truncate" style="max-width:45%">{h.sub.clone()}</span> })}
                        </button>
                    }).collect_view()}
                </div>
            </div>
        </div>
    }
}
