//! Notifications' web part: a sound in open pages when a thread needs you, finishes or fails (the
//! built-in sounds of Agentum's desktop app), chosen per device; the other settings are the
//! manifest's (drawn by Settings → Plugins); this adds the sounds and "Send a test".
use agentum_plugin::web::WebPlugin;
use agentum_plugin::Manifest;
use agentum_proto::api::WsOut;
use agentum_ui::prelude::*;
use agentum_ui::store;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

/// Served from `/sounds/<id>.mp3`.
const SOUNDS: [&str; 9] = ["two-tone", "bong", "thump", "blip", "sonar", "blop", "ding", "clack", "beep"];
/// (kind, label, default sound)
const KINDS: [(&str, &str, &str); 3] = [("ask", "Needs you", "two-tone"), ("done", "Done", "ding"), ("failed", "Failed", "bong")];

fn sound_for(kind: &str) -> String {
    let default = KINDS.iter().find(|k| k.0 == kind).map(|k| k.2).unwrap_or("ding");
    store::get(&format!("agentum.sound.{kind}")).unwrap_or_else(|| default.to_string())
}
fn volume() -> f64 {
    store::get("agentum.sound.volume").and_then(|v| v.parse().ok()).unwrap_or(70.0)
}

fn play(sound: &str) {
    if sound == "off" || !SOUNDS.contains(&sound) {
        return;
    }
    if let Ok(a) = web_sys::HtmlAudioElement::new_with_src(&format!("/sounds/{sound}.mp3")) {
        a.set_volume((volume() / 100.0).clamp(0.0, 1.0));
        // Refused until the page has had a click or a key: nothing to do about it then.
        let _ = a.play();
    }
}

/// One sound per notice across every open tab of this browser: the first tab to claim it plays.
fn claim(key: &str) -> bool {
    let Some(ls) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else { return true };
    let now = js_sys::Date::now();
    let last: f64 = ls.get_item("agentum.sound.last").ok().flatten().and_then(|v| v.split_once(' ').and_then(|(t, k)| (k == key).then(|| t.parse().ok()).flatten())).unwrap_or(0.0);
    if now - last < 3000.0 {
        return false;
    }
    let _ = ls.set_item("agentum.sound.last", &format!("{now} {key}"));
    true
}

/// Are you looking at this thread right now (this tab in front, the thread open)?
fn watching(thread: &str) -> bool {
    let Some(w) = web_sys::window() else { return false };
    let Some(d) = w.document() else { return false };
    !thread.is_empty() && d.has_focus().unwrap_or(false) && d.visibility_state() == web_sys::VisibilityState::Visible && w.location().search().unwrap_or_default().contains(thread)
}

fn on_notice(payload: &Value) {
    let (Some(kind), thread) = (payload["sound"].as_str(), payload["thread"].as_str().unwrap_or("")) else { return };
    if store::get("agentum.sound.watching").as_deref() != Some("1") && watching(thread) {
        return;
    }
    let sound = sound_for(kind);
    let key = format!("{kind}:{thread}");
    // A random beat so that two tabs don't both read "unclaimed" at the same instant.
    let wait = (js_sys::Math::random() * 120.0) as u32;
    gloo_timers::callback::Timeout::new(wait, move || {
        if claim(&key) {
            play(&sound);
        }
    })
    .forget();
}

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    fn overlay(&self) -> Option<AnyView> {
        subscribe(&["plugin:notify"]);
        on_ws(|m| {
            if let WsOut::Plugin { plugin, payload } = m
                && plugin == "notify"
            {
                on_notice(payload);
            }
        });
        None
    }

    fn settings(&self) -> Option<AnyView> {
        let busy = RwSignal::new(false);
        let test = move |_| {
            busy.set(true);
            spawn_local(async move {
                match rpc::<Value>("notify", "test", &json!({})).await {
                    Ok(v) if v["quiet"] == true => toast("Sent. (Quiet hours are on now: threads stay silent until quiet hours end.)"),
                    Ok(_) => toast("Sent: check your devices."),
                    Err(e) => toast_bad(e.message),
                }
                busy.set(false);
            });
        };
        let vol = RwSignal::new(volume());
        let watching = RwSignal::new(store::get("agentum.sound.watching").as_deref() == Some("1"));
        Some(view! {
            <div style="display:flex;flex-direction:column;gap:8px">
                <div class="small subdue">"Sounds on this device, in any open Agentum tab (one tab plays)."</div>
                {KINDS.iter().map(|&(kind, label, _)| {
                    let cur = RwSignal::new(sound_for(kind));
                    view! {
                        <div class="row" style="gap:8px">
                            <span style="width:90px">{label}</span>
                            <select style="width:160px" prop:value=move || cur.get() on:change=move |e| {
                                let v = event_target_value(&e);
                                store::set(&format!("agentum.sound.{kind}"), &v);
                                play(&v);
                                cur.set(v);
                            }>
                                <option value="off">"No sound"</option>
                                {SOUNDS.iter().map(|s| view! { <option value=*s selected=move || cur.get() == *s>{*s}</option> }).collect_view()}
                            </select>
                            <button class="icon-btn" title="Play it" on:click=move |_| play(&cur.get_untracked())><Icon name="volume" size=14/></button>
                        </div>
                    }
                }).collect_view()}
                <div class="row" style="gap:8px">
                    <span style="width:90px">"Volume"</span>
                    <input type="range" min="0" max="100" step="5" style="width:160px" prop:value=move || vol.get().to_string()
                        on:input=move |e| { let v: f64 = event_target_value(&e).parse().unwrap_or(70.0); store::set("agentum.sound.volume", &v.to_string()); vol.set(v) }
                        on:change=move |_| play(&sound_for("done"))/>
                    <span class="small subdue">{move || format!("{}%", vol.get())}</span>
                </div>
                <label class="row" style="gap:8px">
                    <input type="checkbox" prop:checked=move || watching.get() on:change=move |e| {
                        let on = event_target_checked(&e);
                        store::set("agentum.sound.watching", if on { "1" } else { "" });
                        watching.set(on);
                    }/>
                    <span>"Also for the thread I'm looking at"</span>
                </label>
                <div class="row" style="gap:8px">
                    <button class="btn" disabled=move || busy.get() on:click=test><Icon name="bell" size=14/>"Send a test"</button>
                    <span class="small subdue">"Pushes go to the devices that allowed notifications, and a toast and a sound in open pages."</span>
                </div>
            </div>
        }.into_any())
    }
}
