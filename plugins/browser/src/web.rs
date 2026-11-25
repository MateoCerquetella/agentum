//! Browser's web part: a project's own browser (Surf) beside a thread (the Browser tab: start it
//! with one click), and every project's browser at /p/browser. The viewer is Steel's, served by
//! the hub under /api/p/browser/view/… (same origin, the person's session), in a frame.
use agentum_plugin::web::{NavEntry, RailTab, ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::{Machine, WsOut};
use agentum_proto::machine::SurfInfo;
use agentum_ui::prelude::*;
use wasm_bindgen::JsCast;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn nav(&self) -> Option<NavEntry> {
        Some(NavEntry { path: "/p/browser", title: "Browser", icon: "globe", view: || view! { <BrowsersPage/> }.into_any() })
    }
    fn rail_tabs(&self) -> &'static [RailTab] {
        &[RailTab { id: "browser", title: "Browser", icon: "globe", view: rail }]
    }
}

fn rail(cx: ThreadCx) -> AnyView {
    let summary = cx.summary;
    let place = Memo::new(move |_| summary.with(|s| s.as_ref().map(|s| (s.machine.clone(), s.project.clone()))));
    view! {
        {move || place.get().map(|(machine, project)| view! { <ProjectBrowser machine project/> })}
    }
    .into_any()
}

fn name_of(project: &str) -> String {
    project.trim_end_matches('/').rsplit('/').next().unwrap_or(project).to_string()
}

fn surf_of(v: &Value) -> Option<SurfInfo> {
    serde_json::from_value(v.clone()).ok()
}

/// Refresh when the plugin says a machine's browsers changed.
fn on_change(machine: Option<String>, f: impl Fn() + 'static) {
    subscribe(&["plugin:browser"]);
    on_ws(move |m| {
        if let WsOut::Plugin { plugin, payload } = m {
            if plugin == "browser" && (payload["settings"] == true || machine.as_deref().is_none_or(|mc| payload["surf"] == mc)) {
                f();
            }
        }
    });
}

/// The thread's project's browser, or a button to start it.
#[component]
fn ProjectBrowser(machine: String, project: String) -> impl IntoView {
    let surf = RwSignal::new(None::<SurfInfo>);
    let loaded = RwSignal::new(false);
    let busy = RwSignal::new(None::<&'static str>);
    let error = RwSignal::new(None::<String>);
    let input = StoredValue::new(json!({"machine": machine, "project": project}));
    let refresh = move || {
        spawn_local(async move {
            match rpc::<Value>("browser", "status", &input.get_value()).await {
                Ok(v) => {
                    surf.set(surf_of(&v["surf"]));
                    error.set(None);
                }
                Err(e) => error.set(Some(e.message)),
            }
            loaded.set(true);
        });
    };
    refresh();
    on_change(Some(machine.clone()), refresh);
    let start = move || {
        busy.set(Some("Starting the browser…"));
        error.set(None);
        spawn_local(async move {
            match rpc::<Value>("browser", "start", &input.get_value()).await {
                Ok(v) => surf.set(surf_of(&v["surf"])),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(None);
        });
    };
    let stop = Callback::new(move |_: ()| {
        spawn_local(async move {
            if !confirm("Stop this project's browser?", "Its tabs close; its sign-ins are kept for next time.", "Stop", true).await {
                return;
            }
            busy.set(Some("Stopping…"));
            match rpc::<Value>("browser", "stop", &input.get_value()).await {
                Ok(_) => surf.set(None),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(None);
        });
    });
    let (m2, p2) = (machine.clone(), project.clone());
    view! {
        <div class="brw compact">
            {move || error.get().map(|e| view! { <div class="brw-err small"><Icon name="alert" size=13/>{e}</div> })}
            {move || {
                let (m, p) = (m2.clone(), p2.clone());
                match (loaded.get(), surf.get(), busy.get()) {
                    (false, _, _) => view! { <div class="brw-empty small subdue">"…"</div> }.into_any(),
                    (_, _, Some(what)) => view! {
                        <div class="brw-empty">
                            <Icon name="refresh" size=20 class="spin muted"/>
                            <b>{what}</b>
                            <p class="small subdue">"The first start on a machine downloads the browser (a few minutes)."</p>
                        </div>
                    }.into_any(),
                    (_, Some(s), None) if s.state == "running" => view! { <Viewer machine=m surf=s stop=stop/> }.into_any(),
                    (_, s, None) => {
                        let asleep = s.is_some();
                        view! {
                            <div class="brw-empty">
                                <Icon name="globe" size=22 class="muted"/>
                                <b>{if asleep { format!("{}'s browser is asleep", name_of(&p)) } else { format!("{} has no browser yet", name_of(&p)) }}</b>
                                <p class="small subdue">
                                    {format!("A Chromium of this project's own on {m}: its agents drive it, you watch it here and type what they mustn't see (passwords, codes). Sign-ins are kept.")}
                                </p>
                                <button class="btn primary" on:click=move |_| start()><Icon name="bolt" size=14/>{if asleep { "Wake it" } else { "Start the browser" }}</button>
                            </div>
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}

/// The live viewer: Steel's page through the hub, with Paste (the clipboard bridge) and Stop.
#[component]
fn Viewer(machine: String, surf: SurfInfo, #[prop(optional)] stop: Option<Callback<()>>) -> impl IntoView {
    let frame = NodeRef::<leptos::html::Iframe>::new();
    let reload = RwSignal::new(0u32);
    let paste_open = RwSignal::new(false);
    let paste_text = RwSignal::new(String::new());
    let base = format!("/api/p/browser/view/{machine}/{}/v1/sessions/debug?interactive=true&clipboardBridge=true", surf.id);
    let b2 = base.clone();
    let src = move || {
        let n = reload.get();
        if n == 0 { b2.clone() } else { format!("{b2}&r={n}") }
    };
    // The viewer asks for the clipboard (⌘V in it): open the paste box instead.
    on_window(leptos::ev::message, move |e: web_sys::MessageEvent| {
        let Some(f) = frame.get_untracked() else { return };
        let (Some(win), Some(from)) = (f.content_window(), e.source()) else { return };
        if !js_sys::Object::is(win.as_ref(), from.as_ref()) {
            return;
        }
        let kind = js_sys::Reflect::get(&e.data(), &"type".into()).ok().and_then(|t| t.as_string()).unwrap_or_default();
        if kind == "requestClipboardRead" {
            paste_open.set(true);
        }
    });
    let send_paste = move || {
        let text = paste_text.get_untracked();
        paste_text.set(String::new());
        paste_open.set(false);
        if text.is_empty() {
            return;
        }
        let (Some(f), Ok(origin)) = (frame.get_untracked(), window().location().origin()) else { return };
        if let (Some(win), Ok(msg)) = (f.content_window(), js_sys::JSON::parse(&json!({"type": "triggerPaste", "text": text}).to_string())) {
            let _ = win.post_message(&msg, &origin);
            let _ = f.unchecked_ref::<web_sys::HtmlElement>().focus();
        }
    };
    let title = format!("{} · {machine}", name_of(&surf.project));
    view! {
        <div class="brw-bar row">
            <span class="brw-dot live"/>
            <span class="small grow brw-title" title=surf.project.clone()>{title}</span>
            <button class="icon-btn" title="Paste into the focused field (passwords: they never reach the agent)" on:click=move |_| paste_open.update(|o| *o = !*o)><Icon name="keyboard" size=14/></button>
            <button class="icon-btn" title="Reconnect the viewer" on:click=move |_| reload.update(|n| *n += 1)><Icon name="refresh" size=14/></button>
            <a class="icon-btn" href=base target="_blank" rel="noopener" title="Open in a new tab"><Icon name="link" size=14/></a>
            {stop.map(|s| view! { <button class="icon-btn" title="Stop this browser" on:click=move |_| s.run(())><Icon name="stop" size=14/></button> })}
        </div>
        <Show when=move || paste_open.get()>
            <form class="brw-paste row" on:submit=move |e| { e.prevent_default(); send_paste() }>
                <input type="password" class="grow" autocomplete="off" placeholder="Text for the field focused in the browser" maxlength="4096"
                    prop:value=move || paste_text.get() on:input=move |e| paste_text.set(event_target_value(&e))/>
                <button class="btn primary" type="submit">"Paste"</button>
                <button class="icon-btn" type="button" title="Close" on:click=move |_| { paste_text.set(String::new()); paste_open.set(false) }><Icon name="x" size=14/></button>
            </form>
        </Show>
        <iframe node_ref=frame class="brw-frame" src=src title="The project's browser" allow="clipboard-read; clipboard-write; fullscreen"/>
    }
}

/// /p/browser: every project's browser on every machine.
#[component]
fn BrowsersPage() -> impl IntoView {
    let machines = RwSignal::new(Vec::<Machine>::new());
    let surfs = RwSignal::new(Vec::<(String, SurfInfo)>::new());
    let errors = RwSignal::new(Vec::<(String, String)>::new());
    let loaded = RwSignal::new(false);
    let busy = RwSignal::new(None::<String>);
    let picked = RwSignal::new(None::<(String, String)>);
    let new_pick = RwSignal::new(String::new());
    let refresh = move || {
        spawn_local(async move {
            let ms: Vec<Machine> = api_get("/code/machines").await.unwrap_or_default();
            let mut all = vec![];
            let mut errs = vec![];
            for m in ms.iter().filter(|m| m.online) {
                match rpc::<Value>("browser", "list", &json!({"machine": m.id})).await {
                    Ok(v) => all.extend(v["surfs"].as_array().into_iter().flatten().filter_map(surf_of).map(|s| (m.id.clone(), s))),
                    Err(e) => errs.push((m.name.clone(), e.message)),
                }
            }
            all.sort_by(|a, b| (a.1.state != "running", name_of(&a.1.project)).cmp(&(b.1.state != "running", name_of(&b.1.project))));
            if picked.get_untracked().is_none() {
                picked.set(all.iter().find(|(_, s)| s.state == "running").map(|(m, s)| (m.clone(), s.id.clone())));
            }
            machines.set(ms);
            surfs.set(all);
            errors.set(errs);
            loaded.set(true);
        });
    };
    refresh();
    on_change(None, refresh);
    let act = move |method: &'static str, machine: String, project: String| {
        spawn_local(async move {
            if method == "stop" && !confirm(&format!("Stop {}'s browser?", name_of(&project)), "Its tabs close; its sign-ins are kept for next time.", "Stop", true).await {
                return;
            }
            busy.set(Some(if method == "start" { format!("Starting {}'s browser…", name_of(&project)) } else { "Stopping…".into() }));
            match rpc::<Value>("browser", method, &json!({"machine": machine, "project": project})).await {
                Ok(v) => {
                    if let Some(s) = surf_of(&v["surf"]) {
                        picked.set(Some((machine.clone(), s.id)));
                    }
                }
                Err(e) => toast_bad(e.message),
            }
            busy.set(None);
            refresh();
        });
    };
    let shown = Memo::new(move |_| {
        let p = picked.get()?;
        surfs.with(|v| v.iter().find(|(m, s)| *m == p.0 && s.id == p.1 && s.state == "running").cloned())
    });
    view! {
        <div class="brw brw-page">
            <div class="brw-list">
                <div class="row brw-head">
                    <b class="grow">"Projects' browsers"</b>
                    {move || busy.get().map(|b| view! { <span class="small subdue row"><Icon name="refresh" size=12 class="spin"/>{b}</span> })}
                    <button class="icon-btn" title="Refresh" on:click=move |_| refresh()><Icon name="refresh" size=14/></button>
                </div>
                {move || errors.get().into_iter().map(|(m, e)| view! { <div class="brw-err small"><Icon name="alert" size=13/>{format!("{m}: {e}")}</div> }).collect_view()}
                <Show when=move || loaded.get() && surfs.with(Vec::is_empty)>
                    <p class="small subdue brw-none">"No project has a browser yet. Start one below, or from a thread's Browser tab."</p>
                </Show>
                {move || surfs.get().into_iter().map(|(m, s)| {
                    let running = s.state == "running";
                    let (m1, p1, m2, p2, m3, id3) = (m.clone(), s.project.clone(), m.clone(), s.project.clone(), m.clone(), s.id.clone());
                    let on = { let (m, id) = (m.clone(), s.id.clone()); move || picked.get().is_some_and(|p| p.0 == m && p.1 == id) };
                    view! {
                        <div class=move || if on() { "brw-item row on" } else { "brw-item row" }>
                            <span class=if running { "brw-dot live" } else { "brw-dot" }/>
                            <button class="brw-name grow" title=s.project.clone() disabled=!running on:click=move |_| picked.set(Some((m3.clone(), id3.clone())))>
                                <b>{name_of(&s.project)}</b>
                                <span class="small subdue">{format!("{m} · {}", if running { s.status.clone() } else { "asleep (its next agent wakes it)".into() })}</span>
                            </button>
                            {(!running).then(|| view! { <button class="btn" on:click=move |_| act("start", m1.clone(), p1.clone())>"Wake"</button> })}
                            <button class="icon-btn" title="Stop and remove (sign-ins are kept)" on:click=move |_| act("stop", m2.clone(), p2.clone())><Icon name="stop" size=14/></button>
                        </div>
                    }
                }).collect_view()}
                <form class="row brw-new" on:submit=move |e| {
                    e.prevent_default();
                    if let Some((m, p)) = new_pick.get_untracked().split_once('\u{1f}') { act("start", m.to_string(), p.to_string()) }
                }>
                    <select class="grow" aria-label="Project" on:change=move |e| new_pick.set(event_target_value(&e))>
                        <option value="">"Start a browser for…"</option>
                        {move || machines.get().into_iter().filter(|m| m.online).flat_map(|m| {
                            let have: Vec<String> = surfs.with(|v| v.iter().filter(|(mm, _)| *mm == m.id).map(|(_, s)| s.project.clone()).collect());
                            m.projects.into_iter().filter(move |p| !have.contains(&p.path)).map(move |p| {
                                let v = format!("{}\u{1f}{}", m.id, p.path);
                                view! { <option value=v>{format!("{} · {}", p.name, m.name)}</option> }
                            })
                        }).collect_view()}
                    </select>
                    <button class="btn" type="submit" disabled=move || new_pick.with(String::is_empty)>"Start"</button>
                </form>
            </div>
            {move || match shown.get() {
                Some((m, s)) => view! { <div class="brw-view"><Viewer machine=m surf=s/></div> }.into_any(),
                None => view! { <div class="brw-view brw-empty small subdue">"Pick a running browser to watch it."</div> }.into_any(),
            }}
        </div>
    }
}
