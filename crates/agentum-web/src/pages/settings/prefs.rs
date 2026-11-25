//! The smaller tabs: Appearance, Sidebar, Security (password, passkeys), Memory, the voice check.
use super::{passkey, Ctx};
use crate::pages::theme::{self, ModeSwitch};
use agentum_ui::prelude::*;

/// Per device: the look and System, Light or Dark.
#[component]
pub fn Appearance() -> impl IntoView {
    let look = theme::look_signal();
    view! {
        <div class="col" style="gap:12px">
            <div class="set-looks" role="radiogroup" aria-label="Look">
                {theme::all_themes().into_iter().map(|(k, name, note)| {
                    let on = move || { look.track(); theme::theme() == k };
                    view! {
                        <button type="button" role="radio" aria-checked=move || on().to_string() class=move || if on() { "set-look on" } else { "set-look" } on:click=move |_| theme::set_theme(k)>
                            <span class=move || { look.track(); format!("set-swatch sw-{k}-{}", theme::scheme()) }><i/><i/><i/></span>
                            <span class="col" style="gap:2px;align-items:flex-start">
                                <span class="hi">{name}</span>
                                <span class="small muted" style="text-align:left">{note}</span>
                            </span>
                        </button>
                    }
                }).collect_view()}
            </div>
            <ModeSwitch labels=true/>
            <div class="small subdue">"Saved on this device, and a new device starts with it. Every page follows. System follows your device’s light or dark mode."</div>
        </div>
    }
}

/// The app's name, the order of your pages in the sidebar, Home Assistant.
#[component]
pub fn SidebarPrefs(cx: Ctx) -> impl IntoView {
    let brand = RwSignal::new(cx.d.with_untracked(|d| d.as_ref().and_then(|d| d["brand"].as_str().map(str::to_string))).filter(|b| !b.eq_ignore_ascii_case("cerq")).unwrap_or_else(|| "Agentum".into()));
    let ui = move || cx.d.with(|d| d.as_ref().map(|d| d["ui"].clone()).unwrap_or(json!({})));
    let ha = RwSignal::new(cx.d.with_untracked(|d| d.as_ref().and_then(|d| d["ui"]["ha_url"].as_str().map(str::to_string))).unwrap_or_default());
    let order = RwSignal::new(Vec::<&'static str>::new());
    Effect::new(move |_| {
        let u = ui();
        let mut on: Vec<&'static str> = crate::pages::NAV.to_vec();
        if u["ha_url"].as_str().is_some_and(|h| !h.is_empty()) {
            on.push("home");
        }
        let saved: Vec<String> = u["sidebar"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect();
        let mut out: Vec<&'static str> = saved.iter().filter_map(|s| on.iter().copied().find(|p| p == s)).collect();
        for p in on {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        order.set(out);
    });
    let shift = move |i: usize, by: isize| {
        let mut v = order.get_untracked();
        let j = i as isize + by;
        if j < 0 || j as usize >= v.len() {
            return;
        }
        v.swap(i, j as usize);
        order.set(v.clone());
        spawn_local(async move {
            if let Err(e) = api_put::<Value>("/settings/sidebar", &json!({"order": v})).await {
                toast_bad(e.message);
            }
        });
    };
    let save_brand = move |_| {
        let b = brand.get_untracked();
        spawn_local(async move {
            match api_post::<Value>("/settings", &json!({"brand": b})).await {
                Ok(x) => {
                    toast("Renamed.");
                    cx.d.set(Some(x));
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let save_ha = move |_| {
        let mut u = ui();
        if !u.is_object() {
            u = json!({});
        }
        u["ha_url"] = json!(ha.get_untracked().trim());
        spawn_local(async move {
            match api_post::<Value>("/settings", &json!({"ui": u})).await {
                Ok(x) => {
                    toast("Saved. Home shows in the sidebar.");
                    cx.d.set(Some(x));
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    view! {
        <div class="col" style="gap:0">
            <div class="set-row">
                <div class="set-label">"App name"</div>
                <div class="set-ctl row">
                    <input maxlength="24" prop:value=brand on:input=move |e| brand.set(event_target_value(&e))/>
                    <button class="btn" on:click=save_brand>"Save"</button>
                </div>
            </div>
            <div class="set-row">
                <div class="set-label">"Pages"<div class="small subdue">"in the Personal side"</div></div>
                <div class="set-ctl col ag-order" style="gap:4px">
                    {move || {
                        let v = order.get();
                        let n = v.len();
                        v.into_iter().enumerate().map(|(i, id)| {
                            let d = crate::pages::def(id);
                            view! {
                                <div class="row ag-order-row">
                                    <Icon name=d.map(|d| d.icon).unwrap_or("dot") class="muted"/>
                                    <span class="grow">{d.map(|d| d.title).unwrap_or(id)}</span>
                                    <button class="icon-btn" title="Move up" aria-label="Move up" disabled=i == 0 on:click=move |_| shift(i, -1)><Icon name="chevronU" size=14/></button>
                                    <button class="icon-btn" title="Move down" aria-label="Move down" disabled=i + 1 == n on:click=move |_| shift(i, 1)><Icon name="chevronD" size=14/></button>
                                </div>
                            }
                        }).collect_view()
                    }}
                    <span class="small subdue">"You can also drag a page onto another in the sidebar."</span>
                </div>
            </div>
            <div class="set-row">
                <div class="set-label">"Home Assistant"</div>
                <div class="set-ctl row">
                    <input class="grow" placeholder="https://home.example.lan" prop:value=ha on:input=move |e| ha.set(event_target_value(&e))/>
                    <button class="btn" on:click=save_ha>"Save"</button>
                </div>
            </div>
        </div>
    }
}

/// The password, and the passkeys that sign in without it.
#[component]
pub fn Security() -> impl IntoView {
    let cur = RwSignal::new(String::new());
    let new = RwSignal::new(String::new());
    let change = move |_| {
        let body = json!({"current": cur.get_untracked(), "new": new.get_untracked()});
        spawn_local(async move {
            match api_post::<Value>("/auth/password", &body).await {
                Ok(_) => {
                    toast("Password changed.");
                    cur.set(String::new());
                    new.set(String::new());
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    view! {
        <div class="col" style="gap:0">
            <div class="set-row">
                <div class="set-label">"Password"</div>
                <div class="set-ctl col">
                    <input type="password" placeholder="Current password" autocomplete="current-password" prop:value=cur on:input=move |e| cur.set(event_target_value(&e))/>
                    <input type="password" placeholder="New password (8+ characters)" autocomplete="new-password" prop:value=new on:input=move |e| new.set(event_target_value(&e))/>
                    <button class="btn" style="align-self:flex-start" disabled=move || { cur.get().is_empty() || new.get().chars().count() < 8 } on:click=change>"Change password"</button>
                </div>
            </div>
            <Passkeys/>
            <div class="set-row">
                <div class="set-label">"This device"</div>
                <div class="set-ctl">
                    <button class="btn" on:click=move |_| spawn_local(async {
                        let _ = api_post::<Value>("/auth/logout", &json!({})).await;
                        let _ = web_sys::window().unwrap().location().reload();
                    })>"Sign out"</button>
                </div>
            </div>
        </div>
    }
}

fn date_of(secs: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(secs as f64 * 1000.0));
    d.to_locale_date_string("default", &wasm_bindgen::JsValue::UNDEFINED).into()
}

#[component]
fn Passkeys() -> impl IntoView {
    let st = RwSignal::new(None::<Value>);
    let load = move || {
        spawn_local(async move {
            if let Ok(s) = api_get::<Value>("/auth/passkey/state").await {
                st.set(Some(s));
            }
        })
    };
    load();
    let add = move |_| {
        spawn_local(async move {
            match passkey::add(passkey::device_name()).await {
                Ok(_) => {
                    toast("Passkey added: next time, Touch ID / Face ID.");
                    load();
                }
                Err(e) if e.is_empty() => {}
                Err(e) => toast_bad(e),
            }
        })
    };
    view! {
        <div class="set-row">
            <div class="set-label">"Touch ID / Face ID"</div>
            <div class="set-ctl col" style="gap:6px">
                {move || st.get().map(|s| s["passkeys"].as_array().cloned().unwrap_or_default()).unwrap_or_default().into_iter().map(|p| {
                    let id = p["id"].as_str().unwrap_or("").to_string();
                    let name = p["name"].as_str().unwrap_or("Passkey").to_string();
                    let used = p["last_used"].as_i64().filter(|t| *t > 0).map(|t| format!(" · used {} ago", ago(t))).unwrap_or_default();
                    let n2 = name.clone();
                    view! {
                        <div class="row small">
                            <Icon name="fingerprint" size=14 class="muted"/>
                            <span class="hi">{name}</span>
                            <span class="subdue grow">"added "{date_of(p["created"].as_i64().unwrap_or(0))}{used}</span>
                            <button class="btn ghost small" on:click=move |_| {
                                let (id, name) = (id.clone(), n2.clone());
                                spawn_local(async move {
                                    if !confirm(format!("Remove the passkey “{name}”?"), "That device signs in with the password again.", "Remove", true).await { return; }
                                    match api_delete::<Value>(&format!("/auth/passkey/{}", js_sys::encode_uri_component(&id))).await {
                                        Ok(_) => load(),
                                        Err(e) => toast_bad(e.message),
                                    }
                                });
                            }>"Remove"</button>
                        </div>
                    }
                }).collect_view()}
                {if passkey::works() {
                    view! { <button class="btn" style="align-self:flex-start" on:click=add><Icon name="plus" size=13/>{format!("Add this {}", passkey::device_name())}</button> }.into_any()
                } else {
                    view! { <span class="small err">"Passkeys need a secure page: open Agentum over https."</span> }.into_any()
                }}
                <span class="small subdue">"Sign in without the password; Settings asks for it before you change anything (or your password, if the device isn’t at hand). Passkeys sync to your other Apple devices through iCloud Keychain."</span>
            </div>
        </div>
    }
}

/// What the Secretary remembers: add, fix, forget.
#[component]
pub fn Memories() -> impl IntoView {
    let list = RwSignal::new(None::<Vec<Value>>);
    let add = RwSignal::new(String::new());
    let edit = RwSignal::new(None::<(String, String)>);
    let load = move || {
        spawn_local(async move {
            match api_get::<Vec<Value>>("/memories").await {
                Ok(l) => list.set(Some(l)),
                Err(e) => toast_bad(e.message),
            }
        })
    };
    load();
    subscribe(&["app"]);
    on_ws(move |m| {
        if let agentum_proto::api::WsOut::App { payload } = m {
            if payload["kind"] == "memories" {
                load();
            }
        }
    });
    let run = move |fut: std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, ApiError>>>>, after: Box<dyn Fn()>| {
        spawn_local(async move {
            match fut.await {
                Ok(_) => {
                    after();
                    load();
                }
                Err(e) => toast_bad(e.message),
            }
        })
    };
    view! {
        <div class="col" style="gap:8px">
            <form class="row" on:submit=move |e| {
                e.prevent_default();
                let t = add.get_untracked();
                if t.trim().is_empty() { return; }
                run(Box::pin(async move { api_post::<Value>("/memories", &json!({"text": t})).await }), Box::new(move || add.set(String::new())));
            }>
                <input class="grow" placeholder="Something the Secretary should always know, e.g. “I prefer meetings after 10”" prop:value=add on:input=move |e| add.set(event_target_value(&e))/>
                <button class="btn" disabled=move || add.get().trim().is_empty()><Icon name="plus" size=13/>" Add"</button>
            </form>
            {move || match list.get() {
                None => view! { <div class="muted small">"Loading…"</div> }.into_any(),
                Some(l) if l.is_empty() => view! { <div class="muted small">"Nothing yet. Tell the Secretary “remember that…” and it shows up here."</div> }.into_any(),
                Some(l) => l.into_iter().map(|m| {
                    let id = m["id"].as_str().map(str::to_string).unwrap_or_else(|| m["id"].to_string());
                    let text = m["text"].as_str().unwrap_or("").to_string();
                    let when = m["created_at"].as_str().map(|c| format!(" · {}", c.chars().take(10).collect::<String>())).unwrap_or_default();
                    let (i1, i2, i3, t1) = (id.clone(), id.clone(), id.clone(), text.clone());
                    view! {
                        <div class="set-row" style="align-items:center">
                            {move || if edit.get().is_some_and(|(e, _)| e == i1) {
                                let i = i1.clone();
                                view! {
                                    <form class="row grow" on:submit=move |e| {
                                        e.prevent_default();
                                        let t = edit.get_untracked().map(|x| x.1).unwrap_or_default();
                                        let i = i.clone();
                                        run(Box::pin(async move { api_put::<Value>(&format!("/memories/{i}"), &json!({"text": t})).await }), Box::new(move || edit.set(None)));
                                    }>
                                        <input class="grow" autofocus prop:value=move || edit.get().map(|x| x.1).unwrap_or_default()
                                            on:input=move |e| { let v = event_target_value(&e); edit.update(|x| if let Some(x) = x { x.1 = v }) }
                                            on:keydown=move |e| if e.key() == "Escape" { edit.set(None) }/>
                                        <button class="btn primary">"Save"</button>
                                        <button type="button" class="btn" on:click=move |_| edit.set(None)>"Cancel"</button>
                                    </form>
                                }.into_any()
                            } else {
                                let (i2, i3, t1) = (i2.clone(), i3.clone(), t1.clone());
                                view! {
                                    <div class="grow">{text.clone()}<span class="subdue small">{when.clone()}</span></div>
                                    <button class="icon-btn" title="Edit" aria-label="Edit memory" on:click=move |_| edit.set(Some((i2.clone(), t1.clone())))><Icon name="edit" size=13/></button>
                                    <button class="icon-btn" title="Forget" aria-label="Forget memory" on:click=move |_| {
                                        let i = i3.clone();
                                        run(Box::pin(async move { api_delete::<Value>(&format!("/memories/{i}")).await }), Box::new(|| {}));
                                    }><Icon name="trash" size=13/></button>
                                }.into_any()
                            }}
                        </div>
                    }
                }).collect_view().into_any(),
            }}
        </div>
    }
}

/// Is the speech server answering?
#[component]
pub fn VoiceStatus() -> impl IntoView {
    let st = RwSignal::new(None::<Value>);
    let check = move || {
        st.set(None);
        spawn_local(async move {
            let s = api_get::<Value>("/voice/status").await.unwrap_or_else(|e| json!({"ok": false, "error": e.message}));
            st.set(Some(s));
        })
    };
    check();
    view! {
        <div class="set-row">
            <div class="set-label">"Status"</div>
            <div class="set-ctl row">
                {move || match st.get() {
                    None => view! { <span class="muted small">"Checking…"</span> }.into_any(),
                    Some(s) if s["ok"] == true => view! { <span class="small pos">"The speech server is answering. The mic shows in the Secretary."</span> }.into_any(),
                    Some(s) => view! { <span class="small err">{s["error"].as_str().unwrap_or("It didn't answer.").to_string()}</span> }.into_any(),
                }}
                <button class="btn ghost small" on:click=move |_| check()>"Check again"</button>
            </div>
        </div>
    }
}
