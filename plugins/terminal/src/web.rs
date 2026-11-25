//! The terminal's web part: shells on the thread's machine, in tabs. agentd streams the PTY's
//! raw bytes on `term:<machine>:<term>`; a vt100 parser here keeps the screen, drawn as rows of
//! spans (only the rows that changed, once per frame). Keys and pastes go back as TermInput,
//! the panel's size as TermResize; a terminal opened again repaints from agentd's scrollback.
use super::vt::{key_bytes, paste_bytes, row_html};
use agentum_plugin::web::{RailTab, ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::{WsIn, WsOut};
use agentum_proto::machine::{Req, TermReq};
use agentum_ui::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroU16;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

static TABS: [RailTab; 1] = [RailTab { id: "terminal", title: "Terminal", icon: "terminal", view: tab }];

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

const SCROLLBACK: usize = 5000;

thread_local! {
    /// Each thread's terminals (ids) and the one shown.
    static TERMS: RwSignal<HashMap<String, (Vec<String>, Option<String>)>> = global(HashMap::new());
}
fn terms() -> RwSignal<HashMap<String, (Vec<String>, Option<String>)>> {
    TERMS.with(|t| *t)
}

async fn term_call(machine: &str, req: TermReq) -> Result<Value, ApiError> {
    api_post::<Value>("/code/call", &json!({"machine": machine, "req": Req::Term(req)})).await
}

fn b64_encode(b: &[u8]) -> String {
    let s: String = b.iter().map(|&c| c as char).collect();
    web_sys::window().and_then(|w| w.btoa(&s).ok()).unwrap_or_default()
}
fn b64_decode(s: &str) -> Vec<u8> {
    web_sys::window().and_then(|w| w.atob(s).ok()).map(|s| s.chars().map(|c| c as u8).collect()).unwrap_or_default()
}

fn tab(cx: ThreadCx) -> AnyView {
    view! { <TerminalTab cx=cx/> }.into_any()
}

/// The terminal ids of a thread: "<thread>.<n>".
fn next_id(thread: &str, have: &[String]) -> String {
    let n = have.iter().filter_map(|t| t.strip_prefix(&format!("{thread}.")).and_then(|n| n.parse::<u32>().ok())).max().unwrap_or(0) + 1;
    format!("{thread}.{n}")
}

#[component]
fn TerminalTab(cx: ThreadCx) -> impl IntoView {
    let summary = cx.summary;
    let place = Memo::new(move |_| summary.with(|s| s.as_ref().map(|s| (s.machine.clone(), s.cwd.clone()))));
    let id = cx.id.clone();
    let err = RwSignal::new(None::<String>);
    // The machine's open terminals of this thread, else a first one.
    {
        let id = id.clone();
        Effect::new(move |done: Option<bool>| {
            if done == Some(true) {
                return true;
            }
            let Some((m, _)) = place.get() else { return false };
            let id = id.clone();
            spawn_local(async move {
                let live: Vec<String> = match term_call(&m, TermReq::List).await {
                    Ok(v) => v.as_array().into_iter().flatten().filter_map(|t| t["term"].as_str().map(str::to_string)).filter(|t| t.starts_with(&format!("{id}."))).collect(),
                    Err(e) => {
                        err.set(Some(e.message));
                        return;
                    }
                };
                terms().update(|all| {
                    let e = all.entry(id.clone()).or_default();
                    for t in live {
                        if !e.0.contains(&t) {
                            e.0.push(t);
                        }
                    }
                    e.0.sort_by_key(|t| t.rsplit('.').next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0));
                    if e.0.is_empty() {
                        let t = next_id(&id, &[]);
                        e.0.push(t);
                    }
                    if e.1.as_ref().is_none_or(|a| !e.0.contains(a)) {
                        e.1 = e.0.first().cloned();
                    }
                });
            });
            true
        });
    }
    let list = {
        let id = id.clone();
        Memo::new(move |_| terms().with(|t| t.get(&id).cloned().unwrap_or_default()))
    };
    let add = {
        let id = id.clone();
        move |_| {
            terms().update(|all| {
                let e = all.entry(id.clone()).or_default();
                let t = next_id(&id, &e.0);
                e.0.push(t.clone());
                e.1 = Some(t);
            })
        }
    };
    let pick = {
        let id = id.clone();
        Callback::new(move |t: String| {
            terms().update(|all| {
                if let Some(e) = all.get_mut(&id) {
                    e.1 = Some(t);
                }
            })
        })
    };
    let close = {
        let id = id.clone();
        Callback::new(move |t: String| {
            let Some((m, _)) = place.get_untracked() else { return };
            let id = id.clone();
            spawn_local(async move {
                if !confirm("Close this terminal?", "Its shell and whatever runs in it are ended.", "Close", true).await {
                    return;
                }
                let _ = term_call(&m, TermReq::Close { term: t.clone() }).await;
                terms().update(|all| {
                    if let Some(e) = all.get_mut(&id) {
                        let i = e.0.iter().position(|x| *x == t).unwrap_or(0);
                        e.0.retain(|x| *x != t);
                        if e.1.as_deref() == Some(t.as_str()) {
                            e.1 = e.0.get(i.min(e.0.len().saturating_sub(1))).cloned();
                        }
                    }
                });
            });
        })
    };
    view! {
        <div class="tx">
            <div class="tx-tabs">
                {move || list.get().0.into_iter().enumerate().map(|(i, t)| {
                    let (t1, t2, t3) = (t.clone(), t.clone(), t.clone());
                    view! {
                        <div class="tx-tab" class:on=move || list.with(|l| l.1.as_deref() == Some(t1.as_str())) on:click=move |_| pick.run(t2.clone())>
                            <Icon name="terminal" size=12/>
                            <span>{format!("{}", i + 1)}</span>
                            <button class="tx-x" title="Close" on:click=move |e| { e.stop_propagation(); close.run(t3.clone()) }><Icon name="x" size=11/></button>
                        </div>
                    }
                }).collect_view()}
                <button class="icon-btn tx-add" title="New terminal" on:click=add><Icon name="plus" size=14/></button>
            </div>
            {move || err.get().map(|e| view! { <div class="err small" style="padding:10px">{e}</div> })}
            {move || match (place.get(), list.get().1) {
                (Some((m, c)), Some(t)) => view! { <Term machine=m cwd=c id=t/> }.into_any(),
                _ => ().into_any(),
            }}
        </div>
    }
}

struct State {
    parser: vt100::Parser,
    /// The HTML each row shows now.
    rows: Vec<String>,
    /// Output is drawn (the replay is in).
    ready: bool,
    frame: bool,
    focused: bool,
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).unwrap()
}

/// Draw the rows that changed.
fn paint(st: &Rc<RefCell<State>>, el: &web_sys::HtmlElement) {
    let mut s = st.borrow_mut();
    s.frame = false;
    let screen = s.parser.screen();
    let (rows, _) = screen.size();
    let rows = rows.get();
    let cur = (!screen.hide_cursor() && screen.scrollback() == 0).then(|| screen.cursor_position());
    let doc = web_sys::window().unwrap().document().unwrap();
    let kids = el.children();
    while kids.length() > rows as u32 {
        if let Some(k) = el.last_element_child() {
            k.remove();
        }
    }
    while kids.length() < rows as u32 {
        let d = doc.create_element("div").unwrap();
        d.set_class_name("t-row");
        let _ = el.append_child(&d);
    }
    let html: Vec<String> = (0..rows).map(|r| row_html(screen, r, cur.filter(|c| c.0 == r).map(|c| c.1))).collect();
    s.rows.resize(rows as usize, String::from("\u{0}"));
    for (r, h) in html.into_iter().enumerate() {
        if s.rows[r] != h {
            if let Some(k) = kids.item(r as u32) {
                k.set_inner_html(&h);
            }
            s.rows[r] = h;
        }
    }
}

fn schedule(st: &Rc<RefCell<State>>, el: &web_sys::HtmlElement) {
    if std::mem::replace(&mut st.borrow_mut().frame, true) {
        return;
    }
    let (st, el) = (st.clone(), el.clone());
    let f = Closure::once_into_js(move || paint(&st, &el));
    let _ = web_sys::window().unwrap().request_animation_frame(f.unchecked_ref());
}

/// Rows and columns that fit `el`, from one character's size.
fn fit(el: &web_sys::HtmlElement, probe: &web_sys::HtmlElement) -> (u16, u16) {
    let r = probe.get_bounding_client_rect();
    let (cw, ch) = (r.width() / 10.0, r.height());
    if cw <= 0.0 || ch <= 0.0 || el.client_width() <= 0 {
        return (24, 80);
    }
    let rows = ((el.client_height() as f64 - 8.0) / ch).floor().max(2.0) as u16;
    let cols = ((el.client_width() as f64 - 12.0) / cw).floor().max(10.0) as u16;
    (rows, cols)
}

#[component]
fn Term(machine: String, cwd: String, id: String) -> impl IntoView {
    let screen = NodeRef::<leptos::html::Div>::new();
    let probe = NodeRef::<leptos::html::Span>::new();
    let input = NodeRef::<leptos::html::Textarea>::new();
    let exited = RwSignal::new(None::<Option<i32>>);
    let failed = RwSignal::new(None::<String>);
    let focused = RwSignal::new(false);
    let st = Rc::new(RefCell::new(State { parser: vt100::Parser::new(nz(24), nz(80), SCROLLBACK), rows: vec![], ready: false, frame: false, focused: false }));
    let ch = format!("term:{machine}:{id}");
    subscribe(&[&ch]);
    let send = {
        let (m, t) = (machine.clone(), id.clone());
        Rc::new(move |b: &[u8]| agentum_ui::ws::send(WsIn::TermInput { machine: m.clone(), term: t.clone(), data: b64_encode(b) }))
    };
    {
        let (m, t, st) = (machine.clone(), id.clone(), st.clone());
        on_ws(move |msg| match msg {
            WsOut::TermData { machine, term, data } if *machine == m && *term == t => {
                if !st.borrow().ready {
                    return;
                }
                st.borrow_mut().parser.process(&b64_decode(data));
                if let Some(el) = screen.get_untracked() {
                    schedule(&st, &el);
                }
            }
            WsOut::TermExit { machine, term, code } if *machine == m && *term == t => exited.set(Some(*code)),
            _ => {}
        });
    }
    // Open (or reattach to) the shell at the panel's size, then repaint what it showed.
    let open = {
        let (m, c, t, st) = (machine.clone(), cwd.clone(), id.clone(), st.clone());
        Rc::new(move || {
            let (Some(el), Some(pr)) = (screen.get_untracked(), probe.get_untracked()) else { return };
            let (rows, cols) = fit(&el, &pr);
            {
                let mut s = st.borrow_mut();
                s.parser = vt100::Parser::new(nz(rows), nz(cols), SCROLLBACK);
                s.ready = false;
                s.rows.clear();
            }
            exited.set(None);
            failed.set(None);
            let (m, c, t, st) = (m.clone(), c.clone(), t.clone(), st.clone());
            spawn_local(async move {
                match term_call(&m, TermReq::Open { term: t.clone(), cwd: c, cols, rows, cmd: None }).await {
                    Ok(v) => {
                        if v["existing"].as_bool().unwrap_or(false) {
                            if let Ok(r) = term_call(&m, TermReq::Replay { term: t }).await {
                                st.borrow_mut().parser.process(&b64_decode(r["data"].as_str().unwrap_or("")));
                            }
                        }
                        st.borrow_mut().ready = true;
                        paint(&st, &el);
                        if let Some(i) = input.get_untracked() {
                            let _ = i.focus();
                        }
                    }
                    Err(e) => failed.set(Some(e.message)),
                }
            });
        })
    };
    // Once mounted: open, and follow the panel's size.
    {
        let (open, st, m, t) = (open.clone(), st.clone(), machine.clone(), id.clone());
        let observer = StoredValue::new_local(None::<(web_sys::ResizeObserver, Closure<dyn FnMut()>)>);
        Effect::new(move |_| {
            let (Some(el), Some(pr)) = (screen.get(), probe.get()) else { return };
            open();
            let timer = Rc::new(RefCell::new(None::<gloo_timers::callback::Timeout>));
            let (st, m, t, el2) = (st.clone(), m.clone(), t.clone(), el.clone());
            let cb = Closure::<dyn FnMut()>::new(move || {
                let (st, m, t, el, pr) = (st.clone(), m.clone(), t.clone(), el2.clone(), pr.clone());
                *timer.borrow_mut() = Some(gloo_timers::callback::Timeout::new(80, move || {
                    let (rows, cols) = fit(&el, &pr);
                    let changed = {
                        let mut s = st.borrow_mut();
                        let (r0, c0) = s.parser.screen().size();
                        let changed = (r0.get(), c0.get()) != (rows, cols);
                        if changed {
                            s.parser.screen_mut().set_size(nz(rows), nz(cols));
                        }
                        changed && s.ready
                    };
                    if changed {
                        agentum_ui::ws::send(WsIn::TermResize { machine: m.clone(), term: t.clone(), cols, rows });
                        paint(&st, &el);
                    }
                }));
            });
            if let Ok(ro) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
                ro.observe(&el);
                observer.set_value(Some((ro, cb)));
            }
        });
        on_cleanup(move || {
            observer.try_with_value(|o| {
                if let Some((ro, _)) = o {
                    ro.disconnect();
                }
            });
        });
    }
    let on_key = {
        let (st, send, open) = (st.clone(), send.clone(), open.clone());
        move |e: web_sys::KeyboardEvent| {
            if e.is_composing() {
                return;
            }
            if exited.get_untracked().is_some() {
                if e.key() == "Enter" {
                    e.prevent_default();
                    open();
                }
                return;
            }
            let app = st.borrow().parser.screen().application_cursor();
            let Some(b) = key_bytes(&e.key(), e.ctrl_key(), e.alt_key(), e.shift_key(), e.meta_key(), app) else { return };
            e.prevent_default();
            // Keys are the shell's (⌃B, ⌃K…), not the app's shortcuts.
            e.stop_propagation();
            {
                let mut s = st.borrow_mut();
                if s.parser.screen().scrollback() > 0 {
                    s.parser.screen_mut().set_scrollback(0);
                    drop(s);
                    if let Some(el) = screen.get_untracked() {
                        schedule(&st, &el);
                    }
                }
            }
            send(&b);
        }
    };
    let on_paste = {
        let (st, send) = (st.clone(), send.clone());
        move |e: web_sys::ClipboardEvent| {
            let Some(text) = e.clipboard_data().and_then(|d| d.get_data("text").ok()) else { return };
            e.prevent_default();
            let br = st.borrow().parser.screen().bracketed_paste();
            send(&paste_bytes(&text, br));
        }
    };
    let on_compose = {
        let send = send.clone();
        move |e: web_sys::CompositionEvent| {
            if let Some(d) = e.data().filter(|d| !d.is_empty()) {
                send(d.as_bytes());
            }
            if let Some(i) = input.get_untracked() {
                i.set_value("");
            }
        }
    };
    let on_wheel = {
        let (st, send) = (st.clone(), send.clone());
        move |e: web_sys::WheelEvent| {
            let lines = ((e.delta_y() / 40.0).round() as i64).clamp(-10, 10);
            if lines == 0 {
                return;
            }
            e.prevent_default();
            let (alt, app, mouse) = {
                let s = st.borrow();
                let sc = s.parser.screen();
                (sc.alternate_screen(), sc.application_cursor(), sc.mouse_protocol_mode() != vt100::MouseProtocolMode::None)
            };
            if alt {
                // Full-screen programs (less, vim) scroll by arrows.
                if !mouse {
                    let k = if lines < 0 { "ArrowUp" } else { "ArrowDown" };
                    for _ in 0..lines.abs().min(5) {
                        if let Some(b) = key_bytes(k, false, false, false, false, app) {
                            send(&b);
                        }
                    }
                }
                return;
            }
            {
                let mut s = st.borrow_mut();
                let now = s.parser.screen().scrollback() as i64;
                s.parser.screen_mut().set_scrollback((now - lines).max(0) as usize);
            }
            if let Some(el) = screen.get_untracked() {
                schedule(&st, &el);
            }
        }
    };
    let focus_input = move |_| {
        // Keep a selection (to copy); else type here.
        let selecting = web_sys::window().and_then(|w| w.get_selection().ok().flatten()).is_some_and(|s| !s.is_collapsed());
        if !selecting {
            if let Some(i) = input.get_untracked() {
                let _ = i.focus();
            }
        }
    };
    let st2 = st.clone();
    let set_focus = move |f: bool| {
        focused.set(f);
        st2.borrow_mut().focused = f;
    };
    let sf = set_focus.clone();
    // Reactive views must be Send: reach the (Rc) opener through a local store.
    let reopen = StoredValue::new_local(open.clone());
    view! {
        <div class="tx-term" class:focused=move || focused.get() on:mouseup=focus_input on:wheel=on_wheel>
            <span class="tx-probe" node_ref=probe aria-hidden="true">"MMMMMMMMMM"</span>
            <div class="tx-screen" node_ref=screen></div>
            <textarea class="tx-input" node_ref=input autocapitalize="off" autocomplete="off" spellcheck="false" aria-label="Terminal input"
                on:keydown=on_key on:paste=on_paste on:compositionend=on_compose
                on:input=move |e| { if let Some(i) = input.get_untracked() { if !e.dyn_ref::<web_sys::InputEvent>().is_some_and(|x| x.is_composing()) { i.set_value("") } } }
                on:focus=move |_| set_focus(true) on:blur=move |_| sf(false)></textarea>
            {move || failed.get().map(|e| view! { <div class="tx-note err">{e}</div> })}
            {move || exited.get().map(|code| view! {
                <div class="tx-note">
                    {format!("The shell ended{}.", code.map(|c| format!(" (exit {c})")).unwrap_or_default())}
                    <button class="btn tx-sm" on:click=move |_| reopen.with_value(|f| f())>"Start a new one"</button>
                </div>
            })}
        </div>
    }
}
