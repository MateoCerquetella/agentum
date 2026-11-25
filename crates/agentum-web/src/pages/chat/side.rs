//! The Secretary's earlier chats: the list (shared by the sidebar's Personal side and the chat
//! page), and rename / pin / delete.
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Session {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub count: i64,
}
impl Session {
    pub fn title(&self) -> String {
        self.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Chat".into())
    }
    /// When it last changed, in ms (the hub's SQLite time is UTC).
    pub fn at(&self) -> i64 {
        self.updated_at.as_deref().map(|s| js_sys::Date::parse(&format!("{}Z", s.replace(' ', "T"))) as i64).filter(|t| *t > 0).unwrap_or(0)
    }
}

thread_local! {
    static SESSIONS: RwSignal<Option<Vec<Session>>> = agentum_ui::global(None);
    static LOADED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Every chat, pinned first then the latest (loaded once, then on [`reload`]).
pub fn sessions() -> RwSignal<Option<Vec<Session>>> {
    let s = SESSIONS.with(|s| *s);
    if !LOADED.with(|l| l.replace(true)) {
        reload();
    }
    s
}

pub fn reload() {
    let s = SESSIONS.with(|s| *s);
    spawn_local(async move {
        if let Ok(v) = api_get::<Vec<Session>>("/chat/sessions").await {
            s.set(Some(v));
        }
    });
}

/// Reload the list when the hub says a chat changed (while the calling view lives).
pub fn follow_changes() {
    subscribe(&["app"]);
    on_ws(|m| {
        if let agentum_proto::api::WsOut::App { payload } = m {
            if payload["kind"] == "chat" {
                reload();
            }
        }
    });
}

fn patch(id: &str, body: Value, f: impl Fn(&mut Session) + 'static) {
    let id = id.to_string();
    sessions().update(|v| {
        if let Some(s) = v.as_mut().and_then(|v| v.iter_mut().find(|s| s.id == id)) {
            f(s)
        }
    });
    spawn_local(async move {
        if let Err(e) = api_put::<Value>(&format!("/chat/sessions/{id}"), &body).await {
            toast_bad(e.message);
        }
        reload();
    });
}

pub fn rename(id: &str, name: &str) {
    let n = name.trim().to_string();
    if n.is_empty() {
        return;
    }
    let n2 = n.clone();
    patch(id, json!({"name": n}), move |s| s.name = Some(n2.clone()));
}

pub fn pin(id: &str, on: bool) {
    patch(id, json!({"pinned": on}), move |s| s.pinned = on);
}

/// Delete after asking; a chat that is open leaves the page for a new one.
pub fn delete(s: &Session) {
    let (id, title) = (s.id.clone(), s.title());
    spawn_local(async move {
        if !confirm("Delete this chat?", title, "Delete", true).await {
            return;
        }
        match api_delete::<Value>(&format!("/chat/sessions/{id}")).await {
            Ok(_) => {
                sessions().update(|v| {
                    if let Some(v) = v.as_mut() {
                        v.retain(|x| x.id != id)
                    }
                });
                if use_param("chat").get_untracked().as_deref() == Some(id.as_str()) {
                    navigate("chat", Where::Replace, &[("chat", "")]);
                }
            }
            Err(e) => toast_bad(e.message),
        }
    });
}

pub fn meta(s: &Session) -> String {
    if s.count == 1 { "1 message".into() } else { format!("{} messages", s.count) }
}

/// The Personal side's list of chats with the Secretary (under its card in the sidebar).
#[component]
pub fn PersonalChats() -> impl IntoView {
    let all = sessions();
    follow_changes();
    let on = use_param("chat");
    let find = RwSignal::new(None::<String>);
    // Folded unless you open it (remembered on this device).
    let open = RwSignal::new(agentum_ui::store::flag("agentum.chats.open"));
    let delete_all = move || spawn_local(async move {
        let n = all.with_untracked(|v| v.as_ref().map(|v| v.len()).unwrap_or(0));
        if n == 0 || !confirm(format!("Delete all {n} chats?"), "Every chat with the Secretary and its messages. This can't be undone.", "Delete all", true).await {
            return;
        }
        match api_delete::<Value>("/chat/sessions").await {
            Ok(_) => {
                all.set(Some(vec![]));
                if use_param("chat").get_untracked().is_some_and(|c| !c.is_empty()) {
                    navigate("chat", Where::Replace, &[("chat", "")]);
                }
                toast("All chats deleted.");
            }
            Err(e) => toast_bad(e.message),
        }
    });
    let shown = move || {
        let q = find.get().unwrap_or_default().trim().to_lowercase();
        all.get().unwrap_or_default().into_iter().filter(|s| !s.archived && (q.is_empty() || s.title().to_lowercase().contains(&q))).take(40).collect::<Vec<_>>()
    };
    view! {
        <div class="dock-chats-head">
            <button class="chats-fold" aria-expanded=move || open.get().to_string() title=move || if open.get() { "Hide the chats" } else { "Show the chats" }
                on:click=move |_| { open.update(|o| *o = !*o); agentum_ui::store::set("agentum.chats.open", if open.get_untracked() { "1" } else { "" }) }>
                <span>"Chats"</span><span class="dock-count">{move || all.with(|v| v.as_ref().map(|v| v.iter().filter(|s| !s.archived).count()).unwrap_or(0))}</span>
                <Icon name="chevron" size=12 class=Signal::derive(move || if open.get() { "dk-chev open".to_string() } else { "dk-chev".to_string() })/>
            </button>
            <span class="grow"/>
            <Show when=move || open.get()>
                <button class=move || if find.get().is_some() { "icon-btn dock-btn on" } else { "icon-btn dock-btn" } title="Find a chat" aria-pressed=move || find.get().is_some().to_string()
                    on:click=move |_| find.update(|f| *f = if f.is_some() { None } else { Some(String::new()) })><Icon name="search" size=13/></button>
                <agentum_ui::picker::Menu items=Signal::derive(move || vec![PaneAction::new("trash", "Delete all chats", move || delete_all()).danger()]) title="More"/>
            </Show>
        </div>
        <Show when=move || open.get()>
        <Show when=move || find.get().is_some()>
            <input class="dock-find" autofocus placeholder="Find a chat" aria-label="Find a chat" prop:value=move || find.get().unwrap_or_default()
                on:input=move |e| find.set(Some(event_target_value(&e))) on:keydown=move |e| if e.key() == "Escape" { find.set(None) }/>
        </Show>
        <div class="ag-side-chats">
            {move || {
                let v = shown();
                if v.is_empty() {
                    let msg = if all.with(|a| a.is_none()) { "Loading chats…" } else if find.get().is_some() { "Nothing matches." } else { "No chats yet. Start one with +." };
                    return view! { <p class="dock-empty">{msg}</p> }.into_any();
                }
                v.into_iter().map(|s| {
                    let (id, s2, s3) = (s.id.clone(), s.clone(), s.clone());
                    let id2 = id.clone();
                    let is_on = move || on.get().as_deref() == Some(id2.as_str());
                    let at = s.at();
                    let fresh = now_ms() - at < 3_600_000;
                    view! {
                        <div class=move || format!("dock-row{}", if is_on() { " on" } else { "" })>
                            <button class="dock-row-hit" title=s.title() on:click=move |e| navigate("chat", if e.alt_key() || e.meta_key() { Where::Right } else { Where::Replace }, &[("chat", &id)])/>
                            <span class="dock-row-1"><span class="dock-title">{s.title()}</span><span class="dock-time">{ago_ms(at)}</span>
                                <button class="icon-btn dock-act" title=if s.pinned { "Unpin" } else { "Pin" } aria-label=if s.pinned { "Unpin" } else { "Pin" } on:click=move |_| pin(&s2.id, !s2.pinned)><Icon name="pin" size=12/></button>
                                <button class="icon-btn dock-act" title="Delete" aria-label="Delete" on:click=move |_| delete(&s3)><Icon name="x" size=12/></button>
                            </span>
                            <span class="dock-row-2"><span class="dock-meta">{s.pinned.then(|| view! { <Icon name="pin" size=10/> })}{meta(&s)}</span>
                                <span class=if fresh { "dock-mark dock-mark-unread ag-chat-mark" } else { "dock-mark dock-mark-inactive" }><Icon name="lock" size=12/></span>
                            </span>
                        </div>
                    }
                }).collect_view().into_any()
            }}
        </div>
        </Show>
    }
}
