//! Your floatnote notes (`/api/floatnotes`): the library as cards, and the editor (a textarea with
//! a Markdown preview), saved half a second after you stop typing. A note changed on another
//! device meanwhile (409) asks which version to keep.
use super::assist::{Suggest, Tidy};
use crate::pages::chat::md;
use agentum_ui::prelude::*;
use wasm_bindgen::JsCast;

/// `- [ ] ` / `1. [x] `: (the part before the mark, the mark).
fn task_mark(line: &str) -> Option<(usize, char)> {
    let t = line.trim_start();
    let lead = line.len() - t.len();
    let after = if let Some(r) = t.strip_prefix(['-', '*', '+', '•']) {
        r
    } else {
        let d = t.chars().take_while(|c| c.is_ascii_digit()).count();
        if d == 0 {
            return None;
        }
        t[d..].strip_prefix(['.', ')'])?
    };
    let sp = after.len() - after.trim_start().len();
    if sp == 0 {
        return None;
    }
    let rest = &after[sp..];
    let mut cs = rest.chars();
    if cs.next() != Some('[') {
        return None;
    }
    let mark = cs.next()?;
    if !matches!(mark, ' ' | 'x' | 'X') || cs.next() != Some(']') || !cs.next().is_some_and(char::is_whitespace) {
        return None;
    }
    Some((lead + (t.len() - rest.len()) + 1, mark))
}

/// Without Markdown marks, for titles and previews.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '\\' {
            if let Some(n) = rest[1..].chars().next() {
                out.push(n);
                rest = &rest[1 + n.len_utf8()..];
                continue;
            }
        }
        if rest.starts_with("**") || rest.starts_with("__") || rest.starts_with("~~") {
            rest = &rest[2..];
            continue;
        }
        if c == '`' {
            rest = &rest[1..];
            continue;
        }
        if c == '[' {
            if let Some(close) = rest.find("](") {
                if let Some(end) = rest[close..].find(')') {
                    out.push_str(&rest[1..close]);
                    rest = &rest[close + end + 1..];
                    continue;
                }
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn strip_lead(l: &str) -> &str {
    let t = l.trim_start();
    for p in ["###### ", "##### ", "#### ", "### ", "## ", "# ", "> ", "- ", "* ", "+ "] {
        if let Some(r) = t.strip_prefix(p) {
            return r;
        }
    }
    let d = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if d > 0 {
        if let Some(r) = t[d..].strip_prefix(". ").or_else(|| t[d..].strip_prefix(") ")) {
            return r;
        }
    }
    t
}

fn when(mtime: f64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(mtime * 1000.0));
    let now = js_sys::Date::new_0();
    if d.to_date_string() == now.to_date_string() {
        return format!("Today, {:02}:{:02}", d.get_hours(), d.get_minutes());
    }
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let y = if d.get_full_year() != now.get_full_year() { format!(" {}", d.get_full_year()) } else { String::new() };
    format!("{} {}{y}", d.get_date(), M[d.get_month() as usize % 12])
}

fn card(n: &Value, query: &str, latest: bool, open: Callback<String>) -> AnyView {
    let body = n["body"].as_str().unwrap_or("");
    let lines: Vec<&str> = body.trim().lines().skip(1).collect();
    let shown: Vec<&str> = lines.iter().copied().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("---") && !l.trim_start().starts_with("```")).collect();
    let tasks: Vec<char> = lines.iter().filter_map(|l| task_mark(l).map(|t| t.1)).collect();
    let at = if query.is_empty() { 0 } else { shown.iter().position(|l| l.to_lowercase().contains(query)).unwrap_or(0) };
    let preview: Vec<&str> = shown.iter().skip(at).take(if latest { 5 } else { 3 }).copied().collect();
    let id = n["id"].as_str().unwrap_or("").to_string();
    let title = plain(n["title"].as_str().unwrap_or(""));
    view! {
        <button type="button" class=if latest { "nf-card nf-featured" } else { "nf-card" } on:click=move |_| open.run(id.clone())>
            {latest.then(|| view! { <span class="nf-recent"><Icon name="clock" size=13/>" Last edited"</span> })}
            <span class="nf-title">{if title.is_empty() { "Untitled note".to_string() } else { title }}</span>
            <span class="nf-body">{if preview.is_empty() {
                view! { <span class="nf-line muted">"Title only"</span> }.into_any()
            } else {
                preview.into_iter().map(|l| {
                    let task = task_mark(l);
                    let text = match task { Some((at, _)) => plain(l[at + 1..].trim_start()), None => plain(strip_lead(l)) };
                    let done = task.is_some_and(|t| t.1 != ' ');
                    view! { <span class=if done { "nf-line done" } else { "nf-line" }>{task.map(|t| view! { <Icon name=if t.1 == ' ' { "dot" } else { "check" } size=13/> })}{text}</span> }
                }).collect_view().into_any()
            }}</span>
            <span class="nf-meta"><span>{when(n["mtime"].as_f64().unwrap_or(0.0))}</span>
                {(!tasks.is_empty()).then(|| view! { <span><Icon name="check" size=13/>{format!(" {}/{} tasks", tasks.iter().filter(|t| **t != ' ').count(), tasks.len())}</span> })}</span>
        </button>
    }
    .into_any()
}

#[component]
pub fn Files(q: Signal<String>) -> impl IntoView {
    let notes = RwSignal::new(None::<Vec<Value>>);
    let err = RwSignal::new(String::new());
    let opening = RwSignal::new(false);
    let open = RwSignal::new(None::<Value>);
    let load = move || {
        spawn_local(async move {
            match api_get::<Vec<Value>>("/floatnotes").await {
                Ok(n) => {
                    notes.set(Some(n));
                    err.set(String::new());
                }
                Err(e) => err.set(e.message),
            }
        })
    };
    load();
    subscribe(&["app"]);
    on_ws(move |m| {
        if let agentum_proto::api::WsOut::App { payload } = m {
            if payload["kind"] == "notes" {
                load();
            }
        }
    });
    // Syncthing can change files between the hub's own events.
    let _poll = StoredValue::new_local(gloo_timers::callback::Interval::new(15_000, move || {
        if web_sys::window().and_then(|w| w.document()).is_some_and(|d| !d.hidden()) {
            load();
        }
    }));
    let open_note = move |id: Option<String>| {
        opening.set(true);
        spawn_local(async move {
            let r = match id {
                Some(id) => api_get::<Value>(&format!("/floatnotes/{}", js_sys::encode_uri_component(&id))).await,
                None => api_post::<Value>("/floatnotes", &json!({"md": ""})).await,
            };
            match r {
                Ok(n) => open.set(Some(n)),
                Err(e) => toast_bad(e.message),
            }
            opening.set(false);
        });
    };
    let link = use_param("note");
    Effect::new(move |_| {
        if let Some(id) = link.get().filter(|l| !l.is_empty()) {
            set_param("note", None);
            open_note(Some(id));
        }
    });
    let pick = Callback::new(move |id: String| {
        if !opening.get_untracked() {
            open_note(Some(id))
        }
    });
    move || {
        if let Some(n) = open.get() {
            let id = n["id"].as_str().unwrap_or("").to_string();
            let latest = Signal::derive(move || notes.with(|v| v.as_ref().and_then(|v| v.iter().find(|x| x["id"] == id.as_str()).and_then(|x| x["mtime"].as_f64()))));
            return view! { <Editor note=n latest=latest on_close=Callback::new(move |_| { open.set(None); load() })/> }.into_any();
        }
        let query = q.get().trim().to_lowercase();
        view! {
            <section class="nf-library" aria-label="Synced Markdown notes" aria-busy=move || opening.get().to_string()>
                <header class="nf-library-head">
                    <div><h1>"Notebook"</h1><p><Icon name="refresh" size=13/>" Floatnote · Syncthing"</p></div>
                    <button type="button" class="btn primary" disabled=move || opening.get() on:click=move |_| open_note(None)><Icon name="plus" size=16/>" New note"</button>
                </header>
                <Show when=move || !err.get().is_empty()>
                    <div class="nf-library-state" role="alert"><span class="err">"Couldn’t load notes. "</span><button class="btn" on:click=move |_| load()>"Retry"</button></div>
                </Show>
                {move || match notes.get() {
                    None => err.get().is_empty().then(|| view! { <div class="nf-library-state" role="status">"Loading notes…"</div> }).into_any(),
                    Some(all) => {
                        let shown: Vec<Value> = all.into_iter().filter(|n| query.is_empty() || format!("{}\n{}", n["title"].as_str().unwrap_or(""), n["body"].as_str().unwrap_or("")).to_lowercase().contains(&query)).collect();
                        let count = shown.len();
                        view! {
                            <div class="nf-list-head"><h2>{if query.is_empty() { "Recent notes" } else { "Search results" }}</h2><span>{format!("{count} {}", if count == 1 { "note" } else { "notes" })}</span></div>
                            {if shown.is_empty() {
                                view! { <div class="nf-library-state">{if query.is_empty() { "Start a note here, or add one in floatnote and it will appear after sync." } else { "No notes match your search." }}</div> }.into_any()
                            } else {
                                view! { <div class="nf-grid">{shown.iter().enumerate().map(|(i, n)| card(n, &query, query.is_empty() && i == 0, pick)).collect_view()}</div> }.into_any()
                            }}
                        }.into_any()
                    }
                }}
            </section>
        }.into_any()
    }
}

#[component]
fn Editor(note: Value, latest: Signal<Option<f64>>, on_close: Callback<()>) -> impl IntoView {
    let id = note["id"].as_str().unwrap_or("").to_string();
    let url = format!("/floatnotes/{}", js_sys::encode_uri_component(&id));
    let first = note["md"].as_str().unwrap_or("").to_string();
    let md = RwSignal::new(first.clone());
    let base = StoredValue::new((first.clone(), note["mtime"].as_f64().unwrap_or(0.0)));
    let state = RwSignal::new("Saved".to_string());
    let preview = RwSignal::new(false);
    let ai = RwSignal::new(None::<&'static str>);
    let writing = StoredValue::new(false);
    let timer = StoredValue::new_local(None::<gloo_timers::callback::Timeout>);
    let area = NodeRef::<leptos::html::Textarea>::new();
    let url = StoredValue::new(url);

    /// Write the draft (once any write in flight is done). Err: why it failed.
    async fn save(md: RwSignal<String>, base: StoredValue<(String, f64)>, state: RwSignal<String>, writing: StoredValue<bool>, url: StoredValue<String>) -> Result<(), String> {
        while writing.get_value() {
            gloo_timers::future::TimeoutFuture::new(50).await;
        }
        let text = md.get_untracked();
        if text == base.get_value().0 {
            state.set("Saved".into());
            return Ok(());
        }
        state.set("Saving…".into());
        writing.set_value(true);
        let u = url.get_value();
        let put = |mtime: f64| {
            let (u, text) = (u.clone(), text.clone());
            async move { api_put::<Value>(&u, &json!({"md": text, "mtime": mtime})).await }
        };
        let res: Result<(), String> = async {
            match put(base.get_value().1).await {
                Ok(r) => base.set_value((text.clone(), r["mtime"].as_f64().unwrap_or(0.0))),
                Err(e) if e.status == 409 => {
                    let theirs = api_get::<Value>(&u).await.map_err(|e| e.message)?;
                    let keep = confirm("This note changed on another device", "Keep your version, or Cancel to use the synced one.", "Keep mine", false).await;
                    let (tmd, tm) = (theirs["md"].as_str().unwrap_or("").to_string(), theirs["mtime"].as_f64().unwrap_or(0.0));
                    if keep {
                        let r = put(tm).await.map_err(|e| e.message)?;
                        base.set_value((text.clone(), r["mtime"].as_f64().unwrap_or(0.0)));
                    } else {
                        base.set_value((tmd.clone(), tm));
                        if md.get_untracked() == text {
                            md.set(tmd);
                        }
                    }
                }
                Err(e) => return Err(e.message),
            }
            Ok(())
        }
        .await;
        writing.set_value(false);
        match res {
            Ok(()) => {
                state.set(if md.get_untracked() == base.get_value().0 { "Saved" } else { "Unsaved changes" }.into());
                Ok(())
            }
            Err(e) => {
                state.set("Save failed".into());
                Err(e)
            }
        }
    }
    let flush = move || save(md, base, state, writing, url);
    let change = move |text: String| {
        md.set(text);
        state.set("Unsaved changes".into());
        timer.set_value(Some(gloo_timers::callback::Timeout::new(500, move || {
            spawn_local(async move {
                let _ = flush().await;
            })
        })));
    };
    let close = move || {
        timer.set_value(None);
        spawn_local(async move {
            for _ in 0..3 {
                if let Err(e) = flush().await {
                    toast_bad(e);
                    return;
                }
                if md.get_untracked() == base.get_value().0 {
                    break;
                }
            }
            if md.get_untracked().trim().is_empty() {
                let _ = api_delete::<Value>(&format!("{}?mtime={}", url.get_value(), base.get_value().1)).await;
            }
            on_close.run(());
        });
    };
    let remove = move |_| {
        spawn_local(async move {
            if !confirm("Delete this note?", "It goes to Recently Deleted in floatnote and is kept for 30 days.", "Delete", true).await {
                return;
            }
            timer.set_value(None);
            while writing.get_value() {
                gloo_timers::future::TimeoutFuture::new(50).await;
            }
            match api_delete::<Value>(&format!("{}?mtime={}", url.get_value(), base.get_value().1)).await {
                Ok(_) => on_close.run(()),
                Err(e) => toast_bad(e.message),
            }
        });
    };
    // Changed by sync while open and not edited here: take the new version.
    Effect::new(move |_| {
        let Some(m) = latest.get() else { return };
        let (bmd, bm) = base.get_value();
        if (m - bm).abs() < 0.000001 || writing.get_value() || md.get_untracked() != bmd {
            return;
        }
        spawn_local(async move {
            if let Ok(n) = api_get::<Value>(&url.get_value()).await {
                if writing.get_value() || md.get_untracked() != base.get_value().0 {
                    return;
                }
                let t = n["md"].as_str().unwrap_or("").to_string();
                base.set_value((t.clone(), n["mtime"].as_f64().unwrap_or(0.0)));
                md.set(t);
                state.set("Updated from sync".into());
            }
        });
    });
    let toggle = Callback::new(move |line: usize| {
        let text = md.get_untracked();
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        if let Some((at, mark)) = lines.get(line).and_then(|l| task_mark(l)) {
            lines[line].replace_range(at..at + 1, if mark == ' ' { "x" } else { " " });
            change(lines.join("\n"));
        }
    });
    let format = move |prefix: &'static str, suffix: &'static str| {
        let Some(t) = area.get_untracked() else { return };
        let text = md.get_untracked();
        // Selection offsets are UTF-16; work in chars.
        let chars: Vec<char> = text.chars().collect();
        let u16_to_char = |u: u32| {
            let mut n = 0u32;
            for (i, c) in chars.iter().enumerate() {
                if n >= u {
                    return i;
                }
                n += c.len_utf16() as u32;
            }
            chars.len()
        };
        let (caret, end) = (u16_to_char(t.selection_start().ok().flatten().unwrap_or(0)), u16_to_char(t.selection_end().ok().flatten().unwrap_or(0)));
        let start = if !suffix.is_empty() || caret == 0 { caret } else { chars[..caret].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0) };
        let s: String = chars[..start].iter().collect();
        let sel: String = chars[start..end].iter().collect();
        let rest: String = chars[end..].iter().collect();
        change(format!("{s}{prefix}{sel}{suffix}{rest}"));
        let p = prefix.encode_utf16().count() as u32;
        let (c16, e16) = (t.selection_start().ok().flatten().unwrap_or(0), t.selection_end().ok().flatten().unwrap_or(0));
        request_animation_frame(move || {
            let _ = t.focus();
            let _ = t.set_selection_range(c16 + p, e16 + p);
        });
    };
    // Your model reads the note as saved: save first.
    let pick_ai = move |which: &'static str| {
        if ai.get_untracked() == Some(which) {
            ai.set(None);
            return;
        }
        spawn_local(async move {
            match flush().await {
                Ok(()) => ai.set(Some(which)),
                Err(e) => toast_bad(e),
            }
        });
    };
    let title = move || {
        let t = md.with(|m| m.trim_start().lines().next().map(|l| plain(l.trim_start_matches('#').trim())).unwrap_or_default());
        if t.is_empty() { "Untitled note".to_string() } else { t }
    };
    view! {
        <section class="nf-editor" on:keydown=move |e| if e.key() == "Escape" { e.prevent_default(); close() }>
            <header class="nf-bar">
                <button class="btn ghost" on:click=move |_| close()><Icon name="chevron-left" size=15/>" All notes"</button>
                <span class="grow"/>
                <span class=move || if state.get() == "Save failed" { "nf-save err" } else { "nf-save" } role="status">
                    {move || (state.get() == "Saved").then(|| view! { <Icon name="check" size=14/> })}{move || state.get()}
                </span>
                <Show when=move || state.get() == "Save failed">
                    <button class="btn ghost" on:click=move |_| spawn_local(async move { if let Err(e) = flush().await { toast_bad(e) } })>"Retry"</button>
                </Show>
                <button class="icon-btn" title="Delete note" aria-label="Delete note" on:click=remove><Icon name="trash"/></button>
            </header>
            <Show when=move || !preview.get()><h1 class="nf-edit-title">{title}</h1></Show>
            <div class="nf-editor-actions">
                <div class="notes-tabs" role="group" aria-label="Note view">
                    <button type="button" aria-pressed=move || (!preview.get()).to_string() on:click=move |_| preview.set(false)>"Write"</button>
                    <button type="button" aria-pressed=move || preview.get().to_string() on:click=move |_| preview.set(true)>"Preview"</button>
                </div>
                <div class="nf-model-actions">
                    <button class="btn ghost" aria-pressed=move || (ai.get() == Some("suggest")).to_string() disabled=move || md.with(|m| m.trim().is_empty()) title="Find to-dos and events in this note"
                        on:click=move |_| pick_ai("suggest")><Icon name="sparkle" size=13/>" To-dos & events"</button>
                    <button class="btn ghost" aria-pressed=move || (ai.get() == Some("tidy")).to_string() disabled=move || md.with(|m| m.trim().is_empty()) title="Preview a tidier version of this note"
                        on:click=move |_| pick_ai("tidy")><Icon name="sparkle" size=13/>" Tidy"</button>
                </div>
            </div>
            {move || match ai.get() {
                Some("suggest") => view! { <Suggest url=url.get_value() close=Callback::new(move |_| ai.set(None))/> }.into_any(),
                Some(_) => view! { <Tidy url=url.get_value() md=md.get_untracked() close=Callback::new(move |_| ai.set(None))
                    accept=Callback::new(move |t: String| { ai.set(None); md.set(t); spawn_local(async move { let _ = flush().await; }) })/> }.into_any(),
                None => ().into_any(),
            }}
            <div class="nf-sheet">
                {move || if preview.get() {
                    view! { <div class="nf-preview">{move || if md.with(|m| m.trim().is_empty()) { view! { <p class="muted">"Write something to preview it here."</p> }.into_any() } else { md::markdown(&md.get(), false, Some(toggle)) }}</div> }.into_any()
                } else {
                    view! {
                        <div class="nf-format" role="group" aria-label="Markdown formatting">
                            <button type="button" title="Heading" on:click=move |_| format("## ", "")>"Heading"</button>
                            <button type="button" title="Bold" on:click=move |_| format("**", "**")><strong>"Bold"</strong></button>
                            <button type="button" title="Bullet" on:click=move |_| format("- ", "")>"Bullet"</button>
                            <button type="button" title="Checklist item" on:click=move |_| format("- [ ] ", "")><Icon name="check" size=14/>" Checklist"</button>
                        </div>
                        <textarea node_ref=area class="nf-text" aria-label="Note Markdown" placeholder="Title\n\nStart writing…" spellcheck="true" autofocus=first.is_empty()
                            prop:value=move || md.get()
                            on:input=move |e| change(event_target_value(&e))
                            on:keydown=move |e| {
                                if e.key() == "Enter" && (e.ctrl_key() || e.meta_key()) {
                                    e.prevent_default();
                                    let Some(t) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok()) else { return };
                                    let caret = t.selection_start().ok().flatten().unwrap_or(0);
                                    let before: String = String::from_utf16_lossy(&md.get_untracked().encode_utf16().take(caret as usize).collect::<Vec<u16>>());
                                    toggle.run(before.matches('\n').count());
                                    request_animation_frame(move || { let _ = t.set_selection_range(caret, caret); });
                                }
                            }/>
                    }.into_any()
                }}
            </div>
        </section>
    }
}
