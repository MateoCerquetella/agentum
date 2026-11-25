//! A widget's settings: its title and the option fields its type declares (GET /api/dashboard
//! `types`: text, number, choice, strings, and items with fields of their own).
use agentum_ui::prelude::*;

#[derive(Clone, Debug, PartialEq)]
enum Seg {
    Key(String),
    Idx(usize),
}
type Path = Vec<Seg>;

fn get<'a>(v: &'a Value, p: &[Seg]) -> &'a Value {
    p.iter().fold(v, |v, s| match s {
        Seg::Key(k) => &v[k.as_str()],
        Seg::Idx(i) => &v[*i],
    })
}
fn get_mut<'a>(v: &'a mut Value, p: &[Seg]) -> &'a mut Value {
    let mut cur = v;
    for s in p {
        cur = match s {
            Seg::Key(k) => {
                if !cur.is_object() {
                    *cur = json!({});
                }
                cur.as_object_mut().unwrap().entry(k.clone()).or_insert(Value::Null)
            }
            Seg::Idx(i) => {
                if !cur.is_array() {
                    *cur = json!([]);
                }
                let a = cur.as_array_mut().unwrap();
                while a.len() <= *i {
                    a.push(json!({}));
                }
                &mut a[*i]
            }
        };
    }
    cur
}
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}
fn with(p: &Path, s: Seg) -> Path {
    let mut p = p.clone();
    p.push(s);
    p
}

fn field(m: RwSignal<Value>, path: Path, spec: Value) -> AnyView {
    let label = spec["label"].as_str().unwrap_or("").to_string();
    let hint = spec["hint"].as_str().unwrap_or("").to_string();
    let hint_v = (!hint.is_empty()).then(|| view! { <small class="subdue">{hint.clone()}</small> });
    let p = path.clone();
    match spec["kind"].as_str().unwrap_or("text") {
        "number" => view! {
            <label class="field"><span>{label}</span>
                <input type="number" prop:value=move || m.with(|v| text_of(get(v, &p)))
                    on:input={ let p = path.clone(); move |e| { let x = event_target_value(&e); m.update(|v| *get_mut(v, &p) = x.trim().parse::<i64>().map(|n| json!(n)).unwrap_or(Value::Null)) } }/>
                {hint_v}
            </label>
        }.into_any(),
        "choice" => {
            let opts: Vec<(String, String)> = spec["options"].as_array().cloned().unwrap_or_default().iter().map(|o| (text_of(&o[0]), text_of(&o[1]))).collect();
            view! {
                <label class="field"><span>{label}</span>
                    <select class="pick" on:change={ let p = path.clone(); move |e| { let x = event_target_value(&e); m.update(|v| *get_mut(v, &p) = json!(x)) } }>
                        {opts.into_iter().map(|(val, l)| {
                            let p = p.clone();
                            let v2 = val.clone();
                            view! { <option value=val selected=move || m.with(|v| text_of(get(v, &p)) == v2)>{l}</option> }
                        }).collect_view()}
                    </select>
                    {hint_v}
                </label>
            }.into_any()
        }
        "strings" => {
            // One per line; empty lines stay while typing and go on save.
            let local = RwSignal::new(m.with_untracked(|v| get(v, &p).as_array().map(|a| a.iter().map(text_of).collect::<Vec<_>>().join("\n")).unwrap_or_default()));
            view! {
                <label class="field"><span>{label}</span>
                    <textarea rows="4" prop:value=move || local.get() on:input=move |e| {
                        let x = event_target_value(&e);
                        local.set(x.clone());
                        m.update(|v| *get_mut(v, &p) = json!(x.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>()));
                    }/>
                    {hint_v}
                </label>
            }.into_any()
        }
        "items" => {
            let subs: Vec<Value> = spec["fields"].as_array().cloned().unwrap_or_default();
            let len = Memo::new({ let p = p.clone(); move |_| m.with(|v| get(v, &p).as_array().map(|a| a.len()).unwrap_or(0)) });
            let p2 = p.clone();
            view! {
                <div class="field dash-items"><span>{label}</span>
                    {hint_v}
                    {move || (0..len.get()).map(|i| {
                        let ip = with(&p, Seg::Idx(i));
                        let pr = p.clone();
                        view! {
                            <fieldset class="dash-item">
                                {subs.iter().map(|f| field(m, with(&ip, Seg::Key(f["key"].as_str().unwrap_or("").to_string())), f.clone())).collect_view()}
                                <button type="button" class="btn ghost danger" on:click=move |_| m.update(|v| { if let Some(a) = get_mut(v, &pr).as_array_mut() { if i < a.len() { a.remove(i); } } })>
                                    <Icon name="trash" size=12/>" Remove"
                                </button>
                            </fieldset>
                        }
                    }).collect_view()}
                    <button type="button" class="btn" on:click=move |_| m.update(|v| { let a = get_mut(v, &p2); if !a.is_array() { *a = json!([]) } a.as_array_mut().unwrap().push(json!({})) })>
                        <Icon name="plus" size=12/>" Add"
                    </button>
                </div>
            }.into_any()
        }
        _ => view! {
            <label class="field"><span>{label}</span>
                <input prop:value=move || m.with(|v| text_of(get(v, &p))) placeholder=hint.clone()
                    on:input={ let p = path.clone(); move |e| { let x = event_target_value(&e); m.update(|v| *get_mut(v, &p) = json!(x)) } }/>
            </label>
        }.into_any(),
    }
}

/// `widget` is the existing one (None: a new widget of `ty`). `on_done(Some(item))` after a save,
/// `on_done(None)` after a delete.
#[component]
pub fn Editor(ty: String, types: Value, widget: Option<Value>, on_close: Callback<()>, on_done: Callback<(String, Option<Value>)>) -> impl IntoView {
    let t = types[&ty].clone();
    let label = t["label"].as_str().unwrap_or(&ty).to_string();
    let id = widget.as_ref().and_then(|w| w["id"].as_str().map(str::to_string));
    let title = RwSignal::new(widget.as_ref().and_then(|w| w["title"].as_str().map(str::to_string)).unwrap_or_else(|| label.clone()));
    let opts = RwSignal::new(widget.as_ref().map(|w| w["options"].clone()).filter(|o| o.is_object()).unwrap_or_else(|| json!({})));
    let busy = RwSignal::new(false);
    let err = RwSignal::new(String::new());
    let heading = match &widget {
        Some(_) => format!("{label}: {}", title.get_untracked()),
        None => format!("New {}", label.to_lowercase()),
    };
    let (ty2, id2, id3) = (ty.clone(), id.clone(), id.clone());
    let save = move || {
        if title.get_untracked().trim().is_empty() {
            return err.set("Give it a title.".into());
        }
        busy.set(true);
        let mut body = json!({"type": ty2, "title": title.get_untracked().trim(), "options": opts.get_untracked()});
        if let Some(id) = &id2 {
            body["id"] = json!(id);
        }
        spawn_local(async move {
            match api_put::<Value>("/widgets", &body).await {
                Ok(item) => on_done.run((item["id"].as_str().unwrap_or("").to_string(), Some(item))),
                Err(e) => {
                    err.set(e.message);
                    busy.set(false);
                }
            }
        });
    };
    let delete = move || {
        let Some(id) = id3.clone() else { return };
        spawn_local(async move {
            if !confirm("Delete this widget?", "It leaves every page and its settings are gone.", "Delete widget", true).await {
                return;
            }
            match api_delete::<Value>(&format!("/widgets/{}", js_sys::encode_uri_component(&id))).await {
                Ok(_) => on_done.run((id, None)),
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let fields: Vec<Value> = t["fields"].as_array().cloned().unwrap_or_default();
    let existing = widget.is_some();
    view! {
        <Modal title=heading on_close=on_close>
            <form class="cal-form dash-editor" on:submit=move |e| { e.prevent_default(); save() }>
                <label class="field"><span>"Title"</span>
                    <input maxlength="50" prop:value=move || title.get() on:input=move |e| title.set(event_target_value(&e))/>
                </label>
                {fields.into_iter().map(|f| field(opts, vec![Seg::Key(f["key"].as_str().unwrap_or("").to_string())], f)).collect_view()}
                <Show when=move || !err.get().is_empty()><div class="small err">{move || err.get()}</div></Show>
                <div class="row">
                    {existing.then(|| view! { <button type="button" class="btn ghost danger" on:click=move |_| delete()><Icon name="trash" size=14/>" Delete widget"</button> })}
                    <span class="grow"/>
                    <button type="button" class="btn" on:click=move |_| on_close.run(())>"Cancel"</button>
                    <button type="submit" class="btn primary" disabled=move || busy.get()>{if existing { "Save" } else { "Create and add" }}</button>
                </div>
            </form>
        </Modal>
    }
}
