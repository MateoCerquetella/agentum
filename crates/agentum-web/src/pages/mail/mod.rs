//! Mail: the mail client and the Inbox Zero assistant in one pane. Wide: mail sidebar | list |
//! reading pane; mid: list | reading pane; narrow (phones, a thin split): one column with Back.
pub mod lib;
mod composer;
mod inbox;
pub mod push;
mod reader;
mod rules;
mod tabs;
pub mod watch;

use super::Pane;
use agentum_proto::api::WsOut;
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use lib::*;
use send_wrapper::SendWrapper;
use std::collections::{BTreeSet, HashSet};
use wasm_bindgen::{closure::Closure, JsCast};

/// One list of messages a tab owns (the inbox, To reply, …): rows act on it.
pub type List = RwSignal<Option<Vec<Msg>>>;

/// What the reader shows: the message, the row it came from (for thread actions), that row's list,
/// and what to do once it's open (reply, reply all, forward, an AI draft).
#[derive(Clone, PartialEq)]
pub struct Reading {
    pub item: Item,
    pub thread: Option<Thread>,
    pub set: List,
    pub then: Option<&'static str>,
}

#[derive(Clone, Default, PartialEq)]
pub struct Meta {
    pub accounts: Vec<Account>,
    pub mine: Vec<String>,
    pub llm: bool,
    pub interval: i64,
    pub last_run: i64,
}

#[derive(Clone, Default, PartialEq)]
pub struct Filter {
    pub folder: String,
    pub account: String,
    pub cat: String,
    pub unread: bool,
    pub q: String,
}

/// An open pane (reader, composer, rule editor, help) for Esc and the shortcuts.
#[derive(Clone)]
pub struct PaneReg {
    id: u64,
    kind: &'static str,
    prio: Signal<u8>,
    esc: Callback<()>,
    keys: Option<Callback<String, bool>>,
}

/// What every part of the Mail pane shares.
#[derive(Clone, Copy)]
pub struct MailCx {
    pub meta: RwSignal<Option<Meta>>,
    pub counts: RwSignal<Counts>,
    pub reply_ids: RwSignal<HashSet<String>>,
    pub sel: RwSignal<BTreeSet<String>>,
    pub focus: RwSignal<Option<String>>,
    pub reading: RwSignal<Option<Reading>>,
    pub draft: RwSignal<Option<Draft>>,
    pub front: RwSignal<&'static str>,
    pub undo: RwSignal<Option<(String, u64)>>,
    undo_fn: StoredValue<Option<SendWrapper<std::rc::Rc<dyn Fn()>>>>,
    pub ver: RwSignal<u32>,
    pub rows: StoredValue<Vec<(Thread, List)>>,
    panes: StoredValue<Vec<PaneReg>>,
    pub side: Pane,
}

pub fn use_mail() -> MailCx {
    expect_context::<MailCx>()
}

impl MailCx {
    pub fn accounts(&self) -> Vec<Account> {
        self.meta.with(|m| m.as_ref().map(|m| m.accounts.clone()).unwrap_or_default())
    }
    pub fn mine(&self) -> Vec<String> {
        self.meta.with_untracked(|m| m.as_ref().map(|m| m.mine.clone()).unwrap_or_default())
    }
    pub fn llm(&self) -> bool {
        self.meta.with(|m| m.as_ref().is_some_and(|m| m.llm))
    }
    pub fn color(&self, id: &str) -> String {
        let name = self.meta.with_untracked(|m| m.as_ref().and_then(|m| m.accounts.iter().find(|a| a.id == id).map(|a| a.name.clone())));
        color_of_name(&name.unwrap_or_else(|| id.to_string()))
    }
    pub fn toggle_sel(&self, k: &str, on: Option<bool>) {
        self.sel.update(|s| {
            if on.unwrap_or(!s.contains(k)) {
                s.insert(k.to_string());
            } else {
                s.remove(k);
            }
        });
    }
    pub fn open(&self, r: Reading) {
        self.focus.set(Some(r.item.key()));
        self.reading.set(Some(r));
        self.front.set("reader");
    }
    /// A new draft (asks before replacing the one being written).
    pub fn compose(self, mut d: Draft) {
        spawn_local(async move {
            if self.draft.get_untracked().is_some() && !confirm("Replace the open draft?", "The email you're writing will be discarded.", "Replace", true).await {
                return;
            }
            d.id = now_ms() as u64;
            self.draft.set(Some(d));
            self.front.set("compose");
        });
    }
    pub fn refresh_counts(self) {
        spawn_local(async move {
            if let Ok(c) = api_get::<Value>("/mail/counts").await {
                self.counts.set(Counts::of(&c));
            }
        });
    }
    /// The undo bar for 7 s.
    pub fn show_undo(self, text: String, f: impl Fn() + 'static) {
        let id = now_ms() as u64;
        self.undo_fn.set_value(Some(SendWrapper::new(std::rc::Rc::new(f))));
        self.undo.set(Some((text, id)));
        gloo_timers::callback::Timeout::new(7000, move || {
            if self.undo.try_get_untracked().flatten().is_some_and(|u| u.1 == id) {
                self.undo.set(None);
            }
        })
        .forget();
    }
    /// Register an open pane while the calling view lives.
    pub fn pane(self, kind: &'static str, prio: Signal<u8>, esc: Callback<()>, keys: Option<Callback<String, bool>>) {
        let id = now_ms() as u64 ^ (js_sys::Math::random() * 1e9) as u64;
        self.panes.update_value(|p| p.push(PaneReg { id, kind, prio, esc, keys }));
        on_cleanup(move || {
            let _ = self.panes.try_update_value(|p| p.retain(|x| x.id != id));
        });
    }
    /// The pane in front: highest priority, the newest among equals.
    fn top(self) -> Option<PaneReg> {
        self.panes.with_value(|ps| {
            let mut best: Option<&PaneReg> = None;
            for p in ps {
                if best.is_none_or(|b| p.prio.get_untracked() >= b.prio.get_untracked()) {
                    best = Some(p);
                }
            }
            best.cloned()
        })
    }
    /// The rows j/k walk through, registered by the tab on screen.
    pub fn use_rows(self, rows: impl Fn() -> Vec<(Thread, List)> + Send + Sync + 'static) {
        Effect::new(move |_| self.rows.set_value(rows()));
        on_cleanup(move || {
            let _ = self.rows.try_update_value(|r| r.clear());
        });
    }

    /// Run an action on rows: flags change at once and roll back on error; gone actions leave after
    /// the server agrees.
    pub async fn run(self, ts: Vec<Thread>, act: &'static str, set: List, bulk: bool) -> bool {
        let pick: Vec<Thread> = if bulk && needs(act, &ts.first().cloned().unwrap_or_else(|| single(&Msg::default()))).is_some() { ts.iter().filter(|t| needs(act, t) == Some(true)).cloned().collect() } else { ts.clone() };
        if pick.is_empty() {
            if !ts.is_empty() {
                let w = match act {
                    "mark_read" => "read",
                    "mark_unread" => "unread",
                    "star" => "starred",
                    _ => "unstarred",
                };
                toast(format!("Nothing to change: all {} already {w}", ts.len()));
            }
            return false;
        }
        let items: Vec<Item> = pick.iter().flat_map(|t| items_of(t, act)).collect();
        if items.is_empty() {
            return false;
        }
        let flag = undo_flag(act);
        if flag.is_some() {
            set.update(|ms| {
                if let Some(ms) = ms {
                    apply_local(ms, &items, act)
                }
            });
        }
        hush();
        match post_action(&items, act).await {
            Ok(r) => {
                hush();
                if gone(act) {
                    let g: HashSet<String> = pick.iter().map(|t| t.key.clone()).collect();
                    let rows: Vec<String> = self.rows.with_value(|r| r.iter().map(|(t, _)| t.key.clone()).collect());
                    self.focus.update(|f| {
                        if let Some(cur) = f.clone().filter(|c| g.contains(c)) {
                            let i = rows.iter().position(|k| *k == cur).unwrap_or(0);
                            *f = rows.iter().skip(i + 1).find(|k| !g.contains(*k)).or_else(|| rows.iter().take(i).rev().find(|k| !g.contains(*k))).cloned();
                        }
                    });
                    self.sel.update(|s| s.retain(|k| !g.contains(k)));
                    if self.reading.with_untracked(|r| r.as_ref().is_some_and(|r| g.contains(&r.item.key()))) {
                        self.reading.set(None);
                    }
                    set.update(|ms| {
                        if let Some(ms) = ms {
                            apply_local(ms, &items, act)
                        }
                    });
                }
                if bulk {
                    let t = format!(
                        "{}: {}{}{}",
                        action_label(act),
                        r.done,
                        if r.skipped > 0 { format!(" ({} already were)", r.skipped) } else { String::new() },
                        if r.errors.is_empty() { String::new() } else { format!(", {} failed", r.errors.len()) }
                    );
                    if r.errors.is_empty() { toast(t) } else { toast_bad(t) }
                } else if gone(act) {
                    toast(format!("{} done", action_label(act)));
                }
                if !r.errors.is_empty() && flag.is_some() {
                    self.ver.update(|v| *v += 1);
                }
                self.refresh_counts();
                true
            }
            Err(e) => {
                toast_bad(e);
                if let Some(back) = flag {
                    set.update(|ms| {
                        if let Some(ms) = ms {
                            apply_local(ms, &items, back)
                        }
                    });
                }
                false
            }
        }
    }
    pub fn run_now(self, ts: Vec<Thread>, act: &'static str, set: List) {
        spawn_local(async move {
            self.run(ts, act, set, false).await;
        });
    }
}

const TABS: [(&str, &str); 6] = [("inbox", "Inbox"), ("reply", "Reply Zero"), ("rules", "Rules"), ("clean", "Clean"), ("cold", "Cold email"), ("stats", "Analytics")];
fn tab_of(t: Option<String>) -> Option<String> {
    let t = t?;
    if t == "analytics" {
        return Some("stats".into());
    }
    TABS.iter().any(|(k, _)| *k == t).then_some(t)
}
const KEYS: [(&str, &str); 16] = [
    ("j / k", "Next / previous email"),
    ("Enter / o", "Open"),
    ("Esc", "Back / close"),
    ("c", "New email"),
    ("r", "Reply"),
    ("a", "Reply all"),
    ("f", "Forward"),
    ("e", "Archive"),
    ("#", "Delete"),
    ("!", "Spam"),
    ("s", "Star / unstar"),
    ("u", "Read / unread"),
    ("x", "Select"),
    ("/", "Search"),
    ("g then i / r / c / s / l", "Go to Inbox / Reply Zero / Clean / Analytics / Rules"),
    ("?", "This help"),
];

#[component]
fn KeysHelp(on_close: Callback<()>) -> impl IntoView {
    use_mail().pane("keys", Signal::derive(|| 3), on_close, None);
    view! {
        <div class="ml-pane ml-top">
            <div class="ml-pbar"><button type="button" class="btn ghost" on:click=move |_| on_close.run(())><Icon name="chevronL" size=14/>"Back"</button><span class="hi">"Keyboard shortcuts"</span></div>
            <table class="ml-table ml-keys"><tbody>
                {KEYS.iter().map(|(k, l)| view! { <tr><td><span class="kbd">{*k}</span></td><td>{*l}</td></tr> }).collect_view()}
            </tbody></table>
            <p class="subdue small">"Shortcuts work while Mail has the focus (click anywhere in it) and you're not typing."</p>
        </div>
    }
}

pub fn page(pane: Pane) -> AnyView {
    view! { <Mail side=pane/> }.into_any()
}

/// The element's width, live (ResizeObserver).
fn watch_width(el: NodeRef<leptos::html::Div>, width: RwSignal<f64>) {
    Effect::new(move |_| {
        let Some(e) = el.get() else { return };
        let cb = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            if let Ok(en) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() {
                width.set(en.content_rect().width());
            }
        });
        let Ok(ro) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) else { return };
        ro.observe(&e);
        let keep = SendWrapper::new((ro, cb));
        on_cleanup(move || keep.0.disconnect());
    });
}

#[component]
fn Mail(side: Pane) -> impl IntoView {
    let root = NodeRef::<leptos::html::Div>::new();
    let width = RwSignal::new(1200.0);
    watch_width(root, width);
    let layout = Memo::new(move |_| {
        let w = width.get();
        if w >= 980.0 { "wide" } else if w >= 700.0 { "mid" } else { "narrow" }
    });
    let tab = RwSignal::new(tab_of(use_param("tab").get_untracked()).unwrap_or_else(|| "inbox".into()));
    let meta_err = RwSignal::new(String::new());
    let broken = RwSignal::new(Vec::<(String, String)>::new());
    let msgs: List = RwSignal::new(None);
    let loading = RwSignal::new(false);
    let list_err = RwSignal::new(String::new());
    let filter = RwSignal::new(Filter { folder: "inbox".into(), ..Default::default() });
    let keys = RwSignal::new(false);
    let syncing = RwSignal::new(false);
    let push_on = RwSignal::new(false);
    let notify = RwSignal::new(push::notify_on());
    let tick = RwSignal::new(0u32);
    let cx = MailCx {
        meta: RwSignal::new(None),
        counts: RwSignal::new(Counts::default()),
        reply_ids: RwSignal::new(HashSet::new()),
        sel: RwSignal::new(BTreeSet::new()),
        focus: RwSignal::new(None),
        reading: RwSignal::new(None),
        draft: RwSignal::new(None),
        front: RwSignal::new("reader"),
        undo: RwSignal::new(None),
        undo_fn: StoredValue::new(None),
        ver: RwSignal::new(0),
        rows: StoredValue::new(vec![]),
        panes: StoredValue::new(vec![]),
        side,
    };
    provide_context(cx);

    let minute = gloo_timers::callback::Interval::new(60_000, move || tick.update(|n| *n += 1));
    let minute = SendWrapper::new(minute);
    on_cleanup(move || drop(minute));
    spawn_local(async move { push_on.set(push::push_state().await) });

    let load_meta = move || {
        spawn_local(async move {
            let (a, al) = (api_get::<Value>("/mail/accounts").await, api_get::<Value>("/mail/aliases").await);
            match a {
                Ok(a) => {
                    let accounts = a["accounts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|x| x["usable"] == true)
                        .map(|x| Account { id: x["id"].as_str().unwrap_or("").into(), name: x["name"].as_str().unwrap_or("").into(), from: x["from"].as_str().unwrap_or("").into(), default: x["default"] == true })
                        .collect();
                    let mine = al.ok().and_then(|v| v["mine"].as_array().map(|m| m.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default();
                    cx.meta.set(Some(Meta { accounts, mine, llm: a["llm"] == true, interval: a["interval"].as_i64().unwrap_or(0), last_run: a["last_run"].as_i64().unwrap_or(0) }));
                }
                Err(e) => meta_err.set(e.message),
            }
            if let Ok(s) = api_get::<Value>("/summary").await {
                let b: Vec<(String, String)> = s["last_run"]["accounts"].as_object().into_iter().flatten().filter(|(_, v)| !v["error"].is_null() && v["error"] != false && v["error"] != "").map(|(k, v)| (k.clone(), v["error"].as_str().map(str::to_string).unwrap_or_else(|| v["error"].to_string()))).collect();
                broken.set(b);
            }
        });
    };
    load_meta();

    // Live: the server says mail changed (a sync, an action elsewhere) → reload what's on screen.
    subscribe(&["app"]);
    let ev_timer = StoredValue::new(None::<SendWrapper<gloo_timers::callback::Timeout>>);
    on_ws(move |m| {
        if let WsOut::App { payload } = m {
            if payload["kind"] != "mail" {
                return;
            }
            let t = gloo_timers::callback::Timeout::new(600, move || {
                if hushed() {
                    cx.refresh_counts();
                } else {
                    cx.ver.update(|v| *v += 1);
                    load_meta();
                }
            });
            ev_timer.set_value(Some(SendWrapper::new(t)));
        }
    });

    // The inbox list (and "Expects a reply" chips at most once a minute).
    let seq = StoredValue::new(0u32);
    let reply_at = StoredValue::new(0.0f64);
    Effect::new(move |_| {
        if tab.get() != "inbox" {
            return;
        }
        let f = filter.get();
        cx.ver.track();
        let mut q = format!("limit=200&folder={}", enc(&f.folder));
        if !f.cat.is_empty() {
            q += &format!("&category={}", enc(&f.cat));
        }
        if !f.account.is_empty() {
            q += &format!("&account={}", enc(&f.account));
        }
        if !f.q.is_empty() {
            q += &format!("&q={}", enc(&f.q));
        }
        if f.unread {
            q += "&unread=1";
        }
        loading.set(true);
        seq.update_value(|n| *n += 1);
        let me = seq.get_value();
        spawn_local(async move {
            let r = api_get::<Value>(&format!("/mail/inbox?{q}")).await;
            if seq.try_get_value() != Some(me) {
                return;
            }
            match r {
                Ok(v) => {
                    msgs.set(Some(Msg::list(&v["messages"])));
                    list_err.set(String::new());
                }
                Err(e) => list_err.set(e.message),
            }
            loading.set(false);
        });
        if js_sys::Date::now() - reply_at.get_value() > 60_000.0 {
            reply_at.set_value(js_sys::Date::now());
            spawn_local(async move {
                if let Ok(rz) = api_get::<Value>("/mail/reply-zero").await {
                    cx.reply_ids.set(Msg::list(&rz["to_reply"]).into_iter().map(|m| m.msgid).collect());
                }
            });
        }
        cx.refresh_counts();
    });

    let set_tab = move |t: &str| {
        tab.set(t.to_string());
        cx.sel.set(BTreeSet::new());
        cx.focus.set(None);
        cx.reading.set(None);
        keys.set(false);
        set_param("tab", if t == "inbox" { None } else { Some(t) });
    };
    let change_filter = Callback::new(move |f: Filter| {
        filter.set(f);
        cx.sel.set(BTreeSet::new());
        if tab.get_untracked() != "inbox" {
            set_tab("inbox");
        }
    });

    // Deep links: ?tab=reply, ?msg=<acct>:<uid>&folder=INBOX[&reply=1]. A message link is consumed
    // (removed from the address) so the same link works again.
    let params = use_params();
    Effect::new(move |_| {
        let p = params.get();
        let t = tab_of(p.get("tab").cloned());
        if let Some(t) = &t {
            if *t != tab.get_untracked() {
                set_tab(t);
            }
        }
        if let Some((acct, uid)) = p.get("msg").and_then(|m| m.split_once(':')) {
            if tab.get_untracked() != "inbox" && t.is_none() {
                set_tab("inbox");
            }
            let item = Item { account: acct.to_string(), uid: uid.parse().unwrap_or(0), folder: p.get("folder").cloned().unwrap_or_else(|| "INBOX".into()) };
            let then = (p.get("reply").map(String::as_str) == Some("1")).then_some("ai");
            cx.open(Reading { item, thread: None, set: msgs, then });
            set_param("msg", None);
            set_param("folder", None);
            set_param("reply", None);
        }
    });

    let sync_now = move |_| {
        syncing.set(true);
        spawn_local(async move {
            match api_post::<Value>("/mail/run", &json!({})).await {
                Ok(_) => {
                    load_meta();
                    cx.ver.update(|v| *v += 1);
                    toast("Synced");
                }
                Err(e) => toast_bad(e.message),
            }
            syncing.set(false);
        });
    };
    let toggle_bell = move |_| {
        spawn_local(async move {
            if push_on.get_untracked() || (notify.get_untracked() && !push::pushable()) {
                push::disable_push().await;
                push_on.set(false);
                notify.set(false);
                toast("Mail notifications off on this device");
                return;
            }
            agentum_ui::store::set(push::NOTIFY_KEY, "on");
            notify.set(true);
            match push::enable_push().await {
                Ok(()) => {
                    push_on.set(true);
                    toast("Mail notifications on for this device, even with Agentum closed");
                }
                Err(e) => toast_bad(format!("{e} New mail still shows here while Agentum is open.")),
            }
        });
    };

    // ---- Keyboard: only when Mail has the focus (or nothing else does), never while typing.
    let g_at = StoredValue::new(0.0f64);
    let key_handle = window_event_listener(leptos::ev::keydown, move |e| {
        let Some(el) = root.get_untracked() else { return };
        if e.meta_key() || e.ctrl_key() || e.alt_key() || e.default_prevented() {
            return;
        }
        let doc = web_sys::window().and_then(|w| w.document());
        if doc.as_ref().and_then(|d| d.query_selector(".dialog-bg").ok().flatten()).is_some() {
            return;
        }
        let ae = doc.as_ref().and_then(|d| d.active_element());
        let body = doc.as_ref().and_then(|d| d.body()).map(|b| b.unchecked_into::<web_sys::Element>());
        if let Some(a) = &ae {
            if Some(a) != body.as_ref() && !el.contains(Some(a)) {
                return;
            }
        }
        let typing = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).is_some_and(|t| t.closest("input, textarea, select, [contenteditable]").ok().flatten().is_some());
        let top = cx.top();
        let k = e.key();
        let done = || {
            e.prevent_default();
            e.stop_propagation();
        };
        if k == "Escape" {
            if let Some(t) = &top {
                t.esc.run(());
                done();
            } else if typing {
                if let Some(h) = ae.and_then(|a| a.dyn_into::<web_sys::HtmlElement>().ok()) {
                    let _ = h.blur();
                }
            }
            return;
        }
        if typing {
            return;
        }
        let now = js_sys::Date::now();
        if g_at.get_value() > 0.0 && now - g_at.get_value() < 1200.0 {
            g_at.set_value(0.0);
            let t = match k.as_str() {
                "i" => "inbox",
                "r" => "reply",
                "c" => "clean",
                "s" => "stats",
                "l" => "rules",
                _ => "",
            };
            if !t.is_empty() {
                set_tab(t);
                done();
            }
            return;
        }
        match k.as_str() {
            "g" => return g_at.set_value(now),
            "?" => {
                keys.set(true);
                return done();
            }
            "c" => {
                cx.compose(Draft::default());
                return done();
            }
            "/" => {
                if let Some(s) = el.query_selector(".ml-search").ok().flatten().and_then(|s| s.dyn_into::<web_sys::HtmlElement>().ok()) {
                    let _ = s.focus();
                    done();
                }
                return;
            }
            _ => {}
        }
        let nav = matches!(k.as_str(), "j" | "k" | "ArrowDown" | "ArrowUp");
        // With the reader beside the list, j/k walk the list and the reader follows.
        let side_reader = top.as_ref().is_some_and(|t| t.kind == "reader") && tab.get_untracked() == "inbox" && layout.get_untracked() != "narrow";
        if let Some(t) = &top {
            if !(nav && side_reader) {
                if t.keys.is_some_and(|f| f.run(k.clone())) {
                    done();
                }
                return;
            }
        }
        let rows = cx.rows.get_value();
        if rows.is_empty() {
            return;
        }
        let focus = cx.focus.get_untracked();
        let i = rows.iter().position(|(t, _)| Some(&t.key) == focus.as_ref());
        if nav {
            let d: i64 = if k == "j" || k == "ArrowDown" { 1 } else { -1 };
            let j = (i.map(|i| i as i64).unwrap_or(-1) + d).clamp(0, rows.len() as i64 - 1) as usize;
            let (t, set) = rows[j].clone();
            cx.focus.set(Some(t.key.clone()));
            if side_reader && Some(&t.key) != focus.as_ref() {
                cx.open(Reading { item: t.item(), thread: Some(t), set, then: None });
            }
            return done();
        }
        let Some(i) = i else { return };
        let (cur, set) = rows[i].clone();
        let act = match k.as_str() {
            "e" => Some("archive"),
            "!" => Some("spam"),
            "s" => Some(if cur.m.starred { "unstar" } else { "star" }),
            "u" => Some(if cur.m.unread { "mark_read" } else { "mark_unread" }),
            _ => None,
        };
        if let Some(a) = act {
            cx.run_now(vec![cur], a, set);
            return done();
        }
        match k.as_str() {
            "#" => {
                done();
                let subj = if cur.m.subject.is_empty() { "(no subject)".to_string() } else { cur.m.subject.clone() };
                spawn_local(async move {
                    if confirm("Delete this email?", subj, "Delete", true).await {
                        cx.run(vec![cur], "delete", set, false).await;
                    }
                });
            }
            "x" => {
                cx.toggle_sel(&cur.key, None);
                done();
            }
            "Enter" | "o" => {
                cx.open(Reading { item: cur.item(), thread: Some(cur), set, then: None });
                done();
            }
            "r" | "a" | "f" => {
                let then = match k.as_str() {
                    "r" => "reply",
                    "a" => "all",
                    _ => "forward",
                };
                cx.open(Reading { item: cur.item(), thread: Some(cur), set, then: Some(then) });
                done();
            }
            _ => {}
        }
    });
    on_cleanup(move || key_handle.remove());

    let focus_root = move || {
        if let Some(r) = root.get_untracked() {
            let o = web_sys::FocusOptions::new();
            o.set_prevent_scroll(true);
            let _ = r.focus_with_options(&o);
        }
    };
    let close_reader = Callback::new(move |_| {
        cx.reading.set(None);
        cx.front.set("compose");
        focus_root();
    });
    let close_draft = Callback::new(move |_| {
        cx.draft.set(None);
        cx.front.set("reader");
        focus_root();
    });

    // The New / sync / bell / keys buttons go in the pane's header.
    let tools_el = RwSignal::new(None::<SendWrapper<web_sys::Element>>);
    Effect::new(move |_| {
        let id = if side == Pane::Left { "pane-tools-left" } else { "pane-tools-right" };
        tools_el.set(web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id)).map(SendWrapper::new));
    });
    let bell_title = move || {
        if push_on.get() {
            "Mail notifications on for this device (even with Agentum closed). Tap to turn off."
        } else if notify.get() {
            "New mail shows here while Agentum is open. Tap for notifications on this device."
        } else {
            "Turn on mail notifications for this device"
        }
    };
    let tools = move || {
        tools_el.get().map(|el| {
            let mount: web_sys::Element = (*el).clone();
            view! {
                <leptos::portal::Portal mount=mount>
                    <span class="ml-tools row" style="gap:4px">
                        <button type="button" class="btn primary" title="New email (c)" on:click=move |_| cx.compose(Draft::default())><Icon name="edit" size=14/>"New"</button>
                        {move || {
                            tick.track();
                            cx.meta.with(|m| m.as_ref().map(|m| m.last_run).filter(|t| *t > 0)).map(|t| view! { <span class="subdue small ml-synced" title="Last sync">{format!("synced {} ago", ago(t))}</span> })
                        }}
                        <button type="button" class="icon-btn" title="Sync now" aria-label="Sync now" disabled=move || syncing.get() on:click=sync_now>
                            <Icon name="refresh" class=Signal::derive(move || if syncing.get() { "spin".to_string() } else { String::new() })/>
                        </button>
                        <button type="button" class=move || if push_on.get() { "icon-btn on" } else { "icon-btn" } title=bell_title aria-label="Notifications" on:click=toggle_bell><Icon name="bell"/></button>
                        <button type="button" class="icon-btn" title="Keyboard shortcuts (?)" aria-label="Keyboard shortcuts" on:click=move |_| keys.set(true)><Icon name="keyboard"/></button>
                    </span>
                </leptos::portal::Portal>
            }
        })
    };

    let pane_open = move || cx.reading.with(Option::is_some) || cx.draft.with(Option::is_some);
    view! {
        <div node_ref=root tabindex="-1"
            class=move || format!("ml ml-{}{}{}", layout.get(), if tab.get() == "inbox" { " ml-in" } else { "" }, if pane_open() { " ml-open" } else { "" })>
            {tools}
            <nav class="ml-tabs" role="tablist">
                {TABS.iter().map(|(k, l)| {
                    let k = *k;
                    view! { <button type="button" role="tab" aria-selected=move || (tab.get() == k).to_string() class=move || if tab.get() == k { "on" } else { "" } on:click=move |_| set_tab(k)>{*l}</button> }
                }).collect_view()}
            </nav>
            <Show when=move || !broken.with(Vec::is_empty)>
                <div class="ml-warn" role="status">
                    <Icon name="alert" size=15/>
                    <span class="grow">{move || {
                        let b = broken.get();
                        let names: Vec<String> = b.iter().map(|(id, _)| cx.meta.with(|m| m.as_ref().and_then(|m| m.accounts.iter().find(|a| &a.id == id).map(|a| a.name.clone()))).unwrap_or_else(|| id.clone())).collect();
                        let many = b.len() > 1;
                        let google = b.iter().any(|(_, e)| e.contains("invalid_grant") || e.to_lowercase().contains("token"));
                        format!("{} {} syncing{}. New mail, and the replies you send from {}, don’t show here until then.", names.join(" and "), if many { "aren't" } else { "isn't" }, if google { ": Google wants you to sign in again" } else { "" }, if many { "them" } else { "it" })
                    }}</span>
                    <button type="button" class="btn" on:click=move |_| navigate("settings", Where::Replace, &[("section", "accounts")])>"Reconnect"</button>
                </div>
            </Show>
            <div class="ml-main">
                {move || {
                    if !meta_err.get().is_empty() && cx.meta.with(Option::is_none) {
                        return view! { <div class="empty err">{meta_err.get()}</div> }.into_any();
                    }
                    match tab.get().as_str() {
                        "inbox" => view! {
                            <Show when=move || layout.get() == "wide"><inbox::Nav f=filter set_f=change_filter/></Show>
                            <div class="ml-list"><inbox::Inbox msgs=msgs f=filter set_f=change_filter with_nav=Signal::derive(move || layout.get() == "wide") loading=loading error=list_err/></div>
                            <Show when=move || layout.get() != "narrow" && !pane_open()>
                                <div class="ml-pane ml-empty"><Icon name="mail" size=34/><div>"No email open"</div><div class="subdue">"Pick one from the list, or press c to write one."</div></div>
                            </Show>
                        }.into_any(),
                        t => {
                            let t = t.to_string();
                            view! { <div class="ml-body">{match t.as_str() {
                                "reply" => view! { <tabs::ReplyZero/> }.into_any(),
                                "rules" => view! { <rules::Rules/> }.into_any(),
                                "clean" => view! { <tabs::Clean/> }.into_any(),
                                "cold" => view! { <tabs::Cold/> }.into_any(),
                                _ => view! { <tabs::Analytics/> }.into_any(),
                            }}</div> }.into_any()
                        }
                    }
                }}
                {move || cx.reading.get().map(|r| {
                    let key = r.item.key();
                    view! {
                        <div class=move || if cx.front.get() == "reader" { "ml-pane ml-front" } else { "ml-pane" } data-key=key>
                            <reader::Reader r=r on_close=close_reader/>
                        </div>
                    }
                })}
                {move || cx.draft.with(|d| d.as_ref().map(|d| d.id)).map(|_| {
                    let d = cx.draft.get_untracked().unwrap_or_default();
                    view! {
                        <div class=move || if cx.front.get() == "compose" { "ml-pane ml-front" } else { "ml-pane" }>
                            <composer::Composer d=d on_close=close_draft/>
                        </div>
                    }
                })}
                <Show when=move || keys.get()><KeysHelp on_close=Callback::new(move |_| keys.set(false))/></Show>
            </div>
            {move || cx.undo.get().map(|(text, _)| view! {
                <div class="ml-undo">
                    <span>{text}</span>
                    <button type="button" class="btn" on:click=move |_| {
                        if let Some(f) = cx.undo_fn.get_value() { (f.as_ref())() }
                        cx.undo.set(None);
                    }><Icon name="undo" size=13/>"Undo"</button>
                </div>
            })}
        </div>
    }
}
