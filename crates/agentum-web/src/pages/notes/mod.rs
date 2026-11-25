//! Notes: your floatnote files (Markdown kept in step by Syncthing, [`files`]) and to-dos (the
//! notes table's `todo` rows; Today uses them). `?note=<id>` opens a note.
mod assist;
mod files;

use super::Pane;
use agentum_ui::prelude::*;

pub fn page(_pane: Pane) -> AnyView {
    view! { <Notes/> }.into_any()
}

#[component]
fn Notes() -> impl IntoView {
    let saved = agentum_ui::store::get("cerq.notes.filter").unwrap_or_default();
    let filter = RwSignal::new(if saved == "todo" || saved == "done" { saved } else { "note".to_string() });
    let note_link = use_param("note");
    Effect::new(move |_| {
        if note_link.get().is_some() && filter.get_untracked() != "note" {
            filter.set("note".into());
        }
    });
    let q = RwSignal::new(String::new());
    let todos = RwSignal::new(Vec::<Value>::new());
    let open = RwSignal::new(None::<Value>);
    let quick = RwSignal::new(String::new());
    let load = move || {
        spawn_local(async move {
            match api_get::<Vec<Value>>("/notes").await {
                Ok(n) => todos.set(n),
                Err(e) => toast_bad(e.message),
            }
        })
    };
    load();
    subscribe(&["app"]);
    on_ws(move |m| {
        if let agentum_proto::api::WsOut::App { payload } = m {
            if payload["kind"] == "todos" {
                load();
            }
        }
    });
    let shown = move || {
        let (f, ql) = (filter.get(), q.get().to_lowercase());
        let mut v: Vec<Value> = todos.get().into_iter().filter(|n| {
            let text = format!("{} {}", n["title"].as_str().unwrap_or(""), n["content"].as_str().unwrap_or("")).to_lowercase();
            n["note_type"] == "todo" && (ql.is_empty() || text.contains(&ql)) && ((f == "done") == (n["done"] == true))
        }).collect();
        let due = |n: &Value| n["due"].as_str().filter(|d| !d.is_empty()).unwrap_or("9").to_string();
        let upd = |n: &Value| n["updated_at"].as_str().unwrap_or("").to_string();
        v.sort_by(|a, b| (if f == "todo" { due(a).cmp(&due(b)) } else { std::cmp::Ordering::Equal }).then_with(|| upd(b).cmp(&upd(a))));
        v
    };
    let add = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let t = quick.get_untracked().trim().to_string();
        if t.is_empty() {
            return;
        }
        spawn_local(async move {
            match api_post::<Value>("/todos", &json!({"title": t, "note_type": "todo"})).await {
                Ok(_) => {
                    quick.set(String::new());
                    load();
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let today = {
        let d = js_sys::Date::new_0();
        format!("{:04}-{:02}-{:02}", d.get_full_year(), d.get_month() + 1, d.get_date())
    };
    view! {
        <div class="notes">
            <div class="notes-bar">
                <div class="notes-tabs" role="group" aria-label="Notes views">
                    {[("note", "Notes"), ("todo", "To-dos"), ("done", "Done")].into_iter().map(|(f, label)| view! {
                        <button type="button" aria-pressed=move || (filter.get() == f).to_string() on:click=move |_| { filter.set(f.into()); agentum_ui::store::set("cerq.notes.filter", f) }>{label}</button>
                    }).collect_view()}
                </div>
                <label class="notes-search"><Icon name="search" size=15/>
                    <input type="search" aria-label=move || if filter.get() == "note" { "Search notes" } else { "Search to-dos" }
                        placeholder=move || if filter.get() == "note" { "Search notes…" } else { "Search to-dos…" } prop:value=q on:input=move |e| q.set(event_target_value(&e))/>
                </label>
            </div>
            <Show when=move || filter.get() == "todo">
                <form class="row" style="margin-bottom:12px" on:submit=add>
                    <input class="grow" placeholder="Add a to-do…" prop:value=quick on:input=move |e| quick.set(event_target_value(&e))/>
                    <button class="btn primary" disabled=move || quick.get().trim().is_empty()><Icon name="plus" size=14/>" Add"</button>
                </form>
            </Show>
            {move || if filter.get() == "note" {
                view! { <files::Files q=q.into()/> }.into_any()
            } else {
                let today = today.clone();
                let v = shown();
                let empty = v.is_empty();
                view! {
                    <ul class="todo-list">
                        {v.into_iter().map(|n| {
                            let id = n["id"].as_str().unwrap_or("").to_string();
                            let done = n["done"] == true;
                            let items = n["items"].as_array().cloned().unwrap_or_default();
                            let ticked = items.iter().filter(|i| i["done"] == true || i["checked"] == true).count();
                            let due = n["due"].as_str().filter(|d| !d.is_empty()).map(str::to_string);
                            let late = due.as_ref().is_some_and(|d| !done && d.get(..10).unwrap_or(d) < today.as_str());
                            let (n1, n2) = (n.clone(), n.clone());
                            view! {
                                <li class=if done { "done" } else { "" }>
                                    <input type="checkbox" prop:checked=done on:change=move |_| {
                                        let id = id.clone();
                                        spawn_local(async move {
                                            match api_put::<Value>(&format!("/notes/{id}"), &json!({"done": !done})).await {
                                                Ok(_) => load(),
                                                Err(e) => toast_bad(e.message),
                                            }
                                        });
                                    }/>
                                    <span class="grow" on:click=move |_| open.set(Some(n1.clone()))>{n["title"].as_str().unwrap_or("").to_string()}
                                        {(items.len() > 1).then(|| view! { <span class="subdue small">{format!(" · {ticked}/{}", items.len())}</span> })}</span>
                                    {due.map(|d| view! { <span class=if late { "small neg" } else { "small subdue" }>{d.replace('T', " ").chars().take(16).collect::<String>()}</span> })}
                                    <button class="icon-btn" title="Edit" on:click=move |_| open.set(Some(n2.clone()))><Icon name="edit" size=13/></button>
                                </li>
                            }
                        }).collect_view()}
                        {empty.then(|| view! { <div class="empty">{if filter.get_untracked() == "done" { "Nothing done yet." } else { "Nothing to do. Nice." }}</div> })}
                    </ul>
                }.into_any()
            }}
            {move || open.get().map(|n| view! { <TodoEditor note=n on_close=Callback::new(move |_| { open.set(None); load() })/> })}
        </div>
    }
}

/// A to-do: title, due, notes and its checklist.
#[component]
fn TodoEditor(note: Value, on_close: Callback<()>) -> impl IntoView {
    let id = note["id"].as_str().unwrap_or("").to_string();
    let title = RwSignal::new(note["title"].as_str().unwrap_or("").to_string());
    let content = RwSignal::new(note["content"].as_str().unwrap_or("").to_string());
    let pinned = RwSignal::new(note["pinned"] == true);
    let due = RwSignal::new(note["due"].as_str().unwrap_or("").chars().take(16).collect::<String>());
    let items = RwSignal::new(note["items"].as_array().cloned().unwrap_or_default());
    let is_todo = note["note_type"] == "todo";
    let (id1, id2) = (id.clone(), id.clone());
    let save = move || {
        let body = json!({"title": title.get_untracked(), "content": content.get_untracked(), "items": items.get_untracked(), "pinned": pinned.get_untracked(), "due": due.get_untracked()});
        let id = id1.clone();
        spawn_local(async move {
            if let Err(e) = api_put::<Value>(&format!("/notes/{id}"), &body).await {
                toast_bad(e.message);
            }
            on_close.run(());
        });
    };
    let save2 = save.clone();
    view! {
        <div class="note-editor" on:keydown=move |e| if e.key() == "Escape" { save2() }>
            <div class="row">
                <input class="grow title" placeholder="Title" autofocus prop:value=title on:input=move |e| title.set(event_target_value(&e))/>
                <button class=move || if pinned.get() { "icon-btn on" } else { "icon-btn" } title="Pin" on:click=move |_| pinned.update(|p| *p = !*p)><Icon name="pin"/></button>
                <button class="icon-btn" title="Delete" on:click=move |_| {
                    let id = id2.clone();
                    let t = title.get_untracked();
                    spawn_local(async move {
                        if confirm("Delete this?", t, "Delete", true).await {
                            match api_delete::<Value>(&format!("/notes/{id}")).await {
                                Ok(_) => on_close.run(()),
                                Err(e) => toast_bad(e.message),
                            }
                        }
                    });
                }><Icon name="trash"/></button>
                <button class="btn primary" on:click=move |_| save()>"Done"</button>
            </div>
            {is_todo.then(|| view! {
                <label class="field"><span>"Due"</span><input type="datetime-local" style="width:230px" prop:value=due on:input=move |e| due.set(event_target_value(&e))/></label>
            })}
            <textarea rows="8" placeholder="Write…" prop:value=content on:input=move |e| content.set(event_target_value(&e))/>
            <div class="field"><span>"Checklist"</span>
                {move || items.get().into_iter().enumerate().map(|(i, it)| view! {
                    <div class="row">
                        <input type="checkbox" prop:checked=it["done"] == true || it["checked"] == true on:change=move |e| {
                            let on = event_target_checked(&e);
                            items.update(|v| { v[i]["done"] = json!(on); v[i]["checked"] = json!(on); });
                        }/>
                        <input class="grow" prop:value=it["text"].as_str().unwrap_or("").to_string() on:change=move |e| { let t = event_target_value(&e); items.update(|v| v[i]["text"] = json!(t)) }/>
                        <button class="icon-btn" aria-label="Remove item" on:click=move |_| items.update(|v| { v.remove(i); })><Icon name="x" size=12/></button>
                    </div>
                }).collect_view()}
                <button class="btn ghost" style="align-self:flex-start" on:click=move |_| items.update(|v| v.push(json!({"text": "", "done": false})))><Icon name="plus" size=12/>" Item"</button>
            </div>
        </div>
    }
}
