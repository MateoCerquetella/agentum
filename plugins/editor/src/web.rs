//! The editor's web part: open files as tabs, a textarea over the hub's highlighting, ⌘S.
//! Open files live in page-wide state, so switching side panel tabs keeps unsaved work.
use super::{esc, indent_unit, render, MAX_HIGHLIGHT, TAB};
use agentum_plugin::web::{RailTab, ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::WsOut;
use agentum_proto::machine::{FsReq, Req};
use agentum_proto::EventKind;
use agentum_ui::prelude::*;
use std::collections::HashMap;
use wasm_bindgen::JsCast;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

static TABS: [RailTab; 1] = [RailTab { id: TAB, title: "Editor", icon: "edit", view: tab }];

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn rail_tabs(&self) -> &'static [RailTab] {
        &TABS
    }
    fn css(&self) -> Option<&'static str> {
        Some(include_str!("../style.css"))
    }
}

#[derive(Clone, PartialEq)]
enum Load {
    Loading,
    Ready,
    Failed(String),
}

/// An open file. Its signals live as long as the page.
#[derive(Clone)]
struct Doc {
    path: String,
    machine: String,
    text: RwSignal<String>,
    /// As on disk (newlines as LF).
    saved: RwSignal<String>,
    sha: RwSignal<Option<String>>,
    /// (source line, its HTML) from the last highlighting.
    hl: RwSignal<Vec<(String, String)>>,
    lang: RwSignal<Option<String>>,
    crlf: RwSignal<bool>,
    load: RwSignal<Load>,
    /// Bumps when the text is replaced from disk (the textarea takes it).
    version: RwSignal<u32>,
}

impl Doc {
    fn new(machine: &str, path: &str) -> Doc {
        Doc {
            path: path.to_string(),
            machine: machine.to_string(),
            text: global(String::new()),
            saved: global(String::new()),
            sha: global(None),
            hl: global(vec![]),
            lang: global(None),
            crlf: global(false),
            load: global(Load::Loading),
            version: global(0),
        }
    }
    fn dirty(&self) -> bool {
        self.load.with(|l| *l == Load::Ready) && self.text.with(|t| self.saved.with(|s| t != s))
    }
}

#[derive(Clone, Default)]
struct Open {
    docs: Vec<Doc>,
    active: Option<String>,
}

thread_local! {
    /// Open files by thread.
    static OPEN: RwSignal<HashMap<String, Open>> = global(HashMap::new());
}
fn opened() -> RwSignal<HashMap<String, Open>> {
    OPEN.with(|o| *o)
}

fn pairs(content: &str, lines: &Value) -> Vec<(String, String)> {
    let Some(a) = lines.as_array() else { return vec![] };
    content.split('\n').zip(a.iter()).map(|(l, h)| (l.to_string(), h.as_str().unwrap_or("").to_string())).collect()
}

/// Read the file (again) from disk, with its colours.
fn load(d: &Doc, quiet: bool) {
    let d = d.clone();
    if !quiet {
        d.load.set(Load::Loading);
    }
    spawn_local(async move {
        match rpc::<Value>("editor", "open", &json!({"machine": d.machine, "path": d.path})).await {
            Ok(v) => {
                let raw = v["content"].as_str().unwrap_or("");
                let crlf = raw.contains("\r\n");
                let text = if crlf { raw.replace("\r\n", "\n") } else { raw.to_string() };
                let sha = v["sha"].as_str().map(str::to_string);
                if quiet && sha == d.sha.get_untracked() {
                    return;
                }
                d.hl.set(pairs(&text, &v["lines"]));
                d.lang.set(v["lang"].as_str().map(str::to_string));
                d.crlf.set(crlf);
                d.sha.set(sha);
                d.saved.set(text.clone());
                d.text.set(text);
                d.load.set(Load::Ready);
                d.version.update(|n| *n += 1);
            }
            Err(e) => {
                if !quiet {
                    d.load.set(Load::Failed(e.message));
                }
            }
        }
    });
}

fn persist(thread: &str) {
    let (paths, active) = opened().with_untracked(|o| o.get(thread).map(|t| (t.docs.iter().map(|d| d.path.clone()).collect::<Vec<_>>(), t.active.clone())).unwrap_or_default());
    agentum_ui::store::set(&format!("agentum.editor.{thread}"), &json!({"paths": paths, "active": active}).to_string());
}

fn add(thread: &str, machine: &str, path: &str) {
    let mut fresh = None;
    opened().update(|o| {
        let t = o.entry(thread.to_string()).or_default();
        if !t.docs.iter().any(|d| d.path == path) {
            let d = Doc::new(machine, path);
            fresh = Some(d.clone());
            t.docs.push(d);
        }
        t.active = Some(path.to_string());
    });
    if let Some(d) = fresh {
        load(&d, false);
    }
    persist(thread);
}

/// Open `path` (absolute, on the thread's machine) in the thread's Editor tab, and show it.
pub fn open(cx: &ThreadCx, path: &str) {
    let Some(machine) = cx.summary.with_untracked(|s| s.as_ref().map(|s| s.machine.clone())) else { return };
    add(&cx.id, &machine, path);
    agentum_ui::store::set("agentum.rail", TAB);
    cx.rail.set(Some(TAB.to_string()));
}

/// The file shown in a thread's editor, live.
pub fn active(thread: &str) -> Option<String> {
    opened().with(|o| o.get(thread).and_then(|t| t.active.clone()))
}

fn tab(cx: ThreadCx) -> AnyView {
    view! { <EditorTab cx=cx/> }.into_any()
}

fn base(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

#[component]
fn EditorTab(cx: ThreadCx) -> impl IntoView {
    let id = cx.id.clone();
    // After a reload of the page: reopen what was open.
    {
        let id = id.clone();
        let summary = cx.summary;
        Effect::new(move |done: Option<bool>| {
            if done == Some(true) || opened().with_untracked(|o| o.contains_key(&id)) {
                return true;
            }
            let Some(machine) = summary.with(|s| s.as_ref().map(|s| s.machine.clone())) else { return false };
            let saved: Value = agentum_ui::store::get(&format!("agentum.editor.{id}")).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
            let paths: Vec<String> = saved["paths"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_string)).collect();
            for p in &paths {
                add(&id, &machine, p);
            }
            if let Some(a) = saved["active"].as_str().filter(|a| paths.iter().any(|p| p == a)) {
                opened().update(|o| {
                    if let Some(t) = o.get_mut(&id) {
                        t.active = Some(a.to_string());
                    }
                });
            }
            true
        });
    }
    // After a turn the agent may have changed open files: reread the ones without edits.
    {
        let id = id.clone();
        on_ws(move |m| {
            let WsOut::Event { thread, event } = m else { return };
            if *thread != id || !matches!(event.event.kind, EventKind::TurnDone { .. }) {
                return;
            }
            let docs = opened().with_untracked(|o| o.get(&id).map(|t| t.docs.clone()).unwrap_or_default());
            for d in docs.iter().filter(|d| !d.dirty()) {
                load(d, true);
            }
        });
    }
    let docs = {
        let id = id.clone();
        move || opened().with(|o| o.get(&id).map(|t| t.docs.clone()).unwrap_or_default())
    };
    let active_path = {
        let id = id.clone();
        Memo::new(move |_| opened().with(|o| o.get(&id).and_then(|t| t.active.clone())))
    };
    let cwd = Memo::new(move |_| cx.summary.with(|s| s.as_ref().map(|s| s.cwd.clone()).unwrap_or_default()));
    let close = {
        let id = id.clone();
        Callback::new(move |path: String| {
            let id = id.clone();
            spawn_local(async move {
                let d = opened().with_untracked(|o| o.get(&id).and_then(|t| t.docs.iter().find(|d| d.path == path).cloned()));
                if d.as_ref().is_some_and(|d| d.dirty()) && !confirm("Close without saving?", &format!("{} has changes that aren't saved.", base(&path)), "Close", true).await {
                    return;
                }
                opened().update(|o| {
                    if let Some(t) = o.get_mut(&id) {
                        let i = t.docs.iter().position(|d| d.path == path);
                        t.docs.retain(|d| d.path != path);
                        if t.active.as_deref() == Some(path.as_str()) {
                            t.active = i.and_then(|i| t.docs.get(i.min(t.docs.len().saturating_sub(1)))).map(|d| d.path.clone());
                        }
                    }
                });
                persist(&id);
            });
        })
    };
    let activate = {
        let id = id.clone();
        Callback::new(move |path: String| {
            opened().update(|o| {
                if let Some(t) = o.get_mut(&id) {
                    t.active = Some(path);
                }
            });
            persist(&id);
        })
    };
    let rail = cx.rail;
    view! {
        <div class="ed">
            <Show when={ let docs = docs.clone(); move || !docs().is_empty() }>
                <div class="ed-tabs">
                    {
                        let docs = docs.clone();
                        move || docs().into_iter().map(|d| {
                            let p = d.path.clone();
                            let p2 = p.clone();
                            let p3 = p.clone();
                            let d2 = d.clone();
                            view! {
                                <div class="ed-tab" class:on=move || active_path.get().as_deref() == Some(p.as_str()) title=p.clone() on:click=move |_| activate.run(p2.clone())>
                                    <span class="truncate">{base(&d.path).to_string()}</span>
                                    <span class="ed-dot" class:dirty=move || d2.dirty()></span>
                                    <button class="ed-x" title="Close" on:click=move |e| { e.stop_propagation(); close.run(p3.clone()) }><Icon name="x" size=11/></button>
                                </div>
                            }
                        }).collect_view()
                    }
                </div>
            </Show>
            {move || {
                let path = active_path.get();
                let doc = path.and_then(|p| docs().into_iter().find(|d| d.path == p));
                match doc {
                    Some(d) => view! { <Editor doc=d cwd=cwd/> }.into_any(),
                    None => view! {
                        <div class="empty col" style="align-items:center">
                            <span>"No file open."</span>
                            <button class="btn ed-sm" on:click=move |_| { agentum_ui::store::set("agentum.rail", "files"); rail.set(Some("files".into())) }>
                                <Icon name="folder" size=13/>"Browse files"
                            </button>
                        </div>
                    }.into_any(),
                }
            }}
        </div>
    }
}

fn html_document() -> Option<web_sys::HtmlDocument> {
    web_sys::window()?.document()?.dyn_into::<web_sys::HtmlDocument>().ok()
}

/// Type `s` at the caret (through the browser, so ⌘Z undoes it).
fn insert(ta: &web_sys::HtmlTextAreaElement, s: &str) {
    let ok = html_document().and_then(|d| d.exec_command_with_show_ui_and_value("insertText", false, s).ok()).unwrap_or(false);
    if !ok {
        let start = ta.selection_start().ok().flatten().unwrap_or(0);
        let end = ta.selection_end().ok().flatten().unwrap_or(start);
        let _ = ta.set_range_text_with_start_and_end_and_selection_mode(s, start, end, web_sys::SelectionMode::End);
        let _ = ta.dispatch_event(&web_sys::Event::new("input").unwrap());
    }
}

async fn write(d: &Doc, force: bool) -> Result<Value, ApiError> {
    let text = d.text.get_untracked();
    let content = if d.crlf.get_untracked() { text.replace('\n', "\r\n") } else { text };
    let expect_sha = if force { None } else { d.sha.get_untracked() };
    let req = Req::Fs(FsReq::Write { path: d.path.clone(), content, expect_sha });
    api_post::<Value>("/code/call", &json!({"machine": d.machine, "req": req})).await
}

fn save(d: Doc) {
    spawn_local(async move {
        let text = d.text.get_untracked();
        let mut r = write(&d, false).await;
        if let Err(e) = &r {
            if e.message.contains("conflict") {
                if !confirm("Changed on disk", &format!("{} was changed since you opened it (by the agent?). Save yours over it?", base(&d.path)), "Overwrite", true).await {
                    return;
                }
                r = write(&d, true).await;
            }
        }
        match r {
            Ok(v) => {
                d.sha.set(v["sha"].as_str().map(str::to_string));
                d.saved.set(text);
                toast(format!("Saved {}.", base(&d.path)));
            }
            Err(e) => toast_bad(e.message),
        }
    });
}

#[component]
fn Editor(doc: Doc, cwd: Memo<String>) -> impl IntoView {
    let ta = NodeRef::<leptos::html::Textarea>::new();
    let d = doc.clone();
    // The textarea takes the text when it mounts and whenever it is reread from disk.
    Effect::new(move |_| {
        d.version.track();
        if let Some(el) = ta.get() {
            let t = d.text.get_untracked();
            if el.value() != t {
                el.set_value(&t);
            }
        }
    });
    // Re-highlight a moment after typing stops.
    let timer = StoredValue::new_local(None::<gloo_timers::callback::Timeout>);
    let rehighlight = {
        let d = doc.clone();
        move || {
            let d = d.clone();
            timer.set_value(Some(gloo_timers::callback::Timeout::new(350, move || {
                let content = d.text.get_untracked();
                if content.len() > MAX_HIGHLIGHT || d.lang.get_untracked().is_none() {
                    return;
                }
                spawn_local(async move {
                    if let Ok(v) = rpc::<Value>("editor", "highlight", &json!({"path": d.path, "content": content})).await {
                        if d.text.with_untracked(|t| *t == content) {
                            d.hl.set(pairs(&content, &v["lines"]));
                        }
                    }
                });
            })));
        }
    };
    let on_input = {
        let d = doc.clone();
        let rh = rehighlight.clone();
        move |_| {
            if let Some(el) = ta.get() {
                d.text.set(el.value());
                rh();
            }
        }
    };
    let on_key = {
        let d = doc.clone();
        move |e: web_sys::KeyboardEvent| {
            let Some(el) = ta.get() else { return };
            let k = e.key();
            if (e.meta_key() || e.ctrl_key()) && k.eq_ignore_ascii_case("s") {
                e.prevent_default();
                e.stop_propagation();
                save(d.clone());
                return;
            }
            if e.meta_key() || e.ctrl_key() || e.alt_key() {
                return;
            }
            match k.as_str() {
                "Tab" if !e.shift_key() => {
                    e.prevent_default();
                    let unit = d.text.with_untracked(|t| indent_unit(t));
                    insert(&el, &unit);
                }
                "Enter" if !e.shift_key() => {
                    // Keep the line's indentation.
                    let v = el.value();
                    let at = el.selection_start().ok().flatten().unwrap_or(0) as usize;
                    // selectionStart counts UTF-16 units.
                    let before: String = char::decode_utf16(v.encode_utf16().take(at)).map(|c| c.unwrap_or('\u{fffd}')).collect();
                    let line = before.rsplit('\n').next().unwrap_or("");
                    let ws: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                    if !ws.is_empty() {
                        e.prevent_default();
                        insert(&el, &format!("\n{ws}"));
                    }
                }
                _ => {}
            }
        }
    };
    let d = doc.clone();
    let html = move || {
        if d.text.with(|t| t.len() > MAX_HIGHLIGHT) {
            return d.text.with(|t| format!("{}\n", esc(t)));
        }
        d.text.with(|t| d.hl.with(|h| render(t, h)))
    };
    let d = doc.clone();
    let numbers = Memo::new(move |_| d.text.with(|t| t.split('\n').count()));
    let gutter = move || {
        let n = numbers.get();
        let mut s = String::with_capacity(n * 5);
        for i in 1..=n {
            s.push_str(&i.to_string());
            s.push('\n');
        }
        s
    };
    let d = doc.clone();
    let rel = move || {
        let c = cwd.get();
        let c = c.trim_end_matches('/');
        d.path.strip_prefix(c).map(|r| r.trim_start_matches('/').to_string()).filter(|r| !r.is_empty() && !c.is_empty()).unwrap_or_else(|| d.path.clone())
    };
    let (d1, d2, d3, d4, d5) = (doc.clone(), doc.clone(), doc.clone(), doc.clone(), doc.clone());
    let reload = move |_| {
        let d = d4.clone();
        spawn_local(async move {
            if d.dirty() && !confirm("Reload from disk?", "Your changes that aren't saved will be lost.", "Reload", true).await {
                return;
            }
            load(&d, false);
        });
    };
    view! {
        <div class="ed-doc">
            <div class="ed-bar">
                <span class="ed-path truncate" title=doc.path.clone()>{rel}</span>
                <span class="small subdue">{move || d1.lang.get().unwrap_or_default()}</span>
                <span class="grow"></span>
                <Show when=move || d2.dirty()><span class="small ed-unsaved">"Unsaved"</span></Show>
                <button class="icon-btn" title="Reload from disk" on:click=reload><Icon name="refresh" size=14/></button>
                <button class="btn ed-sm" title="Save (⌘S)" disabled=move || !d3.dirty() on:click=move |_| save(d5.clone())>"Save"</button>
            </div>
            {
                let d = doc.clone();
                move || match d.load.get() {
                    Load::Loading => Some(view! { <div class="empty small">"Opening…"</div> }.into_any()),
                    Load::Failed(e) => Some(view! { <div class="ed-err small">{e}</div> }.into_any()),
                    Load::Ready => None,
                }
            }
            <div class="ed-scroll" style:display=move || if doc.load.get() == Load::Ready { "" } else { "none" }>
                <div class="ed-wrap">
                    <div class="ed-gutter" aria-hidden="true">{gutter}</div>
                    <div class="ed-code">
                        <div class="ed-hl md" aria-hidden="true" inner_html=html></div>
                        <textarea class="ed-ta" node_ref=ta spellcheck="false" autocapitalize="off" autocomplete="off" wrap="off"
                            on:input=on_input on:keydown=on_key></textarea>
                    </div>
                </div>
            </div>
        </div>
    }
}
