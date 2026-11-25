//! Aura's web part: a slow colour field behind the whole app. Pure CSS (radial gradients moved by
//! transform keyframes on two compositor layers): no JS animation loop, nothing repaints while you
//! scroll, and it holds still under prefers-reduced-motion. Its CSS is
//! crates/agentum-web/style/plugins/agentum-aura.css; while it is drawn, html[data-aura] makes the
//! sidebar and panes a little translucent so it shows through.
use agentum_plugin::web::WebPlugin;
use agentum_plugin::Manifest;
use agentum_proto::api::WsOut;
use agentum_ui::prelude::*;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn overlay(&self) -> Option<AnyView> {
        Some(view! { <Aura/> }.into_any())
    }
}

fn colors(palette: &str, custom: &str) -> [String; 3] {
    let p: [&str; 3] = match palette {
        "sunset" => ["#ff6b6b", "#ffb347", "#c86bfa"],
        "ocean" => ["#00b4d8", "#2f6bff", "#48e5c2"],
        "forest" => ["#2ecc71", "#16a085", "#c6d93a"],
        "theme" => ["var(--primary)", "var(--positive)", "var(--warm)"],
        "custom" => {
            let c: Vec<String> = custom.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_alphanumeric() || "#(),.% -".contains(ch))).collect();
            if c.len() >= 3 {
                return [c[0].clone(), c[1].clone(), c[2].clone()];
            }
            ["#7c5cff", "#00c2ff", "#ff5ca8"]
        }
        _ => ["#7c5cff", "#00c2a8", "#2f80ff"],
    };
    p.map(str::to_string)
}

fn html_attr(on: bool) {
    if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.document_element()) {
        let _ = if on { el.set_attribute("data-aura", "") } else { el.remove_attribute("data-aura") };
    }
}

#[component]
fn Aura() -> impl IntoView {
    let set = RwSignal::new(Value::Null);
    let load = move || {
        spawn_local(async move {
            if let Ok(v) = api_get::<Vec<Value>>("/plugins").await {
                if let Some(p) = v.into_iter().find(|p| p["id"] == "aura") {
                    set.set(p["settings"].clone());
                }
            }
        })
    };
    load();
    subscribe(&["plugin:aura"]);
    on_ws(move |m| {
        if let WsOut::Plugin { plugin, payload } = m {
            if plugin == "aura" && payload["settings"] == true {
                load();
            }
        }
    });
    html_attr(true);
    on_cleanup(|| html_attr(false));
    let style = move || {
        set.with(|s| {
            let [a, b, c] = colors(s["palette"].as_str().unwrap_or("aurora"), s["colors"].as_str().unwrap_or(""));
            let o = s["intensity"].as_f64().unwrap_or(35.0).clamp(5.0, 100.0) / 100.0;
            format!("--aura-a:{a};--aura-b:{b};--aura-c:{c};opacity:{o:.2}")
        })
    };
    let still = move || set.with(|s| s["motion"] == false);
    view! {
        <div class="aura-field" class:still=still aria-hidden="true" style=style>
            <i class="aura-l1"/>
            <i class="aura-l2"/>
        </div>
    }
}
