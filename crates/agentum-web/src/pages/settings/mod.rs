//! Settings: the tabs on the left (`?tab=…`, so the palette can open Plugins or Machines), the
//! tab's body on the right. With a passkey set up, Settings opens with Touch ID / Face ID (or the
//! password) for 10 minutes, as the hub asks before it changes anything.
mod accounts;
mod fields;
mod machines;
mod passkey;
mod plugins;
mod prefs;

use super::Pane;
use agentum_ui::prelude::*;

/// (id, title, what it is).
const TABS: &[(&str, &str, &str)] = &[
    ("accounts", "Accounts", "Your mail and calendar accounts, and whether they sync."),
    ("you", "You", "Your name signs emails the Secretary writes; the language is used for Today and the brief."),
    ("today", "Today & brief", "When the morning brief, the evening wrap-up and the week ahead arrive."),
    ("calendar", "Calendar", "Where new events go, and which accounts count as work."),
    ("place", "Place & weather", "Weather for the brief, and your time zone."),
    ("travel", "Travel", "For “leave by” reminders before events with a place."),
    ("mail", "Mail", "How often mail syncs and what notifies you."),
    ("writing", "Writing", "The second language of the writing tools."),
    ("model", "Your model", "The model server and model the Secretary, the brief and the writing tools use. Nothing leaves your machines unless you search the web."),
    ("memory", "Memory", "What the Secretary remembers about you, in every chat. It saves a fact when you tell it something worth keeping or ask it to remember; fix or remove anything here."),
    ("voice", "Voice", "Talk to the Secretary and hear its replies, through your speech server (Whisper writes out what you say, Kokoro reads replies aloud)."),
    ("look", "Appearance", "How Agentum looks on this device. Each device keeps its own; a new one starts with your last pick."),
    ("sidebar", "Sidebar", "The app's name, the order of your pages and Home Assistant."),
    ("security", "Security", "Your password and passkeys (Touch ID / Face ID)."),
    ("machines", "Machines", "The computers your coding agents run on. Each runs agentd, which connects to this hub."),
    ("plugins", "Plugins", "Turn each plugin on or off, and set it up."),
];

pub fn page(_pane: Pane) -> AnyView {
    view! { <Gate/> }.into_any()
}

/// Can this browser use passkeys (a secure page and WebAuthn)?
pub fn passkeys_work() -> bool {
    passkey::works()
}

/// Sign in with a passkey. Err("") when the person cancelled.
pub async fn passkey_sign_in() -> Result<Value, String> {
    passkey::sign_in().await
}

/// Locked while a passkey exists and this session hasn't unlocked in the last 10 minutes.
#[component]
fn Gate() -> impl IntoView {
    let state = RwSignal::new(None::<(u64, i64)>);
    let load = move || {
        spawn_local(async move {
            let s = api_get::<Value>("/auth/passkey/state").await.unwrap_or(json!({}));
            state.set(Some((s["count"].as_u64().unwrap_or(0), s["unlocked_until"].as_i64().unwrap_or(0))));
        })
    };
    load();
    // Lock again when the unlock runs out.
    let relock = StoredValue::new_local(None::<gloo_timers::callback::Timeout>);
    Effect::new(move |_| {
        if let Some((_, until)) = state.get().filter(|(_, u)| *u > 0) {
            let ms = (until * 1000 - now_ms()).max(0) + 500;
            relock.set_value(Some(gloo_timers::callback::Timeout::new(ms.min(i32::MAX as i64) as u32, load)));
        }
    });
    let locked = Memo::new(move |_| state.get().map(|(c, u)| c > 0 && u * 1000 <= now_ms()));
    let pw = RwSignal::new(String::new());
    let unlock_pw = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let body = json!({"password": pw.get_untracked()});
        spawn_local(async move {
            match api_post::<Value>("/auth/passkey/unlock/password", &body).await {
                Ok(_) => {
                    pw.set(String::new());
                    load();
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    move || match locked.get() {
        None => view! { <div class="empty">"Loading…"</div> }.into_any(),
        Some(false) => view! { <Body/> }.into_any(),
        Some(true) => view! {
            <div class="set-lock">
                <Icon name="lock" size=30/>
                <div class="hi">"Settings are locked"</div>
                <div class="muted small">"Unlock with Touch ID or Face ID; it stays open for 10 minutes."</div>
                <Show when=passkey::works>
                    <button class="btn primary" on:click=move |_| spawn_local(async move {
                        match passkey::unlock().await {
                            Ok(_) => load(),
                            Err(e) if e.is_empty() => {}
                            Err(e) => toast_bad(e),
                        }
                    })><Icon name="fingerprint" size=15/>"Unlock"</button>
                </Show>
                <form class="row" on:submit=unlock_pw>
                    <input type="password" placeholder="or your password" autocomplete="current-password" prop:value=pw on:input=move |e| pw.set(event_target_value(&e))/>
                    <button class="btn" disabled=move || pw.get().is_empty()>"Unlock"</button>
                </form>
            </div>
        }.into_any(),
    }
}

/// What the hub's settings say (`GET /api/settings`) and the edits not saved yet.
#[derive(Clone, Copy)]
pub(crate) struct Ctx {
    pub d: RwSignal<Option<Value>>,
    pub env: RwSignal<std::collections::BTreeMap<String, String>>,
    pub models: RwSignal<Vec<String>>,
}
impl Ctx {
    pub fn field(&self, key: &str) -> Option<Value> {
        self.d.with(|d| d.as_ref().and_then(|d| d["fields"].as_array().and_then(|a| a.iter().find(|f| f["key"] == key).cloned())))
    }
    /// The value shown: the edit, else what is saved.
    pub fn val(&self, key: &str) -> String {
        self.env.with(|e| e.get(key).cloned()).unwrap_or_else(|| self.field(key).and_then(|f| f["value"].as_str().map(str::to_string)).unwrap_or_default())
    }
    pub fn set(&self, key: &str, v: String) {
        self.env.update(|e| {
            e.insert(key.to_string(), v);
        });
    }
    pub fn changed(&self) -> Vec<(String, String)> {
        let env = self.env.get();
        env.into_iter().filter(|(k, v)| self.field(k).and_then(|f| f["value"].as_str().map(str::to_string)).as_deref() != Some(v.as_str())).collect()
    }
    pub fn reload(&self) {
        let (d, env) = (self.d, self.env);
        spawn_local(async move {
            match api_get::<Value>("/settings").await {
                Ok(x) => {
                    d.set(Some(x));
                    env.set(Default::default());
                }
                Err(e) => toast_bad(e.message),
            }
        });
    }
}

#[component]
fn Body() -> impl IntoView {
    let tab_p = use_param("tab");
    let section_p = use_param("section");
    let tab = Memo::new(move |_| {
        let want = tab_p.get().or_else(|| section_p.get()).unwrap_or_default();
        TABS.iter().find(|t| t.0 == want).map(|t| t.0).unwrap_or("accounts")
    });
    let cx = Ctx { d: RwSignal::new(None), env: RwSignal::new(Default::default()), models: RwSignal::new(vec![]) };
    cx.reload();
    let saving = RwSignal::new(false);
    let changed = Memo::new(move |_| cx.changed());
    let save = move |_| {
        saving.set(true);
        let env: serde_json::Map<String, Value> = changed.get_untracked().into_iter().map(|(k, v)| (k, json!(v))).collect();
        spawn_local(async move {
            match api_post::<Value>("/settings", &json!({"env": env})).await {
                Ok(x) => {
                    cx.d.set(Some(x));
                    cx.env.set(Default::default());
                    toast("Saved.");
                }
                Err(e) => toast_bad(e.message),
            }
            saving.set(false);
        });
    };
    let info = move || TABS.iter().find(|t| t.0 == tab.get()).copied().unwrap_or(TABS[0]);
    view! {
        <div class="settings">
            <nav class="set-nav" aria-label="Settings">
                {TABS.iter().map(|(k, t, _)| {
                    let k = *k;
                    view! { <button class=move || if tab.get() == k { "nav on" } else { "nav" } aria-current=move || (tab.get() == k).then_some("page") on:click=move |_| set_param("tab", Some(k))>{*t}</button> }
                }).collect_view()}
            </nav>
            <div class="set-body">
                <h2>{move || info().1}</h2>
                <div class="muted small" style="margin-bottom:14px">{move || info().2}</div>
                {move || {
                    if cx.d.with(|d| d.is_none()) && !matches!(tab.get(), "machines" | "plugins" | "security" | "memory" | "accounts") {
                        return view! { <div class="muted small">"Loading…"</div> }.into_any();
                    }
                    match tab.get() {
                        "accounts" => view! { <accounts::Accounts/> }.into_any(),
                        "memory" => view! { <prefs::Memories/> }.into_any(),
                        "look" => view! { <prefs::Appearance/> }.into_any(),
                        "sidebar" => view! { <prefs::SidebarPrefs cx=cx/> }.into_any(),
                        "security" => view! { <prefs::Security/> }.into_any(),
                        "machines" => view! { <machines::Machines/> }.into_any(),
                        "plugins" => view! { <plugins::Plugins/> }.into_any(),
                        sec => view! {
                            <fields::Fields cx=cx section=sec/>
                            {(sec == "voice").then(|| view! { <prefs::VoiceStatus/> })}
                        }.into_any(),
                    }
                }}
            </div>
            <Show when=move || !changed.get().is_empty()>
                <div class="save-bar">
                    <span class="grow">{move || { let n = changed.get().len(); format!("{n} change{}", if n > 1 { "s" } else { "" }) }}</span>
                    <button class="btn" on:click=move |_| cx.env.set(Default::default())>"Undo"</button>
                    <button class="btn primary" disabled=move || saving.get() on:click=save>"Save"</button>
                </div>
            </Show>
        </div>
    }
}
