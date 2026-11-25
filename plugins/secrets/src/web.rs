//! Secrets' web part: the tab. Key names are listed; a value is typed (or pasted) into a masked
//! field, sent once, and the field is cleared. Values are never read back.
use agentum_plugin::web::{RailTab, ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_ui::prelude::*;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

static TABS: [RailTab; 1] = [RailTab { id: "secrets", title: "Secrets", icon: "lock", view: tab }];

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn rail_tabs(&self) -> &'static [RailTab] {
        &TABS
    }
    fn css(&self) -> Option<&'static str> {
        Some(include_str!("../style.css"))
    }
}

fn tab(cx: ThreadCx) -> AnyView {
    view! { <SecretsTab cx=cx/> }.into_any()
}

fn keys_of(d: &Value) -> Vec<String> {
    d["keys"].as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_string)).collect()
}

#[component]
fn SecretsTab(cx: ThreadCx) -> impl IntoView {
    let id = StoredValue::new(cx.id.clone());
    let data = RwSignal::new(None::<Value>);
    let err = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let key = RwSignal::new(String::new());
    let value = RwSignal::new(String::new());
    let value_el = NodeRef::<leptos::html::Input>::new();
    let dir = Memo::new(move |_| cx.summary.with(|s| s.as_ref().map(|s| s.cwd.clone()).unwrap_or_default()));
    let keys = Memo::new(move |_| data.with(|d| d.as_ref().map(keys_of).unwrap_or_default()));

    // One call to the hub; `done` is the toast when it worked.
    let call = move |method: &'static str, body: Value, done: Option<String>| {
        busy.set(true);
        spawn_local(async move {
            let r = rpc::<Value>("secrets", method, &body).await;
            if busy.is_disposed() {
                return;
            }
            busy.set(false);
            match r {
                Ok(v) => {
                    err.set(None);
                    if let Some(msg) = done {
                        toast(if v["gitignore_added"] == true { format!("{msg} .env was added to .gitignore.") } else { msg });
                    }
                    data.set(Some(v));
                }
                Err(e) if method == "keys" => err.set(Some(e.message)),
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let load = move || call("keys", json!({"thread": id.get_value()}), None);
    load();

    let save = move || {
        let k = key.get_untracked().trim().to_string();
        let v = value.get_untracked();
        if k.is_empty() {
            toast_bad("Name the key first.");
            return;
        }
        if v.is_empty() {
            toast_bad("Type or paste its value.");
            return;
        }
        let verb = if keys.with_untracked(|ks| ks.contains(&k)) { "Updated" } else { "Added" };
        // The value leaves the page now, whatever happens next.
        value.set(String::new());
        key.set(String::new());
        call("set", json!({"thread": id.get_value(), "key": k, "value": v}), Some(format!("{verb} {k}.")));
    };
    let edit = move |k: String| {
        key.set(k);
        value.set(String::new());
        if let Some(el) = value_el.get_untracked() {
            let _ = el.focus();
        }
    };
    let delete = move |k: String| {
        spawn_local(async move {
            let file = format!("{}/.env", dir.get_untracked());
            if !confirm(format!("Delete {k}?"), format!("It is removed from {file}."), "Delete", true).await || busy.is_disposed() {
                return;
            }
            call("delete", json!({"thread": id.get_value(), "key": k}), Some(format!("Deleted {k}.")));
        });
    };
    let updating = move || key.with(|k| keys.with(|ks| ks.iter().any(|x| x == k.trim())));

    view! {
        <div class="sec">
            <div class="sec-bar">
                <b>"Secrets"</b>
                <span class="small subdue truncate" title=move || format!("{}/.env", dir.get())>{move || format!("{}/.env", dir.get().rsplit('/').next().unwrap_or(""))}</span>
                <span class="grow"></span>
                <button class="icon-btn" title="Read .env again" on:click=move |_| load()>
                    <Icon name="refresh" size=14 class=Signal::derive(move || if busy.get() { "spin".to_string() } else { String::new() })/>
                </button>
            </div>
            <div class="sec-body">
                {move || err.get().map(|e| view! { <div class="err small">{e}</div> })}
                {move || data.get().map(|d| view! {
                    {(d["tracked"] == true).then(|| view! {
                        <div class="sec-warn small"><Icon name="alert" size=13/>
                            <span>".env is committed: its values are in git's history. Untrack it (" <code>"git rm --cached .env"</code> ") and change what was in it."</span>
                        </div>
                    })}
                    {(d["ignored"] == false && d["tracked"] != true).then(|| view! {
                        <div class="sec-warn small"><Icon name="alert" size=13/><span>"git doesn't ignore .env yet: saving a key adds it to .gitignore."</span></div>
                    })}
                })}
                <div class="sec-list">
                    {move || {
                        let ks = keys.get();
                        if data.with(|d| d.is_none()) {
                            return (err.with(|e| e.is_none())).then(|| view! { <div class="empty small">"Reading .env…"</div> }.into_any());
                        }
                        if ks.is_empty() {
                            return Some(view! { <div class="empty small">"No keys in .env yet."</div> }.into_any());
                        }
                        Some(ks.into_iter().map(|k| {
                            let (k1, k2, title) = (k.clone(), k.clone(), k.clone());
                            view! {
                                <div class="sec-row">
                                    <code class="sec-key truncate" title=title>{k}</code>
                                    <span class="sec-dots" aria-label="hidden value">"••••••"</span>
                                    <button class="icon-btn" title="Set a new value" on:click=move |_| edit(k1.clone())><Icon name="edit" size=13/></button>
                                    <button class="icon-btn" title="Delete" disabled=move || busy.get() on:click=move |_| delete(k2.clone())><Icon name="trash" size=13/></button>
                                </div>
                            }
                        }).collect_view().into_any())
                    }}
                </div>
                <form class="sec-form" on:submit=move |e| { e.prevent_default(); save() }>
                    <label class="sec-field">
                        <span class="small subdue">"Key"</span>
                        <input type="text" placeholder="OPENAI_API_KEY" autocomplete="off" spellcheck="false" autocapitalize="characters"
                            prop:value=key on:input=move |e| key.set(event_target_value(&e))/>
                    </label>
                    <label class="sec-field">
                        <span class="small subdue">"Value"</span>
                        <input type="password" node_ref=value_el placeholder="Never shown again" autocomplete="off" data-1p-ignore="" data-lpignore="true"
                            prop:value=value on:input=move |e| value.set(event_target_value(&e))/>
                    </label>
                    <button class="btn primary" type="submit" disabled=move || busy.get()>{move || if updating() { "Update" } else { "Add" }}</button>
                </form>
                <p class="small subdue sec-note">
                    "A value goes from here to the machine's .env and nowhere else: not into the thread, not into a log. Agents ask for keys by name and read them from .env."
                </p>
            </div>
        </div>
    }
}
