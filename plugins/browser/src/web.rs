//! Browser's web part: the agents' browser (Steel) as a page (/p/browser) and as a tab beside a
//! thread. Lists Steel's sessions (through the hub part) and shows the viewer in a frame.
use agentum_plugin::web::{NavEntry, RailTab, ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::WsOut;
use agentum_ui::prelude::*;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn nav(&self) -> Option<NavEntry> {
        Some(NavEntry { path: "/p/browser", title: "Browser", icon: "globe", view: || view! { <Browser compact=false/> }.into_any() })
    }
    fn rail_tabs(&self) -> &'static [RailTab] {
        &[RailTab { id: "browser", title: "Browser", icon: "globe", view: rail }]
    }
}

fn rail(_cx: ThreadCx) -> AnyView {
    view! { <Browser compact=true/> }.into_any()
}

#[derive(Clone, PartialEq)]
struct Session {
    id: String,
    status: String,
    created: String,
    viewer: String,
}

fn web_url(u: &str) -> bool {
    u.starts_with("http://") || u.starts_with("https://")
}

#[component]
fn Browser(compact: bool) -> impl IntoView {
    let viewer = RwSignal::new(String::new());
    let api = RwSignal::new(String::new());
    let sessions = RwSignal::new(Vec::<Session>::new());
    let error = RwSignal::new(None::<String>);
    let loaded = RwSignal::new(false);
    let picked = RwSignal::new(None::<String>);
    let refresh = move || {
        spawn_local(async move {
            if let Ok(v) = api_get::<Vec<Value>>("/plugins").await {
                if let Some(p) = v.into_iter().find(|p| p["id"] == "browser") {
                    viewer.set(p["settings"]["viewer_url"].as_str().unwrap_or("").trim().to_string());
                    api.set(p["settings"]["api_url"].as_str().unwrap_or("").trim().to_string());
                }
            }
            match rpc::<Value>("browser", "sessions", &json!({})).await {
                Ok(v) => {
                    error.set(None);
                    sessions.set(v["sessions"].as_array().into_iter().flatten().map(|s| Session {
                        id: s["id"].as_str().unwrap_or("").to_string(),
                        status: s["status"].as_str().unwrap_or("").to_string(),
                        created: s["createdAt"].as_str().unwrap_or("").chars().take(16).collect::<String>().replace('T', " "),
                        viewer: [&s["sessionViewerUrl"], &s["debugUrl"]].iter().filter_map(|u| u.as_str()).find(|u| web_url(u)).unwrap_or("").to_string(),
                    }).collect());
                }
                Err(e) => error.set(Some(e.message)),
            }
            loaded.set(true);
        });
    };
    refresh();
    subscribe(&["plugin:browser"]);
    on_ws(move |m| {
        if let WsOut::Plugin { plugin, payload } = m {
            if plugin == "browser" && payload["settings"] == true {
                refresh();
            }
        }
    });
    // What the frame shows: the picked session's viewer, else the viewer setting, else the live session's.
    let src = Memo::new(move |_| {
        let ss = sessions.get();
        let chosen = picked.get().and_then(|id| ss.iter().find(|s| s.id == id).map(|s| s.viewer.clone())).filter(|u| !u.is_empty());
        chosen
            .or_else(|| Some(viewer.get()).filter(|v| web_url(v)))
            .or_else(|| ss.iter().find(|s| s.status == "live" && !s.viewer.is_empty()).map(|s| s.viewer.clone()))
    });
    let configured = move || !viewer.get().is_empty() || !api.get().is_empty();
    view! {
        <div class=if compact { "brw compact" } else { "brw" }>
            <Show when=move || loaded.get() && !configured()>
                <div class="brw-empty">
                    <Icon name="globe" size=22 class="muted"/>
                    <b>"The agents' browser isn't set up"</b>
                    <p class="small subdue">
                        "Agentum shows Steel, the browser your agents drive. Run it on a machine ("
                        <code>"docker run -p 3000:3000 -p 5173:5173 ghcr.io/steel-dev/steel-browser"</code>
                        "), then in Settings → Plugins → Browser set the Steel API (http://<machine>:3000) and, if you like, the viewer (http://<machine>:5173)."
                    </p>
                    <button class="btn" on:click=move |_| navigate("settings", Where::Replace, &[("tab", "plugins")])><Icon name="settings" size=14/>"Open Settings"</button>
                </div>
            </Show>
            <Show when=configured>
                <div class="brw-bar row">
                    <span class="small subdue">{move || { let n = sessions.with(Vec::len); if n == 1 { "1 session".to_string() } else { format!("{n} sessions") } }}</span>
                    <div class="brw-sessions grow">
                        {move || sessions.get().into_iter().map(|s| {
                            let id = s.id.clone();
                            let id2 = s.id.clone();
                            view! {
                                <button class=move || if picked.get().as_deref() == Some(id.as_str()) { "chip on" } else { "chip" } title=format!("{} · started {}", s.id, s.created)
                                    on:click=move |_| picked.set(Some(id2.clone()))>
                                    <span class=format!("brw-dot {}", s.status)/>{s.id.chars().take(8).collect::<String>()}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                    {move || src.get().map(|u| view! { <a class="icon-btn" href=u target="_blank" rel="noopener" title="Open in a new tab"><Icon name="link" size=14/></a> })}
                    <button class="icon-btn" title="Refresh" on:click=move |_| refresh()><Icon name="refresh" size=14/></button>
                </div>
                {move || error.get().map(|e| view! { <div class="brw-err small"><Icon name="alert" size=13/>{e}</div> })}
                {move || match src.get() {
                    Some(u) => view! { <iframe class="brw-frame" src=u title="The agents' browser" allow="clipboard-read; clipboard-write"/> }.into_any(),
                    None => view! { <div class="brw-empty small subdue">"No live session to show. One appears when an agent opens the browser."</div> }.into_any(),
                }}
            </Show>
        </div>
    }
}
