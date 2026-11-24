//! Appearance, per device: a look (Glance or Studio) and a mode (System, Light, Dark).
//! html[data-theme] is the look and html[data-scheme] the light or dark it resolves to;
//! each look's tokens are at the end of index.css. A pick is also saved on the hub so every
//! device follows the last one (adopt_look).
use agentum_ui::prelude::*;
use agentum_ui::store;

pub const THEMES: [(&str, &str, &str); 2] = [("glance", "Glance", "JetBrains Mono, deep blue"), ("studio", "Studio", "Geist, soft surfaces, teal")];
pub const MODES: [(&str, &str, &str); 3] = [("system", "System", "monitor"), ("light", "Light", "sun"), ("dark", "Dark", "moon")];
const KEY: &str = "cerq.theme";
const MODE_KEY: &str = "cerq.mode";

thread_local! {
    static LOOK: RwSignal<u32> = RwSignal::new(0);
}
/// Bumps on every change of look or mode.
pub fn look_signal() -> RwSignal<u32> {
    LOOK.with(|l| *l)
}

pub fn theme() -> String {
    store::get(KEY).filter(|v| THEMES.iter().any(|(k, ..)| k == v)).unwrap_or_else(|| "studio".into())
}
pub fn mode() -> String {
    store::get(MODE_KEY).filter(|v| MODES.iter().any(|(k, ..)| k == v)).unwrap_or_else(|| "system".into())
}
fn system_dark() -> bool {
    web_sys::window().and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten()).is_some_and(|m| m.matches())
}
pub fn scheme() -> &'static str {
    match mode().as_str() {
        "light" => "light",
        "dark" => "dark",
        _ => if system_dark() { "dark" } else { "light" },
    }
}

pub fn apply_saved() {
    let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.document_element()) else { return };
    let _ = el.set_attribute("data-theme", &theme());
    let _ = el.set_attribute("data-scheme", scheme());
    look_signal().update(|n| *n += 1);
}

fn save(key: &str, v: &str) {
    store::set(key, v);
    apply_saved();
    let body = json!({"theme": theme(), "mode": mode()});
    spawn_local(async move {
        let _ = api_put::<Value>("/settings/look", &body).await;
    });
}
pub fn set_theme(t: &str) {
    save(KEY, t)
}
pub fn set_mode(m: &str) {
    save(MODE_KEY, m)
}

/// The look saved on the hub (the last pick on any device): this device takes it.
pub fn adopt_look(look: &Value) {
    let mut changed = false;
    if let Some(t) = look["theme"].as_str().filter(|t| THEMES.iter().any(|(k, ..)| k == t)) {
        if store::get(KEY).as_deref() != Some(t) {
            store::set(KEY, t);
            changed = true;
        }
    }
    if let Some(m) = look["mode"].as_str().filter(|m| MODES.iter().any(|(k, ..)| k == m)) {
        if store::get(MODE_KEY).as_deref() != Some(m) {
            store::set(MODE_KEY, m);
            changed = true;
        }
    }
    if changed {
        apply_saved();
    }
}

/// System, Light or Dark: three icons; `compact`, one button that steps through them.
#[component]
pub fn ModeSwitch(#[prop(optional)] compact: bool, #[prop(optional)] labels: bool) -> impl IntoView {
    let look = look_signal();
    if compact {
        return view! {
            <button class="icon-btn" title=move || { look.track(); let m = mode(); format!("{}: click to switch", MODES.iter().find(|x| x.0 == m).map(|x| x.1).unwrap_or("System")) }
                on:click=move |_| { let m = mode(); let i = MODES.iter().position(|x| x.0 == m).unwrap_or(0); set_mode(MODES[(i + 1) % 3].0) }>
                <Icon name=Signal::derive(move || { look.track(); let m = mode(); MODES.iter().find(|x| x.0 == m).map(|x| x.2).unwrap_or("monitor").to_string() }) size=16/>
            </button>
        }.into_any();
    }
    view! {
        <div class=if labels { "mode-switch labels" } else { "mode-switch" } role="radiogroup" aria-label="Light or dark">
            {MODES.iter().map(|(k, label, icon)| {
                let k = *k;
                view! {
                    <button type="button" role="radio" aria-checked=move || { look.track(); (mode() == k).to_string() } aria-label=*label title=format!("{label} theme") on:click=move |_| set_mode(k)>
                        <Icon name=*icon size=14/>{labels.then_some(*label)}
                    </button>
                }
            }).collect_view()}
        </div>
    }.into_any()
}
