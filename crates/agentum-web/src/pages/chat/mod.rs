//! The Secretary: private chats on your own model, with tools over your mail, calendar, to-dos,
//! notes and code. Answers stream in (`POST /api/chat`, SSE); tools show as receipts; sending
//! mail, calendar changes and coding tasks wait for your approval (`POST /api/chat/approve`);
//! a ```ui block is drawn ([`genui`]). Files attach as text, the mic writes out what you say, and a
//! spoken question gets its answer read aloud. Nobody chat saves nothing and has no tools.
//! `?chat=<id>` (empty: a new one), `?action=nobody`, `?q=…&ctx=…` (asked from Mail or Calendar).
mod attach;
pub mod genui;
pub mod md;
pub mod side;
mod stream;
mod voice;

use super::Pane;
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use leptos::portal::Portal;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;

#[derive(Clone, Debug, Default, PartialEq)]
struct Tool {
    name: String,
    args: Value,
    status: String,
    result: Option<String>,
    summary: Option<String>,
    approval: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq)]
struct FileRef {
    id: String,
    name: String,
    parts: u64,
}
#[derive(Clone, Debug, Default)]
struct Msg {
    k: u64,
    id: Option<i64>,
    user: bool,
    content: String,
    tools: Vec<Tool>,
    sources: Vec<Value>,
    files: Vec<FileRef>,
    error: Option<String>,
    live: bool,
    status: Option<String>,
}
#[derive(Clone, Debug)]
struct Attachment {
    key: u64,
    name: String,
    id: Option<String>,
    parts: u64,
    error: Option<String>,
}
type Line = (u64, ArcRwSignal<Msg>);

thread_local! {
    static SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
fn next_key() -> u64 {
    SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    })
}

fn label(name: &str) -> &str {
    match name {
        "calendar_list" => "Checked your calendar",
        "calendar_create" => "Create an event",
        "calendar_update" => "Change an event",
        "calendar_delete" => "Delete an event",
        "mail_search" => "Searched your mail",
        "mail_read" => "Read an email",
        "mail_send" => "Send an email",
        "todo_list" => "Checked your to-dos",
        "todo_add" => "Added a to-do",
        "todo_done" => "Completed a to-do",
        "note_add" => "Saved a note",
        "memory_add" => "Remembered",
        "memory_forget" => "Forgot a memory",
        "file_read" => "Read the file",
        "web_search" => "Searched the web",
        "web_fetch" => "Read a web page",
        "mail_waiting" => "Checked who is waiting",
        "code_status" => "Asked about your coding threads",
        "code_task" => "Start a coding task",
        n => n,
    }
}
fn status_text(s: &str) -> &str {
    match s {
        "running" => "running…",
        "completed" => "done",
        "failed" => "failed",
        "interrupted" => "interrupted",
        "declined" => "declined",
        "proposed" => "waiting for you",
        s => s,
    }
}
fn is_write(n: &str) -> bool {
    matches!(n, "calendar_create" | "calendar_update" | "calendar_delete" | "mail_send" | "code_task")
}

fn tool_of(v: &Value) -> Tool {
    let opt = |k: &str| v[k].as_str().map(str::to_string);
    Tool { name: v["name"].as_str().unwrap_or("").into(), args: v["args"].clone(), status: v["status"].as_str().unwrap_or("").into(), result: opt("result"), summary: opt("summary"), approval: opt("approval") }
}
fn files_of(v: &Value) -> Vec<FileRef> {
    v.as_array().into_iter().flatten().map(|f| FileRef { id: f["id"].as_str().unwrap_or("").into(), name: f["name"].as_str().unwrap_or("").into(), parts: f["parts"].as_u64().unwrap_or(1) }).collect()
}

/// The page's state: every field is a handle, so the whole thing is Copy.
#[derive(Clone, Copy)]
struct St {
    sid: RwSignal<Option<String>>,
    msgs: RwSignal<Vec<Line>>,
    text: RwSignal<String>,
    busy: RwSignal<bool>,
    loading: RwSignal<bool>,
    nobody: RwSignal<bool>,
    think: RwSignal<bool>,
    rag: RwSignal<bool>,
    attached: RwSignal<Vec<Attachment>>,
    voice: RwSignal<bool>,
    rec_on: RwSignal<bool>,
    hearing: RwSignal<bool>,
    reload: RwSignal<u32>,
    /// The chat this page just created while answering: it's on screen, don't load it over the answer.
    born: StoredValue<Option<String>>,
    abort: StoredValue<Option<web_sys::AbortController>, LocalStorage>,
    rec: StoredValue<Option<voice::Recording>, LocalStorage>,
    follow: StoredValue<bool>,
    round_at: StoredValue<usize>,
    via_voice: StoredValue<bool>,
    load_gen: StoredValue<u32>,
    scroll: NodeRef<leptos::html::Div>,
    input: NodeRef<leptos::html::Textarea>,
}

impl St {
    fn scroll_down(self) {
        if !self.follow.get_value() {
            return;
        }
        let el = self.scroll;
        request_animation_frame(move || {
            if let Some(e) = el.get_untracked() {
                e.set_scroll_top(e.scroll_height());
            }
        });
    }
    fn push(self, m: Msg) {
        self.msgs.update(|v| v.push((m.k, ArcRwSignal::new(m))));
        self.scroll_down();
    }
    fn last(self) -> Option<ArcRwSignal<Msg>> {
        self.msgs.with_untracked(|v| v.last().map(|l| l.1.clone()))
    }
    fn patch_last(self, f: impl FnOnce(&mut Msg)) {
        if let Some(m) = self.last().filter(|m| m.with_untracked(|m| !m.user)) {
            m.update(f);
            self.scroll_down();
        }
    }
    fn is_current(self, ctl: &web_sys::AbortController) -> bool {
        self.abort.with_value(|a| a.as_ref().is_some_and(|a| wasm_bindgen::JsValue::from(a.clone()) == wasm_bindgen::JsValue::from(ctl.clone())))
    }
    fn focus(self) {
        let i = self.input;
        request_animation_frame(move || {
            if let Some(t) = i.get_untracked() {
                let _ = t.focus();
            }
        });
    }
    fn reset_height(self) {
        if let Some(t) = self.input.get_untracked() {
            let _ = web_sys::HtmlElement::style(&t).set_property("height", "");
        }
    }

    fn handle(self, e: &Value) {
        let s = |k: &str| e[k].as_str().unwrap_or("").to_string();
        match e["type"].as_str().unwrap_or("") {
            "session" => {
                let id = s("id");
                self.born.set_value(Some(id.clone()));
                self.sid.set(Some(id.clone()));
                set_param("chat", Some(&id));
            }
            "round" => {
                let n = self.last().map(|m| m.with_untracked(|m| m.content.len())).unwrap_or(0);
                self.round_at.set_value(n);
            }
            "delta" => self.patch_last(|m| m.content.push_str(e["text"].as_str().unwrap_or(""))),
            "status" => self.patch_last(|m| m.status = Some(s("label"))),
            "sources" => self.patch_last(|m| m.sources = e["sources"].as_array().cloned().unwrap_or_default()),
            "tool" | "approval" => {
                let round_at = self.round_at.get_value();
                self.patch_last(|m| {
                    let mut t = tool_of(e);
                    if e["type"] == "approval" {
                        t.approval = e["id"].as_str().map(str::to_string);
                    }
                    let key = e["approval"].as_str().or(e["id"].as_str()).map(str::to_string);
                    let i = m.tools.iter().position(|x| match &key {
                        Some(k) => x.approval.as_deref() == Some(k.as_str()),
                        None => x.name == t.name && x.status == "running",
                    });
                    match i {
                        Some(i) => m.tools[i] = t,
                        None => m.tools.push(t),
                    }
                    // Words said before a tool call are the model thinking aloud, not the answer.
                    let cut = round_at.min(m.content.len());
                    if m.content.is_char_boundary(cut) {
                        let aloud: String = m.content[cut..].trim().chars().take(140).collect();
                        m.content.truncate(cut);
                        if !aloud.is_empty() {
                            m.status = Some(aloud);
                        }
                    }
                });
            }
            "error" => self.patch_last(|m| m.error = Some(s("error"))),
            "done" => {
                self.patch_last(|m| {
                    m.live = false;
                    if let Some(id) = e["message_id"].as_i64() {
                        m.id = Some(id);
                    }
                });
                side::reload();
            }
            _ => {}
        }
    }

    fn reading(self) -> bool {
        self.attached.with_untracked(|a| a.iter().any(|a| a.id.is_none() && a.error.is_none()))
    }

    /// Ask. `fresh`: a new chat started from elsewhere (incognito, its context).
    fn send(self, typed: String, fresh: Option<(bool, Option<String>)>) {
        let files: Vec<FileRef> = if fresh.is_some() { vec![] } else { self.attached.with_untracked(|a| a.iter().filter_map(|a| a.id.clone().map(|id| FileRef { id, name: a.name.clone(), parts: a.parts })).collect()) };
        let message = if typed.trim().is_empty() && !files.is_empty() { "What’s in this file?".to_string() } else { typed.trim().to_string() };
        if message.is_empty() || self.abort.with_value(|a| a.is_some()) || (fresh.is_none() && self.reading()) {
            return;
        }
        let via_voice = self.via_voice.get_value() && fresh.is_none();
        if via_voice {
            voice::unlock();
        }
        let incognito = fresh.as_ref().map(|f| f.0).unwrap_or_else(|| self.nobody.get_untracked());
        let history: Vec<Value> = if incognito && fresh.is_none() {
            self.msgs.with_untracked(|v| v.iter().map(|(_, m)| m.with_untracked(|m| json!({"role": if m.user { "user" } else { "assistant" }, "content": m.content}))).collect())
        } else {
            vec![]
        };
        let Ok(ctl) = web_sys::AbortController::new() else { return };
        self.abort.set_value(Some(ctl.clone()));
        self.follow.set_value(true);
        self.via_voice.set_value(false);
        voice::stop();
        let k = next_key();
        // A spoken question: read the answer aloud as it arrives, a sentence at a time.
        let aloud = Rc::new(RefCell::new(via_voice.then(|| voice::speak_stream(&k.to_string(), &message))));
        self.push(Msg { k: next_key(), user: true, content: message.clone(), files: files.clone(), ..Default::default() });
        self.push(Msg { k, live: true, ..Default::default() });
        self.text.set(String::new());
        self.busy.set(true);
        if fresh.is_none() {
            self.attached.set(vec![]);
        }
        self.reset_height();
        let body = json!({
            "session": if fresh.is_some() { None } else { self.sid.get_untracked() },
            "incognito": incognito,
            "context": fresh.as_ref().and_then(|f| f.1.clone()),
            "message": message,
            "history": history,
            "think": self.think.get_untracked(),
            "rag": self.rag.get_untracked() && !incognito,
            "files": files.iter().map(|f| f.id.clone()).collect::<Vec<_>>(),
        });
        spawn_local(async move {
            let signal = ctl.signal();
            let a2 = aloud.clone();
            let r = stream::post_stream("/chat", &body, &signal, |e| {
                if signal.aborted() {
                    return;
                }
                if e["type"] == "delta" {
                    if let Some(a) = a2.borrow_mut().as_mut() {
                        a.push(e["text"].as_str().unwrap_or(""));
                    }
                }
                self.handle(&e);
            })
            .await;
            let aborted = signal.aborted();
            if aborted {
                if aloud.borrow().is_some() {
                    voice::stop();
                }
            } else if let Some(a) = aloud.borrow_mut().as_mut() {
                a.end();
            }
            if let (Err(e), false) = (r, aborted) {
                if !e.is_empty() {
                    self.patch_last(|m| m.error = Some(e));
                }
            }
            if self.is_current(&ctl) {
                self.abort.set_value(None);
                self.busy.set(false);
                self.patch_last(|m| {
                    m.live = false;
                    if aborted {
                        m.status = Some("Stopped".into());
                    }
                });
            }
        });
    }

    /// Approve or decline a proposed write; the card updates and the answer goes on below.
    fn decide(self, approval: String, approve: bool) {
        if self.abort.with_value(|a| a.is_some()) {
            return;
        }
        let Ok(ctl) = web_sys::AbortController::new() else { return };
        self.abort.set_value(Some(ctl.clone()));
        self.busy.set(true);
        self.push(Msg { k: next_key(), live: true, ..Default::default() });
        spawn_local(async move {
            let signal = ctl.signal();
            let r = stream::post_stream("/chat/approve", &json!({"approval_id": approval, "approve": approve}), &signal, |e| {
                if signal.aborted() {
                    return;
                }
                if let (Some("tool"), Some(a)) = (e["type"].as_str(), e["approval"].as_str()) {
                    self.msgs.with_untracked(|v| {
                        for (_, m) in v {
                            if m.with_untracked(|m| m.tools.iter().any(|t| t.approval.as_deref() == Some(a))) {
                                m.update(|m| {
                                    for t in m.tools.iter_mut().filter(|t| t.approval.as_deref() == Some(a)) {
                                        t.status = e["status"].as_str().unwrap_or("").into();
                                        t.result = e["result"].as_str().map(str::to_string);
                                    }
                                });
                            }
                        }
                    });
                    return;
                }
                self.handle(&e);
            })
            .await;
            if let (Err(e), false) = (r, signal.aborted()) {
                if !e.is_empty() {
                    toast_bad(e);
                }
            }
            if self.is_current(&ctl) {
                self.abort.set_value(None);
                self.busy.set(false);
                if self.last().is_some_and(|m| m.with_untracked(|m| !m.user && m.content.is_empty() && m.tools.is_empty() && m.error.is_none())) {
                    self.msgs.update(|v| {
                        v.pop();
                    });
                }
                self.patch_last(|m| m.live = false);
            }
        });
    }

    fn stop(self) {
        if let Some(id) = self.sid.get_untracked() {
            spawn_local(async move {
                let _ = api_post::<Value>("/chat/stop", &json!({"session": id})).await;
            });
        }
        if let Some(a) = self.abort.with_value(|a| a.clone()) {
            a.abort();
        }
    }

    fn switch_chat(self, id: Option<String>, private: bool) {
        if let Some(a) = self.abort.with_value(|a| a.clone()) {
            a.abort();
        }
        self.abort.set_value(None);
        self.born.set_value(None);
        self.follow.set_value(true);
        if self.nobody.get_untracked() {
            self.text.set(String::new());
            self.reset_height();
        }
        if private {
            self.attached.set(vec![]);
        }
        voice::stop();
        self.busy.set(false);
        self.loading.set(id.is_some());
        self.msgs.set(vec![]);
        self.nobody.set(private);
        self.sid.set(id.clone());
        set_param("chat", id.as_deref());
        set_param("action", private.then_some("nobody"));
        if id.is_none() {
            self.focus();
        }
    }

    fn attach(self, files: Vec<web_sys::File>) {
        if files.is_empty() {
            return;
        }
        if self.nobody.get_untracked() {
            toast_bad("Files can’t be attached in Nobody chat.");
            return;
        }
        for file in files {
            let key = next_key();
            let name = file.name();
            self.attached.update(|a| a.push(Attachment { key, name: name.clone(), id: None, parts: 0, error: None }));
            let attached = self.attached;
            spawn_local(async move {
                let res = async {
                    let text = attach::extract_text(&file).await?;
                    api_post::<Value>("/chat/files", &json!({"name": name, "kind": file.type_(), "size": file.size() as i64, "text": text})).await.map_err(|e| e.message)
                }
                .await;
                attached.update(|a| {
                    if let Some(x) = a.iter_mut().find(|x| x.key == key) {
                        match res {
                            Ok(r) => {
                                x.id = r["id"].as_str().map(str::to_string);
                                x.parts = r["parts"].as_u64().unwrap_or(1);
                            }
                            Err(e) => x.error = Some(if e.is_empty() { "Couldn’t read this file.".into() } else { e }),
                        }
                    }
                });
            });
        }
    }

    fn cancel_mic(self) {
        self.rec.update_value(|r| {
            if let Some(r) = r.take() {
                r.cancel();
            }
        });
        self.rec_on.set(false);
    }

    fn toggle_mic(self) {
        voice::unlock();
        if self.rec_on.get_untracked() {
            let mut r = None;
            self.rec.update_value(|x| r = x.take());
            self.rec_on.set(false);
            let Some(r) = r else { return };
            self.hearing.set(true);
            spawn_local(async move {
                let blob = r.stop().await;
                match voice::transcribe(blob).await {
                    Ok(w) if w.trim().is_empty() => toast_bad("I didn’t catch anything. Try again a little closer to the mic."),
                    Ok(w) => {
                        self.text.update(|t| {
                            if !t.trim().is_empty() {
                                *t = format!("{} ", t.trim_end());
                            }
                            t.push_str(w.trim());
                        });
                        self.via_voice.set_value(true);
                        self.focus();
                    }
                    Err(e) => toast_bad(e),
                }
                self.hearing.set(false);
            });
            return;
        }
        voice::stop();
        spawn_local(async move {
            match voice::record().await {
                Ok(r) => {
                    self.rec.set_value(Some(r));
                    self.rec_on.set(true);
                }
                Err(e) => toast_bad(e),
            }
        });
    }
}

fn file_list(l: Option<web_sys::FileList>) -> Vec<web_sys::File> {
    l.map(|l| (0..l.length()).filter_map(|i| l.get(i)).collect()).unwrap_or_default()
}

pub fn page(pane: Pane) -> AnyView {
    view! { <Secretary pane=pane/> }.into_any()
}

#[component]
fn Secretary(pane: Pane) -> impl IntoView {
    let p_chat = use_param("chat");
    let p_action = use_param("action");
    let p_q = use_param("q");
    let p_ctx = use_param("ctx");
    let nobody0 = p_action.get_untracked().as_deref() == Some("nobody");
    let sid0 = if nobody0 { None } else { p_chat.get_untracked().filter(|c| !c.is_empty()) };
    let st = St {
        sid: RwSignal::new(sid0.clone()),
        msgs: RwSignal::new(vec![]),
        text: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
        loading: RwSignal::new(sid0.is_some()),
        nobody: RwSignal::new(nobody0),
        think: RwSignal::new(agentum_ui::store::get("cerq.chat.think").as_deref() == Some("on")),
        rag: RwSignal::new(agentum_ui::store::get("cerq.chat.rag").as_deref() != Some("off")),
        attached: RwSignal::new(vec![]),
        voice: RwSignal::new(false),
        rec_on: RwSignal::new(false),
        hearing: RwSignal::new(false),
        reload: RwSignal::new(0),
        born: StoredValue::new(None),
        abort: StoredValue::new_local(None),
        rec: StoredValue::new_local(None),
        follow: StoredValue::new(true),
        round_at: StoredValue::new(0),
        via_voice: StoredValue::new(false),
        load_gen: StoredValue::new(0),
        scroll: NodeRef::new(),
        input: NodeRef::new(),
    };
    let list_open = RwSignal::new(agentum_ui::store::get("cerq.chat.list").as_deref() == Some("open"));
    let panel = RwSignal::new(agentum_ui::store::get("cerq.chat.panel").as_deref() != Some("hidden"));
    let model_ok = RwSignal::new(None::<Value>);
    let memories = RwSignal::new(None::<usize>);
    spawn_local(async move {
        if let Ok(m) = api_get::<Value>("/model/status").await {
            model_ok.set(Some(m));
        }
    });
    spawn_local(async move { st.voice.set(voice::ready().await) });
    Effect::new(move |_| {
        st.msgs.with(|v| v.len());
        spawn_local(async move {
            if let Ok(m) = api_get::<Vec<Value>>("/memories").await {
                memories.set(Some(m.len()));
            }
        });
    });

    // Load the chat that is open (not the one being answered right here).
    Effect::new(move |_| {
        let id = st.sid.get();
        st.reload.track();
        if id.is_some() && id == st.born.get_value() {
            st.born.set_value(None);
            return;
        }
        let Some(id) = id else { return };
        let g = st.load_gen.get_value() + 1;
        st.load_gen.set_value(g);
        spawn_local(async move {
            let r = api_get::<Value>(&format!("/chat/sessions/{id}")).await;
            if st.load_gen.get_value() != g || st.sid.get_untracked().as_deref() != Some(id.as_str()) {
                return;
            }
            match r {
                Ok(d) => {
                    let lines: Vec<Line> = d["messages"].as_array().into_iter().flatten().filter(|m| m["role"] == "user" || m["role"] == "assistant").map(|m| {
                        let meta = &m["meta"];
                        let k = next_key();
                        let msg = Msg {
                            k,
                            id: m["id"].as_i64(),
                            user: m["role"] == "user",
                            content: m["content"].as_str().unwrap_or("").into(),
                            tools: meta["tools"].as_array().into_iter().flatten().map(tool_of).collect(),
                            sources: meta["sources"].as_array().cloned().unwrap_or_default(),
                            files: files_of(&meta["files"]),
                            error: meta["error"].as_str().map(str::to_string),
                            live: false,
                            status: (meta["stopped"] == true).then(|| "Stopped".to_string()),
                        };
                        (k, ArcRwSignal::new(msg))
                    }).collect();
                    st.msgs.set(lines);
                    st.follow.set_value(true);
                    st.scroll_down();
                }
                Err(_) => {
                    st.sid.set(None);
                    st.msgs.set(vec![]);
                }
            }
            st.loading.set(false);
        });
    });
    // An answer that went on while this page was elsewhere shows once it's saved.
    side::follow_changes();
    on_ws(move |m| {
        if let agentum_proto::api::WsOut::App { payload } = m {
            if payload["kind"] == "chat" && payload["session"].as_str().is_some() && payload["session"].as_str().map(str::to_string) == st.sid.get_untracked() && st.abort.with_value(|a| a.is_none()) {
                st.reload.update(|n| *n += 1);
            }
        }
    });
    // The sidebar picks the chat (?chat=<id>, or empty for a new one): follow it.
    Effect::new(move |_| {
        let w = p_chat.get().filter(|w| !w.is_empty());
        if p_action.get_untracked().as_deref() == Some("nobody") || p_q.get_untracked().is_some() {
            return;
        }
        if w != st.sid.get_untracked() && w != st.born.get_value() {
            st.switch_chat(w, false);
        }
    });
    // Asked from Mail / Calendar / Today: ?q=…&ctx=mail:acct:folder:uid — ask at once.
    Effect::new(move |_| {
        let Some(q) = p_q.get().filter(|q| !q.trim().is_empty()) else { return };
        let ctx = p_ctx.get_untracked();
        gloo_timers::callback::Timeout::new(0, move || {
            let private = st.nobody.get_untracked() || p_action.get_untracked().as_deref() == Some("nobody");
            set_param("q", None);
            set_param("ctx", None);
            st.switch_chat(None, private);
            st.send(q, Some((private, if private { None } else { ctx })));
        })
        .forget();
    });
    if web_sys::window().and_then(|w| w.match_media("(pointer: fine)").ok().flatten()).is_some_and(|m| m.matches()) {
        st.focus();
    }
    on_cleanup(move || {
        if let Some(Some(a)) = st.abort.try_with_value(|a| a.clone()) {
            a.abort();
        }
        let _ = st.rec.try_update_value(|r| {
            if let Some(r) = r.take() {
                r.cancel();
            }
        });
        voice::stop();
    });

    // The pane's header: new chat, the list of chats, the Secretary's panel.
    let tools_el = RwSignal::new_local(None::<web_sys::Element>);
    Effect::new(move |_| {
        let id = if pane == Pane::Left { "pane-tools-left" } else { "pane-tools-right" };
        tools_el.set(web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id)));
    });
    let tools = move || {
        tools_el.get().map(|el| {
            view! {
                <Portal mount=el>
                    <div class="row" style="gap:2px">
                        <button class="icon-btn" title="New chat" aria-label="New chat" on:click=move |_| st.switch_chat(None, st.nobody.get_untracked())><Icon name="plus"/></button>
                        <button class=move || if list_open.get() { "icon-btn on" } else { "icon-btn" } title="Your chats" aria-pressed=move || list_open.get().to_string()
                            on:click=move |_| { list_open.update(|o| *o = !*o); agentum_ui::store::set("cerq.chat.list", if list_open.get_untracked() { "open" } else { "" }) }><Icon name="menu"/></button>
                        <button class=move || if panel.get() { "icon-btn chat-panel-btn on" } else { "icon-btn chat-panel-btn" } title=move || if panel.get() { "Hide the Secretary’s panel" } else { "Show the Secretary’s panel" } aria-pressed=move || panel.get().to_string()
                            on:click=move |_| { panel.update(|p| *p = !*p); agentum_ui::store::set("cerq.chat.panel", if panel.get_untracked() { "" } else { "hidden" }) }><Icon name="book"/></button>
                    </div>
                </Portal>
            }
        })
    };

    let file_pick = NodeRef::<leptos::html::Input>::new();
    let submit = move || {
        if !st.loading.get_untracked() {
            st.send(st.text.get_untracked(), None);
        }
    };
    let rag_on = move || st.rag.get() && !st.nobody.get();
    let sendable = move || !st.loading.get() && !st.attached.with(|a| a.iter().any(|a| a.id.is_none() && a.error.is_none())) && (!st.text.get().trim().is_empty() || st.attached.with(|a| a.iter().any(|a| a.id.is_some())));

    view! {
        {tools}
        <div class="chat">
            <Show when=move || list_open.get()><ChatList st=st/></Show>
            <div class="chat-main">
                <div node_ref=st.scroll class="chat-scroll" on:scroll=move |e| {
                    if let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
                        st.follow.set_value(el.scroll_height() - el.scroll_top() - el.client_height() < 64);
                    }
                }>
                    <Show when=move || st.loading.get()><div class="empty">"Loading chat…"</div></Show>
                    <Show when=move || !st.loading.get() && st.msgs.with(|m| m.is_empty())>
                        <div class="chat-empty">
                            <span class="chat-empty-bot"><Icon name=Signal::derive(move || if st.nobody.get() { "eye".to_string() } else { "lock".to_string() }) size=40 class="muted"/></span>
                            <div class="hi" style="font-size:16px">{move || if st.nobody.get() { "Nobody chat" } else { "Start a chat" }}</div>
                            <div class="muted small">{move || if st.nobody.get() { "This chat is temporary and uses only your model. Web search and workspace tools are off." } else { "Ask your own model about your mail, calendar, to-dos, notes and code, or look something up online." }}</div>
                            <Show when=move || model_ok.with(|m| m.as_ref().is_some_and(|m| m["configured"] == false))>
                                <div class="small err">"No model is set up yet: pick one in Settings → Your model."</div>
                            </Show>
                            <Show when=move || !st.nobody.get()>
                                <div class="row" style="flex-wrap:wrap;justify-content:center;margin-top:8px">
                                    {["What do I have tomorrow?", "Who is waiting for a reply from me?", "Remind me to call the bank on Friday at 10"].into_iter().map(|q| view! {
                                        <button class="chip" on:click=move |_| st.send(q.to_string(), None)>{q}</button>
                                    }).collect_view()}
                                </div>
                            </Show>
                        </div>
                    </Show>
                    <For each=move || st.msgs.get() key=|l| l.0 let:line>
                        <Bubble m=line.1 st=st/>
                    </For>
                </div>
                <form class="composer"
                    on:submit=move |e| { e.prevent_default(); submit() }
                    on:dragover=move |e| { if e.data_transfer().is_some_and(|d| d.types().includes(&"Files".into(), 0)) { e.prevent_default() } }
                    on:drop=move |e| { let f = file_list(e.data_transfer().and_then(|d| d.files())); if !f.is_empty() { e.prevent_default(); st.attach(f) } }>
                    <Show when=move || st.attached.with(|a| !a.is_empty())>
                        <div class="chat-files">
                            {move || st.attached.get().into_iter().map(|a| {
                                let key = a.key;
                                let note = match (&a.error, &a.id) { (Some(_), _) => "can’t read".to_string(), (None, Some(_)) if a.parts > 1 => format!("{} parts", a.parts), (None, Some(_)) => "ready".into(), _ => "reading…".into() };
                                view! {
                                    <span class=if a.error.is_some() { "chat-file err" } else { "chat-file" } title=a.error.clone().unwrap_or_else(|| a.name.clone())>
                                        <Icon name=if a.error.is_some() { "alert" } else { "file" } size=13/>
                                        <span class="truncate">{a.name.clone()}</span>
                                        <span class="subdue small">{note}</span>
                                        <button type="button" class="icon-btn x" aria-label=format!("Remove {}", a.name) on:click=move |_| st.attached.update(|v| v.retain(|x| x.key != key))><Icon name="x" size=11/></button>
                                    </span>
                                }
                            }).collect_view()}
                        </div>
                    </Show>
                    <Show when=move || st.rec_on.get() || st.hearing.get()>
                        <div class="chat-voice" role="status">
                            {move || if st.rec_on.get() {
                                st.rec.with_value(|r| r.as_ref().map(|r| r.stream.clone())).map(|s| view! { <voice::Waveform stream=s/><voice::Elapsed/> }.into_any()).unwrap_or_else(|| ().into_any())
                            } else {
                                view! { <div class="typing"><i/><i/><i/><span>"Writing it out…"</span></div> }.into_any()
                            }}
                        </div>
                    </Show>
                    <textarea node_ref=st.input rows="1" aria-label="Message" hidden=move || st.rec_on.get() || st.hearing.get()
                        placeholder=move || if st.nobody.get() { "Ask without saving…" } else { "Message the Secretary…" }
                        prop:value=move || st.text.get()
                        on:paste=move |e| { let f = file_list(e.clipboard_data().and_then(|d| d.files())); if !f.is_empty() { e.prevent_default(); st.attach(f) } }
                        on:input=move |e| {
                            let v = event_target_value(&e);
                            if v.trim().is_empty() { st.via_voice.set_value(false) }
                            st.text.set(v);
                            if let Some(t) = st.input.get_untracked() {
                                let _ = web_sys::HtmlElement::style(&t).set_property("height", "auto");
                                let _ = web_sys::HtmlElement::style(&t).set_property("height", &format!("{}px", t.scroll_height().min(200)));
                            }
                        }
                        on:keydown=move |e| { if e.key() == "Enter" && !e.shift_key() && !e.is_composing() { e.prevent_default(); submit() } }/>
                    <div class="chat-composer-actions">
                        <div class="chat-options" role="group" aria-label="Message options">
                            <div class="chat-mode" role="group" aria-label="Response mode">
                                <button type="button" aria-label="Fast" title="Fast · Use the model’s lowest thinking setting" aria-pressed=move || (!st.think.get()).to_string()
                                    on:click=move |_| { st.think.set(false); agentum_ui::store::set("cerq.chat.think", "off") }><Icon name="bolt"/></button>
                                <button type="button" aria-label="Think" title="Think · Let the model reason before answering" aria-pressed=move || st.think.get().to_string()
                                    on:click=move |_| { st.think.set(true); agentum_ui::store::set("cerq.chat.think", "on") }><Icon name="bulb"/></button>
                            </div>
                            <button type="button" class="chat-option" disabled=move || st.nobody.get() || st.busy.get() aria-pressed=move || rag_on().to_string()
                                aria-label=move || format!("Online search {}", if rag_on() { "on" } else { "off" })
                                title=move || if st.nobody.get() { "Online search is off in Nobody chat".to_string() } else { format!("Turn online search {}", if rag_on() { "off" } else { "on" }) }
                                on:click=move |_| { st.rag.update(|r| *r = !*r); agentum_ui::store::set("cerq.chat.rag", if st.rag.get_untracked() { "on" } else { "off" }) }>
                                <Icon name=Signal::derive(move || if rag_on() { "search".to_string() } else { "search-off".to_string() })/>
                            </button>
                            <button type="button" class="chat-option" disabled=move || st.nobody.get() aria-label="Attach files"
                                title=move || if st.nobody.get() { "Files can’t be attached in Nobody chat" } else { "Attach files: Word, Excel, PowerPoint, text" }
                                on:click=move |_| { if let Some(i) = file_pick.get_untracked() { i.click() } }><Icon name="paperclip"/></button>
                            <input node_ref=file_pick type="file" multiple accept=attach::ACCEPT hidden on:change=move |e| {
                                let i = event_target::<web_sys::HtmlInputElement>(&e);
                                st.attach(file_list(i.files()));
                                i.set_value("");
                            }/>
                            <button type="button" class="chat-option" aria-label="Nobody" title=move || if st.nobody.get() { "Leave Nobody chat" } else { "Start a private Nobody chat" }
                                aria-pressed=move || st.nobody.get().to_string() on:click=move |_| st.switch_chat(None, !st.nobody.get_untracked())>
                                <Icon name=Signal::derive(move || if st.nobody.get() { "lock".to_string() } else { "eye".to_string() })/>
                            </button>
                        </div>
                        <Show when=move || st.rec_on.get()>
                            <button type="button" class="chat-option" aria-label="Discard recording" title="Discard" on:click=move |_| st.cancel_mic()><Icon name="x"/></button>
                        </Show>
                        <Show when=move || st.voice.get()>
                            <button type="button" class=move || if st.rec_on.get() { "chat-option chat-mic rec" } else { "chat-option chat-mic" } disabled=move || st.hearing.get()
                                aria-pressed=move || st.rec_on.get().to_string()
                                aria-label=move || if st.rec_on.get() { "Stop and write it out" } else if st.hearing.get() { "Writing it out" } else { "Talk" }
                                title=move || if st.rec_on.get() { "Done: write it out" } else if st.hearing.get() { "Writing it out…" } else { "Talk: speak your message" }
                                on:click=move |_| st.toggle_mic()>
                                <Icon name=Signal::derive(move || if st.rec_on.get() { "check".to_string() } else { "mic".to_string() })/>
                            </button>
                        </Show>
                        {move || if st.busy.get() {
                            view! { <button type="button" class="btn chat-send" title="Stop response" aria-label="Stop response" on:click=move |_| st.stop()><Icon name="stop"/></button> }.into_any()
                        } else {
                            view! { <button type="submit" class="btn primary chat-send" disabled=move || !sendable() title="Send message" aria-label="Send message"><Icon name="send"/></button> }.into_any()
                        }}
                    </div>
                </form>
            </div>
            <Show when=move || panel.get()>
                <aside class="bot-panel ag-bot-panel">
                    <div class="bot-panel-body">
                        <div class="row" style="gap:8px"><Icon name="lock" size=18/><b class="hi" style="font-size:15px">"Secretary"</b></div>
                        <section><h3>"What it remembers"</h3><p class="muted">
                            {move || match memories.get() { None => "…".to_string(), Some(0) => "Nothing yet. Tell it “remember …”.".into(), Some(1) => "1 thing about you.".into(), Some(n) => format!("{n} things about you.") }}
                            " "<button class="link-btn" on:click=move |_| navigate("settings", Where::Right, &[("tab", "memory")])>"See them"</button>
                        </p></section>
                        <section><h3>"What it can use"</h3><p class="muted">{move || format!("Your mail, calendar, to-dos, notes and coding threads, files you attach{}. It asks before it sends or changes anything.", if rag_on() { ", and the web" } else { "" })}</p></section>
                        <section><h3>"Its model"</h3><p class="muted">{move || match model_ok.get() {
                            Some(m) if m["configured"] == true => format!("Your own model{} (Settings → Your model). Nothing leaves your machines unless you search the web.", m["model"].as_str().filter(|x| !x.is_empty()).map(|x| format!(", {x}")).unwrap_or_default()),
                            Some(_) => "None set up yet: Settings → Your model.".into(),
                            None => "…".into(),
                        }}</p></section>
                    </div>
                </aside>
            </Show>
        </div>
    }
}

/// The chats in the page: open one, rename, pin, delete.
#[component]
fn ChatList(st: St) -> impl IntoView {
    let all = side::sessions();
    let editing = RwSignal::new(None::<(String, String)>);
    view! {
        <nav class="chat-list is-open" aria-label="Your chats">
            <button class="btn" style="width:100%;justify-content:center;margin-bottom:8px" on:click=move |_| st.switch_chat(None, false)><Icon name="plus" size=14/>"New chat"</button>
            {move || all.get().unwrap_or_default().into_iter().filter(|s| !s.archived).map(|s| {
                let (id, id2, id3) = (s.id.clone(), s.id.clone(), s.id.clone());
                let (s2, s3, s4) = (s.clone(), s.clone(), s.clone());
                let on = move || st.sid.get().as_deref() == Some(id.as_str());
                view! {
                    <div class=move || if on() { "chat-item on" } else { "chat-item" }>
                        {move || match editing.get().filter(|e| e.0 == id2) {
                            Some((_, name)) => view! {
                                <form class="grow" style="padding:4px" on:submit=move |e| { e.prevent_default(); if let Some((i, n)) = editing.get_untracked() { side::rename(&i, &n) } editing.set(None) }>
                                    <input autofocus style="width:100%" prop:value=name aria-label="Chat name"
                                        on:input=move |e| { let v = event_target_value(&e); editing.update(|x| if let Some(x) = x { x.1 = v }) }
                                        on:keydown=move |e| if e.key() == "Escape" { editing.set(None) }
                                        on:blur=move |_| { if let Some((i, n)) = editing.get_untracked() { side::rename(&i, &n) } editing.set(None) }/>
                                </form>
                            }.into_any(),
                            None => {
                                let (i, i2, t) = (id3.clone(), id3.clone(), s2.title());
                                view! {
                                    <button class="chat-pick grow" title=s2.title() on:click=move |_| st.switch_chat(Some(i.clone()), false)
                                        on:dblclick=move |_| editing.set(Some((i2.clone(), t.clone())))>
                                        <span class="col" style="gap:1px;min-width:0;align-items:flex-start">
                                            <span class="truncate" style="max-width:100%">{s2.pinned.then(|| view! { <Icon name="pin" size=10/> })}" "{s2.title()}</span>
                                            <span class="small subdue">{ago_ms(s2.at())}" · "{side::meta(&s2)}</span>
                                        </span>
                                    </button>
                                }.into_any()
                            }
                        }}
                        <button class="icon-btn x" title="Rename" aria-label="Rename" on:click={ let s = s3.clone(); move |_| editing.set(Some((s.id.clone(), s.title()))) }><Icon name="edit" size=12/></button>
                        <button class="icon-btn x" title=if s3.pinned { "Unpin" } else { "Pin" } aria-label=if s3.pinned { "Unpin" } else { "Pin" } on:click={ let s = s3.clone(); move |_| side::pin(&s.id, !s.pinned) }><Icon name="pin" size=12/></button>
                        <button class="icon-btn x" title="Delete" aria-label="Delete" on:click=move |_| side::delete(&s4)><Icon name="trash" size=12/></button>
                    </div>
                }
            }).collect_view()}
        </nav>
    }
}

/// One message: yours, or the Secretary's with its tools, answer, sources and read-aloud.
#[component]
fn Bubble(m: ArcRwSignal<Msg>, st: St) -> impl IntoView {
    let user = m.with_untracked(|m| m.user);
    if user {
        let (content, files) = m.with_untracked(|m| (m.content.clone(), m.files.clone()));
        return view! {
            <div class="msg user">
                {(!files.is_empty()).then(|| view! {
                    <div class="chat-files">{files.into_iter().map(|f| view! { <span class="chat-file"><Icon name="file" size=13/><span class="truncate">{f.name}</span></span> }).collect_view()}</div>
                })}
                <div class="bubble">{content}</div>
            </div>
        }
        .into_any();
    }
    let playing = voice::playing();
    let (m1, m2, m3, m4, m5) = (m.clone(), m.clone(), m.clone(), m.clone(), m.clone());
    let key = m.with_untracked(|m| m.k.to_string());
    let tools = Memo::new(move |_| m1.with(|m| (m.tools.clone(), m.live)));
    let content = Memo::new(move |_| m2.with(|m| (m.content.clone(), m.live, m.status.clone())));
    view! {
        <div class="msg ai">
            {move || {
                let (ts, live) = tools.get();
                ts.into_iter().map(|t| {
                    if t.status == "proposed" || (t.approval.is_some() && matches!(t.status.as_str(), "completed" | "failed" | "declined") && is_write(&t.name)) {
                        view! { <Approval t=t st=st/> }.into_any()
                    } else {
                        receipt(&t, live)
                    }
                }).collect_view()
            }}
            {move || {
                let (c, live, status) = content.get();
                if !c.is_empty() {
                    md::markdown(&c, true, None)
                } else if live {
                    view! { <div class="typing" role="status"><i/><i/><i/><span>{status.unwrap_or_else(|| "Starting…".into())}</span></div> }.into_any()
                } else {
                    ().into_any()
                }
            }}
            {move || m3.with(|m| (!m.live && m.status.as_deref() == Some("Stopped")).then(|| view! { <div class="muted small">"Stopped"</div> }))}
            {move || m4.with(|m| m.sources.clone()).into_iter().next().is_some().then(|| {
                let srcs = m4.with(|m| m.sources.clone());
                let n = srcs.len();
                view! {
                    <details class="chat-sources"><summary>{format!("{n} workspace {}", if n == 1 { "source" } else { "sources" })}</summary>
                        <ol>{srcs.into_iter().map(|s| {
                            let title = s["title"].as_str().unwrap_or("").to_string();
                            let href = s["href"].as_str().map(str::to_string);
                            view! { <li>{match href {
                                Some(h) => view! { <a href=h.clone() on:click=move |e| { if h.starts_with('/') { e.prevent_default(); genui::open_url(&h) } }>{title}</a> }.into_any(),
                                None => title.into_any(),
                            }}<span class="muted">" · "{s["kind"].as_str().unwrap_or("").to_string()}</span></li> }
                        }).collect_view()}</ol>
                    </details>
                }
            })}
            {move || m5.with(|m| m.error.clone()).map(|e| view! { <div class="err small">{e}</div> })}
            {move || {
                let show = st.voice.get() && m.with(|m| !m.live && !m.content.trim().is_empty());
                show.then(|| {
                    let (k, k2, mm) = (key.clone(), key.clone(), m.clone());
                    let on = Signal::derive(move || playing.get().as_deref() == Some(k2.as_str()));
                    view! {
                        <div class="msg-actions">
                            <button class=move || if on.get() { "icon-btn on" } else { "icon-btn" } title=move || if on.get() { "Stop reading" } else { "Read aloud" } aria-label="Read aloud"
                                aria-pressed=move || on.get().to_string()
                                on:click=move |_| { voice::unlock(); voice::speak(&k, &mm.with_untracked(|m| m.content.clone())) }>
                                <Icon name=Signal::derive(move || if on.get() { "stop".to_string() } else { "volume".to_string() }) size=14/>
                            </button>
                        </div>
                    }
                })
            }}
        </div>
    }
    .into_any()
}

fn receipt(t: &Tool, live: bool) -> AnyView {
    let status = if t.status == "running" && !live && t.approval.is_none() { "interrupted".to_string() } else { t.status.clone() };
    let name = if t.name == "web_search" && status != "completed" { "Web search".to_string() } else { label(&t.name).to_string() };
    let icon = match status.as_str() {
        "completed" => "check",
        "failed" => "alert",
        "interrupted" => "stop",
        _ => "clock",
    };
    view! {
        <details class=format!("tool {status}")>
            <summary><Icon name=icon size=12/><span>{name}</span><span class="tool-status subdue">"· "{status_text(&status).to_string()}</span></summary>
            {t.result.clone().map(|r| view! { <pre>{r}</pre> })}
        </details>
    }
    .into_any()
}

#[component]
fn Approval(t: Tool, st: St) -> impl IntoView {
    let a = t.args.clone();
    let s = |k: &str| a[k].as_str().unwrap_or("").to_string();
    let icon = match t.name.as_str() {
        "mail_send" => "mail",
        "code_task" => "bot",
        _ => "calendar",
    };
    let (ap1, ap2) = (t.approval.clone().unwrap_or_default(), t.approval.clone().unwrap_or_default());
    let proposed = t.status == "proposed" && t.approval.is_some();
    let outcome = (t.status != "proposed").then(|| match t.status.as_str() {
        "completed" => ("small pos", "Done.".to_string()),
        "failed" => ("small err", t.result.clone().unwrap_or_else(|| "It failed.".into())),
        _ => ("small subdue", "You declined.".to_string()),
    });
    view! {
        <div class=format!("approval {}", t.status)>
            <div class="row"><Icon name=icon size=14/><span class="hi grow">{label(&t.name).to_string()}</span><span class="small subdue">{status_text(&t.status).to_string()}</span></div>
            {if t.name == "mail_send" {
                view! {
                    <div class="mail-preview">
                        <div><span class="subdue">"From "</span>{if s("from").is_empty() { "your default account".to_string() } else { s("from") }}</div>
                        <div><span class="subdue">"To "</span>{s("to")}</div>
                        {(!s("cc").is_empty()).then(|| view! { <div><span class="subdue">"Cc "</span>{s("cc")}</div> })}
                        <div><span class="subdue">"Subject "</span>{s("subject")}</div>
                        <pre>{s("body")}</pre>
                    </div>
                }.into_any()
            } else {
                view! { <div class="muted">{t.summary.clone().unwrap_or_default()}</div> }.into_any()
            }}
            {outcome.map(|(cls, text)| view! { <div class=cls>{text}</div> })}
            {proposed.then(|| view! {
                <div class="row" style="justify-content:flex-end">
                    <button class="btn" disabled=move || st.busy.get() on:click=move |_| st.decide(ap1.clone(), false)>"Decline"</button>
                    <button class="btn primary" disabled=move || st.busy.get() on:click=move |_| st.decide(ap2.clone(), true)>{if t.name == "mail_send" { "Send" } else { "Approve" }}</button>
                </div>
            })}
        </div>
    }
}
