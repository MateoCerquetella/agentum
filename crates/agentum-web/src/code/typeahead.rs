//! The composer's menus, as BB's: `/` at the start lists the agent's slash commands and skills,
//! `@` lists the other threads (sessions) and the project's files. ↑↓ to move, ⏎ or ⇥ to insert,
//! Esc to close.
//!
//! A thread goes in as `[@Its title](/code?thread=<id>)`: the hub gives the agent that thread's
//! recent conversation with the message. A file goes in as `@path/in/the/project`.
use super::store;
use agentum_proto::api::ThreadSummary;
use agentum_proto::machine::{CommandInfo, CommandKind};
use agentum_proto::AgentKind;
use agentum_ui::prelude::*;

/// Where the menus look.
#[derive(Clone, PartialEq, Debug)]
pub enum Src {
    /// A thread: its agent, machine and directory.
    Thread(String),
    /// A new thread: no thread yet.
    New { machine: String, agent: AgentKind, cwd: String },
}

impl Src {
    fn machine_cwd(&self) -> Option<(String, String)> {
        match self {
            Src::Thread(id) => store::overview().with_untracked(|o| o.as_ref().and_then(|o| o.threads.iter().find(|t| &t.id == id)).map(|t| (t.machine.clone(), t.cwd.clone()))),
            Src::New { machine, cwd, .. } => Some((machine.clone(), cwd.clone())),
        }
    }
    fn commands_url(&self) -> String {
        match self {
            Src::Thread(id) => format!("/code/commands?thread={id}"),
            Src::New { machine, agent, cwd } => format!("/code/commands?machine={}&agent={}&cwd={}", enc(machine), agent.id(), enc(cwd)),
        }
    }
    fn thread(&self) -> Option<&str> {
        match self {
            Src::Thread(id) => Some(id),
            Src::New { .. } => None,
        }
    }
}

fn enc(s: &str) -> String {
    js_sys::encode_uri_component(s).as_string().unwrap_or_default()
}

/// `/` or `@`, where the token starts (a byte index in the text) and what follows the sign.
#[derive(Clone, PartialEq, Debug)]
struct Trigger {
    sign: char,
    start: usize,
    end: usize,
    query: String,
}

/// The token being typed at `caret` (a byte index), if it opens a menu: `/x` as the message's
/// first word, `@x` anywhere after a space.
fn trigger(text: &str, caret: usize) -> Option<Trigger> {
    let before = text.get(..caret)?;
    let start = before.rfind(char::is_whitespace).map(|i| i + before[i..].chars().next().map_or(1, char::len_utf8)).unwrap_or(0);
    let tok = &before[start..];
    let sign = tok.chars().next()?;
    let query = tok[1..].to_string();
    match sign {
        '/' if before[..start].trim().is_empty() && !query.contains('/') => Some(Trigger { sign, start, end: caret, query }),
        '@' if !query.contains(['[', ']', '(', ')']) => Some(Trigger { sign, start, end: caret, query }),
        _ => None,
    }
}

/// A UTF-16 offset (the textarea's caret) as a byte index of `s`.
fn byte_at(s: &str, u16_off: u32) -> usize {
    let mut n = 0u32;
    for (i, c) in s.char_indices() {
        if n >= u16_off {
            return i;
        }
        n += c.len_utf16() as u32;
    }
    s.len()
}
fn u16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

#[derive(Clone, PartialEq)]
struct Hit {
    insert: String,
    label: String,
    sub: String,
    tag: String,
    icon: &'static str,
}

/// Subsequence match, tighter and earlier is better (None: no match).
fn score(hay: &str, q: &str) -> Option<i64> {
    if q.is_empty() {
        return Some(0);
    }
    let h = hay.to_lowercase();
    let q = q.to_lowercase();
    if let Some(i) = h.find(&q) {
        return Some(1000 - i as i64 * 3 - h.len() as i64);
    }
    let mut it = h.chars();
    q.chars().all(|c| it.any(|x| x == c)).then(|| 100 - h.len() as i64)
}

fn command_hits(cmds: &[CommandInfo], q: &str) -> Vec<Hit> {
    let mut v: Vec<(i64, usize, &CommandInfo)> = cmds.iter().enumerate().filter_map(|(i, c)| score(&c.name, q).map(|s| (s, i, c))).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    v.into_iter()
        .take(50)
        .map(|(_, _, c)| Hit {
            insert: format!("/{} ", c.name),
            label: format!("/{}{}", c.name, c.hint.as_deref().map(|h| format!(" {h}")).unwrap_or_default()),
            sub: c.description.clone().unwrap_or_default(),
            tag: match c.kind {
                CommandKind::Skill => "skill".into(),
                CommandKind::Builtin => "built-in".into(),
                CommandKind::Command => c.origin.clone(),
            },
            icon: match c.kind {
                CommandKind::Skill => "sparkle",
                _ => "terminal",
            },
        })
        .collect()
}

fn thread_hits(threads: &[ThreadSummary], me: Option<&str>, q: &str) -> Vec<Hit> {
    let mut v: Vec<(i64, &ThreadSummary)> = threads.iter().filter(|t| Some(t.id.as_str()) != me && !t.title.is_empty()).filter_map(|t| score(&t.title, q).map(|s| (s, t))).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.updated_at.cmp(&a.1.updated_at)));
    v.into_iter()
        .take(6)
        .map(|(_, t)| {
            let title: String = t.title.chars().filter(|c| !matches!(c, '[' | ']' | '(' | ')' | '\n')).collect();
            Hit {
                insert: format!("[@{}](/code?thread={}) ", title.trim(), t.id),
                label: t.title.clone(),
                sub: format!("{} · {}", t.project.trim_end_matches('/').rsplit('/').next().unwrap_or(""), t.agent.label()),
                tag: "thread".into(),
                icon: "chat",
            }
        })
        .collect()
}

/// The menu's state; `on_key` goes first in the textarea's keydown.
#[derive(Clone, Copy)]
pub struct Typeahead {
    text: RwSignal<String>,
    area: NodeRef<leptos::html::Textarea>,
    src: Signal<Option<Src>>,
    trig: RwSignal<Option<Trigger>>,
    /// Closed with Esc, until the caret leaves that token.
    dismissed: RwSignal<Option<usize>>,
    sel: RwSignal<usize>,
    cmds: RwSignal<Option<(Src, Vec<CommandInfo>)>>,
    files: RwSignal<Vec<String>>,
    asked: StoredValue<u32>,
    /// After an insert: the draft to save.
    on_change: Callback<String>,
}

impl Typeahead {
    pub fn new(text: RwSignal<String>, area: NodeRef<leptos::html::Textarea>, src: Signal<Option<Src>>, on_change: Callback<String>) -> Typeahead {
        let t = Typeahead {
            text,
            area,
            src,
            trig: RwSignal::new(None),
            dismissed: RwSignal::new(None),
            sel: RwSignal::new(0),
            cmds: RwSignal::new(None),
            files: RwSignal::new(vec![]),
            asked: StoredValue::new(0),
            on_change,
        };
        // Fetch what the open menu needs.
        Effect::new(move |_| {
            let (Some(tr), Some(src)) = (t.trig.get(), t.src.get()) else { return };
            if tr.sign == '/' {
                if t.cmds.with_untracked(|c| c.as_ref().is_some_and(|(s, _)| *s == src)) {
                    return;
                }
                t.cmds.set(Some((src.clone(), vec![])));
                spawn_local(async move {
                    match api_get::<Vec<CommandInfo>>(&src.commands_url()).await {
                        Ok(v) => t.cmds.set(Some((src, v))),
                        Err(e) => {
                            t.cmds.set(None);
                            toast_bad(e.message);
                        }
                    }
                });
            } else if let Some((machine, cwd)) = src.machine_cwd().filter(|(_, c)| !c.is_empty()) {
                let n = t.asked.get_value() + 1;
                t.asked.set_value(n);
                let q = tr.query.clone();
                spawn_local(async move {
                    gloo_timers::future::TimeoutFuture::new(if q.is_empty() { 0 } else { 120 }).await;
                    if t.asked.get_value() != n {
                        return;
                    }
                    let req = agentum_proto::machine::Req::Fs(agentum_proto::machine::FsReq::Find { root: cwd, query: q, limit: Some(8) });
                    let body = serde_json::json!({"machine": machine, "req": req});
                    let paths = api_post::<Value>("/code/call", &body).await.ok().and_then(|v| v.get("paths").cloned()).and_then(|p| serde_json::from_value::<Vec<String>>(p).ok()).unwrap_or_default();
                    if t.asked.get_value() == n {
                        t.files.set(paths);
                    }
                });
            }
        });
        t
    }

    fn hits(&self) -> Vec<Hit> {
        let Some(tr) = self.trig.get() else { return vec![] };
        if self.dismissed.get() == Some(tr.start) {
            return vec![];
        }
        match tr.sign {
            '/' => self.cmds.with(|c| c.as_ref().map(|(_, v)| command_hits(v, &tr.query)).unwrap_or_default()),
            _ => {
                let me = self.src.with(|s| s.as_ref().and_then(|s| s.thread().map(str::to_string)));
                let mut v = store::overview().with(|o| o.as_ref().map(|o| thread_hits(&o.threads, me.as_deref(), &tr.query)).unwrap_or_default());
                v.extend(self.files.get().into_iter().map(|p| Hit {
                    insert: format!("@{p} "),
                    sub: String::new(),
                    label: p,
                    tag: "file".into(),
                    icon: "file",
                }));
                v
            }
        }
    }

    /// Read the token at the caret (after an input, a click, a caret move).
    pub fn sync(&self) {
        let Some(a) = self.area.get_untracked() else { return };
        let v = a.value();
        let caret = a.selection_start().ok().flatten().unwrap_or(u16_len(&v));
        let tr = trigger(&v, byte_at(&v, caret));
        if tr.as_ref().map(|t| t.start) != self.trig.with_untracked(|t| t.as_ref().map(|t| t.start)) {
            self.dismissed.set(None);
            self.sel.set(0);
        }
        if tr != self.trig.get_untracked() {
            if tr.as_ref().is_some_and(|t| t.sign == '@') && self.trig.with_untracked(|o| o.as_ref().map(|o| &o.query) != tr.as_ref().map(|t| &t.query)) {
                self.sel.set(0);
            }
            self.trig.set(tr);
        }
    }

    fn open(&self) -> bool {
        !self.hits().is_empty()
    }

    fn insert(&self, h: &Hit) {
        let Some(tr) = self.trig.get_untracked() else { return };
        let t = self.text.get_untracked();
        let end = tr.end.min(t.len());
        let new = format!("{}{}{}", &t[..tr.start], h.insert, &t[end..]);
        let caret = u16_len(&t[..tr.start]) + u16_len(&h.insert);
        self.text.set(new.clone());
        self.on_change.run(new.clone());
        self.trig.set(None);
        let area = self.area;
        if let Some(a) = area.get_untracked() {
            a.set_value(&new);
            let _ = a.focus();
            let _ = a.set_selection_range(caret, caret);
        }
    }

    /// The textarea's keydown: true when the menu took the key.
    pub fn on_key(&self, e: &web_sys::KeyboardEvent) -> bool {
        if e.is_composing() || !self.open() {
            return false;
        }
        let n = self.hits().len();
        match e.key().as_str() {
            "ArrowDown" => self.sel.update(|s| *s = (*s + 1) % n),
            "ArrowUp" => self.sel.update(|s| *s = (*s + n - 1) % n),
            "Enter" | "Tab" if !e.shift_key() => {
                let h = self.hits().get(self.sel.get_untracked().min(n - 1)).cloned();
                if let Some(h) = h {
                    self.insert(&h);
                }
            }
            "Escape" => self.dismissed.set(self.trig.get_untracked().map(|t| t.start)),
            _ => return false,
        }
        e.prevent_default();
        e.stop_propagation();
        true
    }

    pub fn view(self) -> impl IntoView {
        let me = self;
        view! {
            <Show when=move || me.open()>
                <div class="pk-pop up ta-pop" role="listbox" on:mousedown=|e| e.prevent_default()>
                    <div class="mp-label">{move || match me.trig.get().map(|t| t.sign) { Some('/') => "Commands and skills", _ => "Threads and files" }}</div>
                    <div class="ta-list">
                        {move || {
                            let sel = me.sel.get();
                            me.hits().into_iter().enumerate().map(|(i, h)| {
                                let h2 = h.clone();
                                view! {
                                    <button type="button" class=if i == sel { "pk-item ta-item on" } else { "pk-item ta-item" } role="option" aria-selected=(i == sel).to_string()
                                        on:mouseenter=move |_| me.sel.set(i) on:click=move |_| me.insert(&h2)>
                                        <Icon name=h.icon size=13/>
                                        <span class="ta-name">{h.label.clone()}</span>
                                        <span class="pk-sub ta-sub">{h.sub.clone()}</span>
                                        <span class="ta-tag">{h.tag.clone()}</span>
                                    </button>
                                }
                            }).collect_view()
                        }}
                    </div>
                </div>
            </Show>
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_token() {
        assert_eq!(trigger("/rev", 4).map(|t| (t.sign, t.start, t.query)), Some(('/', 0, "rev".into())));
        assert_eq!(trigger("  /", 3).map(|t| t.start), Some(2));
        assert_eq!(trigger("hi /rev", 7), None);
        assert_eq!(trigger("see @lo", 7).map(|t| (t.sign, t.start, t.query)), Some(('@', 4, "lo".into())));
        assert_eq!(trigger("mail@x", 6), None);
        assert_eq!(trigger("ñ @a", byte_at("ñ @a", 4)).map(|t| t.query), Some("a".into()));
        assert_eq!(trigger("[@T](/code?thread=a) ", 21), None);
    }

    #[test]
    fn ranks() {
        assert!(score("review", "rev").unwrap() > score("pr-review", "rev").unwrap());
        assert!(score("security-review", "srv").is_some());
        assert!(score("init", "x").is_none());
    }
}
