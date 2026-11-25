//! Generative UI's web part: draws ```ui blocks (see `lang` for the language).
//!
//! The block is evaluated again whenever its `$state` changes. The DOM is rebuilt only when the
//! layout changes ([`lang::shape`]: containers, their children, inputs); otherwise each leaf
//! (a Metric, a Chart…) redraws itself from its own memo and inputs keep their focus mid-drag.
use crate::lang::{self, Node, State, Val};
use agentum_plugin::web::WebPlugin;
use agentum_plugin::Manifest;
use agentum_ui::prelude::*;
use std::sync::Arc;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn code_fence(&self, lang: &str, code: &str) -> Option<AnyView> {
        (lang == "ui").then(|| view! { <GenUi src=code.to_string()/> }.into_any())
    }
}

#[derive(Clone, Copy)]
struct G {
    tree: Memo<Vec<Val>>,
    state: RwSignal<State>,
}

type NodeSig = Signal<Option<Arc<Node>>>;

#[component]
fn GenUi(src: String) -> impl IntoView {
    let prog = Arc::new(lang::Program::parse(&src));
    let (init, errs) = prog.initial_state();
    let state = RwSignal::new(init);
    let p2 = prog.clone();
    let tree = Memo::new(move |_| state.with(|s| p2.roots(s)));
    let shape = Memo::new(move |_| tree.with(|t| lang::shape(t)));
    let g = G { tree, state };
    let show_src = RwSignal::new(false);
    view! {
        <div class="gu-root">
            {errs.into_iter().map(|e| view! { <div class="gu-error"><Icon name="alert" size=13/>{e}</div> }).collect_view()}
            {move || {
                shape.track();
                let n = tree.with_untracked(|t| t.len());
                (0..n).map(|i| render(g, vec![i])).collect_view()
            }}
            <div class="gu-src-bar">
                <button class="gu-src-btn" title="The block as the agent wrote it" on:click=move |_| show_src.update(|s| *s = !*s)>
                    <Icon name="code" size=12/>{move || if show_src.get() { "Hide source" } else { "Source" }}
                </button>
            </div>
            <Show when=move || show_src.get()>
                <pre class="gu-code">{src.clone()}</pre>
            </Show>
        </div>
    }
}

fn node_at(g: G, path: Vec<usize>) -> NodeSig {
    Memo::new(move |_| g.tree.with(|t| match lang::at(t, &path) {
        Some(Val::Node(n)) => Some(n),
        _ => None,
    }))
    .into()
}

/// The structural path: containers and inputs stay put while values change.
fn render(g: G, path: Vec<usize>) -> AnyView {
    match g.tree.with_untracked(|t| lang::at(t, &path)) {
        Some(Val::Node(n)) if lang::child_slot(&n.name).is_some() => {
            let kids = (0..n.kids().len())
                .map(|i| {
                    let mut p = path.clone();
                    p.push(i);
                    render(g, p)
                })
                .collect();
            container(g, &n.name, node_at(g, path), kids)
        }
        Some(Val::Node(n)) if lang::is_input(&n.name) => input(g, &n.name, node_at(g, path)),
        Some(Val::Node(_)) => {
            let node = node_at(g, path);
            (move || node.get().map(|n| leaf(g, &n))).into_any()
        }
        Some(_) => {
            let p = path.clone();
            let text = Memo::new(move |_| g.tree.with(|t| lang::at(t, &p).map(|v| v.text()).unwrap_or_default()));
            view! { <p class="gu-text">{move || md(&text.get())}</p> }.into_any()
        }
        None => ().into_any(),
    }
}

/// The same, all at once, for things nested in a leaf (a Card in a table cell).
fn draw(g: G, v: &Val) -> AnyView {
    match v {
        Val::Node(n) if lang::child_slot(&n.name).is_some() => {
            let kids = n.kids().iter().map(|k| draw(g, k)).collect();
            let n2 = n.clone();
            container(g, &n.name, Signal::derive(move || Some(n2.clone())), kids)
        }
        Val::Node(n) if lang::is_input(&n.name) => {
            let n2 = n.clone();
            input(g, &n.name, Signal::derive(move || Some(n2.clone())))
        }
        Val::Node(n) => leaf(g, n),
        v => view! { <span>{md(&v.text())}</span> }.into_any(),
    }
}

fn prop<T: Send + Sync + 'static>(node: NodeSig, f: impl Fn(&Node) -> T + Send + Sync + 'static, d: T) -> impl Fn() -> T + Copy + Send + Sync
where
    T: Clone,
{
    let f = StoredValue::new((f, d));
    move || f.with_value(|(f, d)| node.with(|n| n.as_deref().map(f)).unwrap_or_else(|| d.clone()))
}

fn container(g: G, name: &str, node: NodeSig, kids: Vec<AnyView>) -> AnyView {
    match name {
        "Card" => {
            let title = prop(node, |n| n.title(), String::new());
            view! {
                <section class="gu-card">
                    {move || { let t = title(); (!t.is_empty()).then(|| view! { <header><b class="gu-title">{t}</b></header> }) }}
                    {kids}
                </section>
            }
            .into_any()
        }
        "Row" => view! { <div class="gu-row">{kids}</div> }.into_any(),
        "Grid" => {
            let cols = prop(node, |n| n.num(1, "columns").map(|c| c.clamp(1.0, 12.0) as u32), None);
            view! { <div class="gu-grid" style=move || cols().map(|c| format!("grid-template-columns:repeat({c},minmax(0,1fr))")).unwrap_or_default()>{kids}</div> }.into_any()
        }
        "Details" => {
            let title = prop(node, |n| n.title(), String::new());
            let open = node.with_untracked(|n| n.as_ref().is_some_and(|n| n.get(2, "open").truthy()));
            view! {
                <details class="gu-details" open=open>
                    <summary class="gu-sum"><Icon name="chevron-right" size=12/>{title}</summary>
                    <div class="gu-stack">{kids}</div>
                </details>
            }
            .into_any()
        }
        "Tabs" => {
            let labels = prop(node, |n| lang::flatten(n.get(0, "labels")).iter().map(Val::text).collect::<Vec<_>>(), vec![]);
            let sel = RwSignal::new(0usize);
            view! {
                <div class="gu-tabs">
                    <div class="gu-tabbar" role="tablist">
                        {move || labels().into_iter().enumerate().map(|(i, l)| view! {
                            <button role="tab" aria-selected=move || (sel.get() == i).to_string() on:click=move |_| sel.set(i)>{l}</button>
                        }).collect_view()}
                    </div>
                    {kids.into_iter().enumerate().map(|(i, k)| view! { <div class="gu-stack" role="tabpanel" hidden=move || sel.get() != i>{k}</div> }).collect_view()}
                </div>
            }
            .into_any()
        }
        "Form" => {
            let submit = prop(node, |n| n.str(1, "submit"), String::new());
            let send = move |_| {
                let Some(n) = node.get_untracked() else { return };
                let mut lines = vec![];
                g.state.with_untracked(|s| answers(&n.kids(), s, &mut lines));
                if lines.is_empty() {
                    toast_bad("Nothing to send yet.");
                } else {
                    send_text(lines.join("\n"));
                }
            };
            view! {
                <div class="gu-form gu-stack">
                    {kids}
                    <div><button class="btn primary" on:click=send>{move || { let s = submit(); if s.is_empty() { "Send".to_string() } else { s } }}</button></div>
                </div>
            }
            .into_any()
        }
        _ => view! { <div class="gu-stack">{kids}</div> }.into_any(),
    }
}

/// "Label: value" for every input inside a Form.
fn answers(kids: &[Val], s: &State, out: &mut Vec<String>) {
    for k in kids {
        let Val::Node(n) = k else { continue };
        if lang::is_input(&n.name) {
            let Some(b) = n.bind(0) else { continue };
            let label = input_label(n);
            let v = s.get(b).map(|v| match v {
                Val::List(l) => l.iter().map(Val::text).collect::<Vec<_>>().join(", "),
                v => v.text(),
            });
            out.push(format!("{}: {}", if label.is_empty() { b.to_string() } else { label }, v.unwrap_or_default()));
        } else {
            answers(&n.kids(), s, out);
        }
    }
}

fn input_label(n: &Node) -> String {
    match n.name.as_str() {
        "Slider" => n.str(4, "label"),
        "Stepper" => n.str(4, "label"),
        "Toggle" | "Switch" => n.str(1, "label"),
        "Choice" | "Radio" | "Select" | "Checklist" => n.str(2, "label"),
        _ => n.str(1, "label"),
    }
}

/// An option of a Choice/Select/Checklist: its value and its label.
fn options(n: &Node) -> Vec<(Val, String)> {
    lang::flatten(n.get(1, "options"))
        .into_iter()
        .map(|o| match &o {
            Val::Obj(_) => {
                let v = match o.field("value") {
                    Val::Null => o.field("label").clone(),
                    v => v.clone(),
                };
                let l = match o.field("label") {
                    Val::Null => v.text(),
                    l => l.text(),
                };
                (v, l)
            }
            v => (v.clone(), v.text()),
        })
        .collect()
}

fn input(g: G, name: &str, node: NodeSig) -> AnyView {
    let Some(bind) = node.with_untracked(|n| n.as_ref().and_then(|n| n.bind(0).map(str::to_string))) else {
        return view! { <div class="gu-error"><Icon name="alert" size=13/>{format!("{name} needs a $state first, like {name}($x, …)")}</div> }.into_any();
    };
    let label = prop(node, input_label, String::new());
    let b = StoredValue::new(bind);
    let get = move || g.state.with(|s| b.with_value(|b| s.get(b).cloned()).unwrap_or(Val::Null));
    let set = move |v: Val| g.state.update(|s| {
        s.insert(b.get_value(), v);
    });
    match name {
        "Slider" => {
            let min = prop(node, |n| n.num(1, "min").unwrap_or(0.0), 0.0);
            let max = prop(node, |n| n.num(2, "max").unwrap_or(100.0), 100.0);
            let step = prop(node, |n| n.num(3, "step").filter(|s| *s > 0.0), None);
            let step = move || {
                step().unwrap_or_else(|| {
                    let (a, z) = (min(), max());
                    if a.fract() == 0.0 && z.fract() == 0.0 && z - a >= 2.0 { 1.0 } else { ((z - a) / 100.0).max(f64::MIN_POSITIVE) }
                })
            };
            let val = move || get().as_num().unwrap_or_else(min);
            view! {
                <label class="gu-field gu-slider">
                    <span class="gu-flabel"><span>{label}</span><b>{move || lang::fmt_num(val(), None)}</b></span>
                    <input type="range" prop:min=move || min().to_string() prop:max=move || max().to_string() prop:step=move || step().to_string() prop:value=move || val().to_string()
                        on:input={move |e| if let Ok(x) = event_target_value(&e).parse::<f64>() { set(Val::Num(x)) }}/>
                </label>
            }
            .into_any()
        }
        "Stepper" => {
            let min = prop(node, |n| n.num(1, "min"), None);
            let max = prop(node, |n| n.num(2, "max"), None);
            let step = prop(node, |n| n.num(3, "step").filter(|s| *s > 0.0).unwrap_or(1.0), 1.0);
            let val = move || get().as_num().unwrap_or(0.0);
            let bump = move |d: f64| {
                let mut x = val() + d * step();
                if let Some(a) = min() {
                    x = x.max(a);
                }
                if let Some(z) = max() {
                    x = x.min(z);
                }
                set(Val::Num(x));
            };
            view! {
                <div class="gu-field">
                    <span class="gu-flabel"><span>{label}</span></span>
                    <div class="gu-stepper">
                        <button class="btn" aria-label="Less" on:click=move |_| bump(-1.0)>"−"</button>
                        <b>{move || lang::fmt_num(val(), None)}</b>
                        <button class="btn" aria-label="More" on:click=move |_| bump(1.0)>"+"</button>
                    </div>
                </div>
            }
            .into_any()
        }
        "Toggle" | "Switch" => view! {
            <label class="gu-toggle">
                <input type="checkbox" role="switch" prop:checked=move || get().truthy() on:change=move |e| set(Val::Bool(event_target_checked(&e)))/>
                <span class="gu-switch"/>
                <span>{label}</span>
            </label>
        }
        .into_any(),
        "Choice" | "Radio" => {
            let opts = prop(node, options, vec![]);
            view! {
                <div class="gu-field">
                    {move || { let l = label(); (!l.is_empty()).then(|| view! { <span class="gu-flabel"><span>{l}</span></span> }) }}
                    <div class="gu-seg" role="radiogroup">
                        {move || opts().into_iter().map(|(v, l)| {
                            let v2 = v.clone();
                            view! { <button role="radio" aria-checked=move || (get() == v2).to_string() on:click=move |_| set(v.clone())>{l}</button> }
                        }).collect_view()}
                    </div>
                </div>
            }
            .into_any()
        }
        "Select" => {
            let opts = prop(node, options, vec![]);
            view! {
                <label class="gu-field">
                    {move || { let l = label(); (!l.is_empty()).then(|| view! { <span class="gu-flabel"><span>{l}</span></span> }) }}
                    <select prop:value=move || { let cur = get(); opts().iter().position(|(v, _)| *v == cur).map(|i| i.to_string()).unwrap_or_default() }
                        on:change={move |e| if let Some((v, _)) = event_target_value(&e).parse::<usize>().ok().and_then(|i| opts().get(i).cloned()) { set(v) }}>
                        {move || opts().into_iter().enumerate().map(|(i, (_, l))| view! { <option value=i.to_string()>{l}</option> }).collect_view()}
                    </select>
                </label>
            }
            .into_any()
        }
        "Checklist" => {
            let opts = prop(node, options, vec![]);
            view! {
                <div class="gu-field">
                    {move || { let l = label(); (!l.is_empty()).then(|| view! { <span class="gu-flabel"><span>{l}</span></span> }) }}
                    {move || opts().into_iter().map(|(v, l)| {
                        let (v2, v3) = (v.clone(), v.clone());
                        view! {
                            <label class="gu-check">
                                <input type="checkbox" prop:checked=move || get().as_list().is_some_and(|l| l.contains(&v2))
                                    on:change=move |e| {
                                        let mut cur = get().as_list().map(<[Val]>::to_vec).unwrap_or_default();
                                        cur.retain(|x| *x != v3);
                                        if event_target_checked(&e) { cur.push(v3.clone()) }
                                        set(Val::List(cur))
                                    }/>
                                <span>{l}</span>
                            </label>
                        }
                    }).collect_view()}
                </div>
            }
            .into_any()
        }
        // TextInput, Input, TextArea
        _ => {
            let ph = prop(node, |n| n.str(2, "placeholder"), String::new());
            let area = name == "TextArea";
            view! {
                <label class="gu-field">
                    {move || { let l = label(); (!l.is_empty()).then(|| view! { <span class="gu-flabel"><span>{l}</span></span> }) }}
                    {if area {
                        view! { <textarea rows="3" placeholder=ph prop:value=move || get().text() on:input=move |e| set(Val::Str(event_target_value(&e)))/> }.into_any()
                    } else {
                        view! { <input type="text" placeholder=ph prop:value=move || get().text() on:input=move |e| set(Val::Str(event_target_value(&e)))/> }.into_any()
                    }}
                </label>
            }
            .into_any()
        }
    }
}

// ───────────────────────────── actions ─────────────────────────────

/// Send text to the thread this block is in, as the person.
fn send_text(text: String) {
    let Some(thread) = agentum_ui::nav::read().get("thread").cloned().filter(|t| !t.is_empty()) else {
        toast_bad("Open the thread to send this.");
        return;
    };
    spawn_local(async move {
        if let Err(e) = api_post::<Value>(&format!("/code/threads/{thread}/send"), &json!({ "text": text })).await {
            toast_bad(e.message);
        }
    });
}

fn run(g: G, actions: &Val) {
    for a in lang::flatten(actions) {
        let Val::Node(n) = a else { continue };
        match n.name.as_str() {
            "Send" => send_text(n.str(0, "text")),
            "Set" => match n.bind(0) {
                Some(b) => {
                    let (b, v) = (b.to_string(), n.get(1, "value").clone());
                    g.state.update(|s| {
                        s.insert(b, v);
                    });
                }
                None => toast_bad("Set needs a $state first: Set($x, value)"),
            },
            "OpenUrl" => {
                let url = n.str(0, "url");
                if url.starts_with("https://") || url.starts_with("http://") {
                    if let Some(w) = web_sys::window() {
                        let _ = w.open_with_url_and_target(&url, "_blank");
                    }
                }
            }
            "Copy" => {
                if let Some(w) = web_sys::window() {
                    let _ = w.navigator().clipboard().write_text(&n.str(0, "text"));
                    toast("Copied.");
                }
            }
            "Error" => toast_bad(n.str(0, "message")),
            other => toast_bad(format!("{other} isn't an action (Send, Set, OpenUrl, Copy)")),
        }
    }
}

// ───────────────────────────── leaves ─────────────────────────────

fn tone_cls(base: &str, n: &Node, i: usize) -> String {
    format!("{base} tone-{}", n.tone(i))
}

fn icon_name(lucide: &str) -> &'static str {
    match lucide.to_lowercase().replace(['-', '_'], "").as_str() {
        "check" | "checkcircle" | "circlecheck" => "check",
        "x" | "xcircle" | "close" => "x",
        "plus" => "plus",
        "edit" | "pencil" => "edit",
        "copy" => "copy",
        "trash" | "trash2" => "trash",
        "download" => "download",
        "externallink" | "link" => "link",
        "play" | "zap" => "bolt",
        "rotateccw" | "refresh" | "refreshcw" => "refresh",
        "search" => "search",
        "settings" => "settings",
        "gitbranch" | "gitmerge" => "branch",
        "gitpullrequest" => "pr",
        "code" => "code",
        "terminal" => "terminal",
        "filetext" | "file" => "file",
        "folder" => "folder",
        "clock" => "clock",
        "calendar" => "calendar",
        "mail" | "sent" => "mail",
        "lock" => "lock",
        "star" => "star",
        "pin" => "pin",
        "eye" => "eye",
        "alerttriangle" | "bug" | "alert" => "alert",
        "info" | "help" => "help",
        "globe" | "cloud" => "globe",
        "layers" => "layers",
        "puzzle" => "puzzle",
        "laptop" | "smartphone" | "server" => "server",
        "user" => "user",
        "bot" => "bot",
        _ => "dot",
    }
}

fn leaf(g: G, n: &Node) -> AnyView {
    match n.name.as_str() {
        "Text" | "Heading" | "Markdown" => view! { <div class=tone_cls("gu-text", n, 1)>{md_block(&n.str(0, "text"))}</div> }.into_any(),
        "Callout" => {
            let title = n.str(2, "title");
            view! {
                <div class=tone_cls("gu-callout", n, 1)>
                    {(!title.is_empty()).then(|| view! { <b>{title}</b> })}
                    <div>{md_block(&n.str(0, "text"))}</div>
                </div>
            }
            .into_any()
        }
        "Metric" | "Stat" => {
            let note = n.str(2, "note");
            view! {
                <div class=tone_cls("gu-metric", n, 3)>
                    <small>{n.str(0, "label")}</small>
                    <b>{n.str(1, "value")}</b>
                    {(!note.is_empty()).then(|| view! { <span class="gu-note">{note}</span> })}
                </div>
            }
            .into_any()
        }
        "Badge" => view! { <span class=tone_cls("gu-badge", n, 1)>{n.str(0, "text")}</span> }.into_any(),
        "Icon" => view! { <span class=tone_cls("gu-icon", n, 1)><Icon name=icon_name(&n.str(0, "name"))/></span> }.into_any(),
        "Divider" => view! { <hr class="gu-hr"/> }.into_any(),
        "Code" => view! { <pre class="gu-code" data-lang=n.str(1, "language")><code>{n.str(0, "code")}</code></pre> }.into_any(),
        "KeyValue" => {
            let pairs: Vec<(String, Val)> = match n.get(0, "pairs") {
                Val::Obj(o) => o.clone(),
                Val::List(l) => l
                    .iter()
                    .filter_map(|p| match p {
                        Val::List(kv) if !kv.is_empty() => Some((kv[0].text(), kv.get(1).cloned().unwrap_or(Val::Null))),
                        Val::Obj(_) => Some(((match p.field("key") { Val::Null => p.field("label"), k => k }).text(), p.field("value").clone())),
                        _ => None,
                    })
                    .collect(),
                _ => vec![],
            };
            view! { <dl class="gu-kv">{pairs.into_iter().map(|(k, v)| view! { <dt>{k}</dt><dd>{draw(g, &v)}</dd> }).collect_view()}</dl> }.into_any()
        }
        "Table" => {
            let cols: Vec<String> = lang::flatten(n.get(0, "columns")).iter().map(Val::text).collect();
            let rows = n.get(1, "rows").as_list().map(<[Val]>::to_vec).unwrap_or_default();
            view! {
                <div class="gu-table"><table>
                    <thead><tr>{cols.iter().map(|c| view! { <th>{c.clone()}</th> }).collect_view()}</tr></thead>
                    <tbody>{rows.into_iter().map(|r| {
                        let cells: Vec<Val> = match &r {
                            Val::List(l) => l.clone(),
                            Val::Obj(_) => cols.iter().map(|c| match r.field(c) { Val::Null => r.field(&c.to_lowercase()).clone(), v => v.clone() }).collect(),
                            v => vec![v.clone()],
                        };
                        view! { <tr>{cells.iter().map(|c| view! { <td>{draw(g, c)}</td> }).collect_view()}</tr> }
                    }).collect_view()}</tbody>
                </table></div>
            }
            .into_any()
        }
        "List" => {
            let items = lang::flatten(n.get(0, "items"));
            view! {
                <div class="gu-list">{items.into_iter().map(|it| match &it {
                    Val::Obj(_) => {
                        let action = it.field("action").clone();
                        let clickable = !matches!(action, Val::Null);
                        let icon = it.field("icon").text();
                        let detail = it.field("detail").text();
                        let value = it.field("value").clone();
                        let tone = it.field("tone").text();
                        view! {
                            <div class=format!("gu-li tone-{}{}", if tone.is_empty() { "neutral" } else { &tone }, if clickable { " click" } else { "" })
                                on:click=move |_| if clickable { run(g, &action) }>
                                {(!icon.is_empty()).then(|| view! { <Icon name=icon_name(&icon) size=14 class="gu-li-icon"/> })}
                                <span class="gu-li-main"><span class="gu-li-title">{it.field("title").text()}</span>
                                    {(!detail.is_empty()).then(|| view! { <small>{detail}</small> })}</span>
                                {(!matches!(value, Val::Null)).then(|| view! { <span class="gu-li-value">{draw(g, &value)}</span> })}
                            </div>
                        }
                        .into_any()
                    }
                    v => view! { <div class="gu-li"><span class="gu-li-main">{draw(g, v)}</span></div> }.into_any(),
                }).collect_view()}</div>
            }
            .into_any()
        }
        "Steps" => {
            let items = lang::flatten(n.get(0, "items"));
            view! {
                <ol class="gu-steps">{items.into_iter().enumerate().map(|(i, it)| {
                    let (title, detail, status) = match &it {
                        Val::Obj(_) => (it.field("title").text(), it.field("detail").text(), it.field("status").text()),
                        v => (v.text(), String::new(), String::new()),
                    };
                    view! {
                        <li class=format!("st-{status}")>
                            <span class="dot">{if status == "done" { "✓".to_string() } else { (i + 1).to_string() }}</span>
                            <span class="ti">{md(&title)}{(!detail.is_empty()).then(|| view! { <small>{md(&detail)}</small> })}</span>
                        </li>
                    }
                }).collect_view()}</ol>
            }
            .into_any()
        }
        "Timeline" => {
            let items = lang::flatten(n.get(0, "items"));
            view! {
                <div class="gu-tl">{items.into_iter().map(|it| {
                    let detail = it.field("detail").text();
                    view! {
                        <div class=format!("gu-tl-i tone-{}", { let t = it.field("tone").text(); if t.is_empty() { "neutral".to_string() } else { t } })>
                            <span class="t">{it.field("time").text()}</span><i/>
                            <span class="ti">{it.field("title").text()}{(!detail.is_empty()).then(|| view! { <small>{detail}</small> })}</span>
                        </div>
                    }
                }).collect_view()}</div>
            }
            .into_any()
        }
        "Progress" => {
            let v = n.num(0, "value").unwrap_or(0.0);
            let pct = match n.num(1, "max") {
                Some(m) if m > 0.0 => v / m * 100.0,
                _ => v,
            }
            .clamp(0.0, 100.0);
            let label = n.str(2, "label");
            view! {
                <div class=tone_cls("gu-prog", n, 3)>
                    <small class="row"><span class="grow">{label}</span><span>{format!("{pct:.0}%")}</span></small>
                    <div role="progressbar" aria-valuenow=format!("{pct:.0}") aria-valuemin="0" aria-valuemax="100"><i style=format!("width:{pct:.1}%")/></div>
                </div>
            }
            .into_any()
        }
        "Chart" => chart(n),
        "Sparkline" => {
            let vals: Vec<f64> = lang::flatten(n.get(0, "values")).iter().filter_map(Val::as_num).collect();
            view! { <svg class=tone_cls("gu-spark", n, 1) viewBox="0 0 120 28" preserveAspectRatio="none"><polyline points=spark_points(&vals, 120.0, 28.0) fill="none" stroke="currentColor" stroke-width="1.6" vector-effect="non-scaling-stroke"/></svg> }.into_any()
        }
        "Button" => {
            let label = n.str(0, "label");
            let action = match n.get(1, "action") {
                Val::Null => Val::Node(Arc::new(Node { name: "Send".into(), args: vec![Val::Str(label.clone())], named: vec![], binds: vec![] })),
                a => a.clone(),
            };
            let cls = match n.str(2, "variant").as_str() {
                "primary" => "btn primary",
                "destructive" | "danger" => "btn danger",
                _ => "btn",
            };
            let icon = n.str(3, "icon");
            view! { <button class=cls on:click=move |_| run(g, &action)>{(!icon.is_empty()).then(|| view! { <Icon name=icon_name(&icon) size=14/> })}{label}</button> }.into_any()
        }
        "Send" | "Set" | "OpenUrl" | "Copy" => {
            let a = Val::Node(Arc::new(Node { name: n.name.clone(), args: n.args.clone(), named: n.named.clone(), binds: n.binds.clone() }));
            let label = if n.name == "Send" { n.str(0, "text") } else { n.name.clone() };
            view! { <button class="btn" on:click=move |_| run(g, &a)>{label}</button> }.into_any()
        }
        "FollowUps" => {
            let prompts: Vec<String> = lang::flatten(n.get(0, "prompts")).iter().map(Val::text).filter(|p| !p.is_empty()).collect();
            view! {
                <div class="gu-follow">{prompts.into_iter().map(|p| {
                    let p2 = p.clone();
                    view! { <button class="gu-fu" on:click=move |_| send_text(p2.clone())><Icon name="send" size=12/>{p}</button> }
                }).collect_view()}</div>
            }
            .into_any()
        }
        "Image" => {
            let src = n.str(0, "src");
            if src.starts_with("https://") || src.starts_with("data:image/") {
                view! { <img class="gu-img" src=src alt=n.str(1, "alt") loading="lazy"/> }.into_any()
            } else {
                unknown(g, n, Some(format!("Image · {src}")))
            }
        }
        "Error" => view! { <div class="gu-error"><Icon name="alert" size=13/>{n.str(0, "message")}</div> }.into_any(),
        _ => unknown(g, n, None),
    }
}

/// A component Agentum doesn't draw: a muted box with its name (and any components it holds).
fn unknown(g: G, n: &Node, label: Option<String>) -> AnyView {
    let inner: Vec<Val> = n.args.iter().chain(n.named.iter().map(|(_, v)| v)).flat_map(lang::flatten).filter(|v| matches!(v, Val::Node(_))).collect();
    view! {
        <div class="gu-unknown" title=format!("Agentum doesn't draw {} yet", n.name)>
            <span class="gu-unknown-name"><Icon name="puzzle" size=12/>{label.unwrap_or_else(|| n.name.clone())}</span>
            {inner.iter().map(|v| draw(g, v)).collect_view()}
        </div>
    }
    .into_any()
}

// ───────────────────────────── charts ─────────────────────────────

fn spark_points(v: &[f64], w: f64, h: f64) -> String {
    if v.is_empty() {
        return String::new();
    }
    let (lo, hi) = v.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| (a.min(*x), b.max(*x)));
    let span = if hi > lo { hi - lo } else { 1.0 };
    let dx = if v.len() > 1 { w / (v.len() - 1) as f64 } else { 0.0 };
    v.iter().enumerate().map(|(i, x)| format!("{:.1},{:.1}", i as f64 * dx, h - 2.0 - (x - lo) / span * (h - 4.0))).collect::<Vec<_>>().join(" ")
}

/// Series as (name, values): a list of numbers is one unnamed series; {name: numbers} several.
fn series(v: &Val) -> Vec<(String, Vec<f64>)> {
    let nums = |v: &Val| lang::flatten(v).iter().map(|x| x.as_num().unwrap_or(0.0)).collect::<Vec<_>>();
    match v {
        Val::Obj(o) => o.iter().map(|(k, v)| (k.clone(), nums(v))).collect(),
        Val::List(l) if l.iter().all(|x| matches!(x, Val::List(_))) => l.iter().enumerate().map(|(i, x)| (format!("Series {}", i + 1), nums(x))).collect(),
        Val::List(l) if l.iter().all(|x| matches!(x, Val::Obj(_))) && !l.is_empty() => {
            // [{name, values|data}]
            l.iter().map(|x| (x.field("name").text(), nums(match x.field("values") { Val::Null => x.field("data"), v => v }))).collect()
        }
        v => vec![(String::new(), nums(v))],
    }
}

fn compact(x: f64) -> String {
    let a = x.abs();
    if a >= 1e9 {
        format!("{:.1}B", x / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", x / 1e6)
    } else if a >= 1e4 {
        format!("{:.0}k", x / 1e3)
    } else if a >= 1e3 {
        format!("{:.1}k", x / 1e3)
    } else {
        lang::fmt_num(x, if a < 10.0 && x.fract() != 0.0 { Some(1) } else { Some(0) })
    }
}

fn chart(n: &Node) -> AnyView {
    let kind = n.str(0, "type");
    let labels: Vec<String> = lang::flatten(n.get(1, "labels")).iter().map(Val::text).collect();
    let ser = series(n.get(2, "series"));
    let unit = n.str(3, "unit");
    let stacked = n.get(4, "stacked").truthy();
    if ser.iter().all(|(_, v)| v.is_empty()) {
        return view! { <div class="gu-unknown"><span class="gu-unknown-name">"Chart: no numbers"</span></div> }.into_any();
    }
    let legend = (ser.len() > 1 || matches!(kind.as_str(), "pie" | "donut")).then(|| {
        let names: Vec<String> = if matches!(kind.as_str(), "pie" | "donut") { labels.clone() } else { ser.iter().map(|s| s.0.clone()).collect() };
        view! { <div class="gu-legend">{names.into_iter().enumerate().map(|(i, l)| view! { <span><i class=format!("c{}", i % 6)/>{l}</span> }).collect_view()}</div> }
    });
    let svg = match kind.as_str() {
        "pie" | "donut" => pie(&labels, &ser[0].1, &unit),
        "line" | "area" => line(&labels, &ser, &unit, kind == "area"),
        _ => bars(&labels, &ser, &unit, stacked),
    };
    view! { <figure class="gu-chart">{svg}{legend}</figure> }.into_any()
}

const W: f64 = 360.0;
const H: f64 = 170.0;
const L: f64 = 36.0;
const B: f64 = 20.0;
const T: f64 = 8.0;

fn nice_max(m: f64) -> f64 {
    if m <= 0.0 {
        return 1.0;
    }
    let p = 10f64.powf(m.log10().floor());
    let f = m / p;
    (if f <= 1.0 { 1.0 } else if f <= 2.0 { 2.0 } else if f <= 2.5 { 2.5 } else if f <= 5.0 { 5.0 } else { 10.0 }) * p
}

fn axes(lo: f64, hi: f64, labels: &[String], n: usize, unit: &str, centered: bool) -> AnyView {
    let pw = W - L - 6.0;
    let ph = H - T - B;
    let y = move |v: f64| T + ph * (1.0 - (v - lo) / (hi - lo));
    let every = (n / 8).max(1);
    view! {
        <g class="gu-axis">
            {[lo, (lo + hi) / 2.0, hi].into_iter().map(|v| view! {
                <line x1=L x2=W - 6.0 y1=y(v) y2=y(v)/>
                <text x=L - 4.0 y=y(v) + 3.0 text-anchor="end">{format!("{}{}", compact(v), if unit.len() <= 2 { unit.to_string() } else { String::new() })}</text>
            }).collect_view()}
            {labels.iter().take(n).enumerate().filter(|(i, _)| i % every == 0).map(|(i, l)| {
                let x = if centered { L + pw * (i as f64 + 0.5) / n as f64 } else if n > 1 { L + pw * i as f64 / (n - 1) as f64 } else { L + pw / 2.0 };
                let l: String = if l.chars().count() > 10 { l.chars().take(9).collect::<String>() + "…" } else { l.clone() };
                view! { <text x=x y=H - 6.0 text-anchor="middle">{l}</text> }
            }).collect_view()}
        </g>
    }
    .into_any()
}

fn bars(labels: &[String], ser: &[(String, Vec<f64>)], unit: &str, stacked: bool) -> AnyView {
    let n = labels.len().max(ser.iter().map(|s| s.1.len()).max().unwrap_or(0)).max(1);
    let at = |s: usize, i: usize| ser[s].1.get(i).copied().unwrap_or(0.0);
    let (mut lo, mut hi) = (0.0f64, 0.0f64);
    for i in 0..n {
        if stacked {
            hi = hi.max((0..ser.len()).map(|s| at(s, i).max(0.0)).sum());
            lo = lo.min((0..ser.len()).map(|s| at(s, i).min(0.0)).sum());
        } else {
            for s in 0..ser.len() {
                hi = hi.max(at(s, i));
                lo = lo.min(at(s, i));
            }
        }
    }
    let hi = nice_max(hi);
    let lo = if lo < 0.0 { -nice_max(-lo) } else { 0.0 };
    let pw = W - L - 6.0;
    let ph = H - T - B;
    let y = |v: f64| T + ph * (1.0 - (v - lo) / (hi - lo));
    let gw = pw / n as f64;
    let k = ser.len().max(1) as f64;
    let bw = if stacked { gw * 0.62 } else { gw * 0.74 / k };
    let mut rects = vec![];
    for i in 0..n {
        let mut base_pos = 0.0;
        let mut base_neg = 0.0;
        for (s, (name, _)) in ser.iter().enumerate() {
            let v = at(s, i);
            let (a, b) = if stacked {
                let start = if v >= 0.0 { base_pos } else { base_neg };
                if v >= 0.0 { base_pos += v } else { base_neg += v }
                (start, start + v)
            } else {
                (0.0, v)
            };
            let x = if stacked { L + gw * i as f64 + (gw - bw) / 2.0 } else { L + gw * i as f64 + gw * 0.13 + bw * s as f64 };
            let (y0, y1) = (y(a.max(b)), y(a.min(b)));
            let label = labels.get(i).cloned().unwrap_or_default();
            let tip = format!("{label}{}: {}{}", if name.is_empty() { String::new() } else { format!(" · {name}") }, lang::fmt_num(v, None), if unit.is_empty() { String::new() } else { format!(" {unit}") });
            rects.push(view! { <rect class=format!("c{}", s % 6) x=format!("{x:.1}") y=format!("{y0:.1}") width=format!("{:.1}", bw.max(1.0)) height=format!("{:.1}", (y1 - y0).max(0.5)) rx="2"><title>{tip}</title></rect> });
        }
    }
    view! {
        <svg viewBox=format!("0 0 {W} {H}") role="img">
            {axes(lo, hi, labels, n, unit, true)}
            {rects}
        </svg>
    }
    .into_any()
}

fn line(labels: &[String], ser: &[(String, Vec<f64>)], unit: &str, area: bool) -> AnyView {
    let n = labels.len().max(ser.iter().map(|s| s.1.len()).max().unwrap_or(0)).max(1);
    let all = ser.iter().flat_map(|s| s.1.iter().copied());
    let (lo, hi) = all.fold((0.0f64, 0.0f64), |(a, b), x| (a.min(x), b.max(x)));
    let hi = nice_max(hi);
    let lo = if lo < 0.0 { -nice_max(-lo) } else { 0.0 };
    let pw = W - L - 6.0;
    let ph = H - T - B;
    let x = |i: usize| if n > 1 { L + pw * i as f64 / (n - 1) as f64 } else { L + pw / 2.0 };
    let y = |v: f64| T + ph * (1.0 - (v - lo) / (hi - lo));
    let paths = ser.iter().enumerate().map(|(s, (name, v))| {
        let pts: Vec<(f64, f64)> = v.iter().enumerate().map(|(i, val)| (x(i), y(*val))).collect();
        let line: String = pts.iter().map(|(a, b)| format!("{a:.1},{b:.1}")).collect::<Vec<_>>().join(" ");
        let fill = (area && !pts.is_empty()).then(|| {
            let poly = format!("{:.1},{:.1} {line} {:.1},{:.1}", pts[0].0, y(lo.max(0.0)), pts[pts.len() - 1].0, y(lo.max(0.0)));
            view! { <polygon class=format!("c{} gu-area", s % 6) points=poly/> }
        });
        let dots = (pts.len() <= 40).then(|| pts.iter().enumerate().map(|(i, (a, b))| {
            let tip = format!("{}{}: {}{}", labels.get(i).cloned().unwrap_or_default(), if name.is_empty() { String::new() } else { format!(" · {name}") }, lang::fmt_num(v[i], None), if unit.is_empty() { String::new() } else { format!(" {unit}") });
            view! { <circle class=format!("c{}", s % 6) cx=format!("{a:.1}") cy=format!("{b:.1}") r="2.4"><title>{tip}</title></circle> }
        }).collect_view());
        view! { <g>{fill}<polyline class=format!("c{} gu-line", s % 6) points=line fill="none"/>{dots}</g> }
    }).collect_view();
    view! {
        <svg viewBox=format!("0 0 {W} {H}") role="img">
            {axes(lo, hi, labels, n, unit, false)}
            {paths}
        </svg>
    }
    .into_any()
}

fn pie(labels: &[String], v: &[f64], unit: &str) -> AnyView {
    let total: f64 = v.iter().filter(|x| **x > 0.0).sum();
    let r = 52.0;
    let c = 2.0 * std::f64::consts::PI * r;
    let mut off = 0.0;
    let slices = v.iter().enumerate().filter(|(_, x)| **x > 0.0).map(|(i, x)| {
        let len = if total > 0.0 { x / total * c } else { 0.0 };
        let tip = format!("{}: {}{} ({:.0}%)", labels.get(i).cloned().unwrap_or_default(), lang::fmt_num(*x, None), if unit.is_empty() { String::new() } else { format!(" {unit}") }, x / total * 100.0);
        let el = view! {
            <circle class=format!("c{} gu-slice", i % 6) cx="80" cy="80" r=r fill="none" stroke-width="30"
                stroke-dasharray=format!("{len:.2} {:.2}", c - len) stroke-dashoffset=format!("{:.2}", -off) transform="rotate(-90 80 80)"><title>{tip}</title></circle>
        };
        off += len;
        el
    }).collect_view();
    view! {
        <svg class="gu-pie" viewBox="0 0 160 160" role="img">
            {slices}
            <text x="80" y="84" text-anchor="middle" class="gu-pie-total">{compact(total)}</text>
        </svg>
    }
    .into_any()
}

// ───────────────────────────── text ─────────────────────────────

/// Paragraphs and bullet lists of inline markdown.
fn md_block(s: &str) -> AnyView {
    s.split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let lines: Vec<&str> = p.lines().collect();
            if lines.iter().all(|l| { let t = l.trim_start(); t.starts_with("- ") || t.starts_with("* ") }) {
                view! { <ul class="gu-bul">{lines.iter().map(|l| view! { <li>{md(l.trim_start()[2..].trim())}</li> }).collect_view()}</ul> }.into_any()
            } else {
                let n = lines.len();
                view! { <p>{lines.into_iter().enumerate().map(|(i, l)| view! { {md(l)}{(i + 1 < n).then(|| view! { <br/> })} }).collect_view()}</p> }.into_any()
            }
        })
        .collect_view()
        .into_any()
}

/// **bold**, *italic*, `code` and [links](https://…).
fn md(s: &str) -> AnyView {
    fn go(s: &str, depth: u8) -> Vec<AnyView> {
        let mut out: Vec<AnyView> = vec![];
        let mut text = String::new();
        let mut i = 0;
        let b = s.as_bytes();
        let flush = |text: &mut String, out: &mut Vec<AnyView>| {
            if !text.is_empty() {
                out.push(std::mem::take(text).into_any());
            }
        };
        while i < s.len() {
            let rest = &s[i..];
            if depth < 4 {
                if let Some(inner) = rest.strip_prefix("**").and_then(|r| r.find("**").map(|e| &r[..e])).filter(|x| !x.is_empty()) {
                    flush(&mut text, &mut out);
                    out.push(view! { <b>{go(inner, depth + 1)}</b> }.into_any());
                    i += inner.len() + 4;
                    continue;
                }
                if let Some(inner) = rest.strip_prefix('`').and_then(|r| r.find('`').map(|e| &r[..e])).filter(|x| !x.is_empty()) {
                    flush(&mut text, &mut out);
                    out.push(view! { <code>{inner.to_string()}</code> }.into_any());
                    i += inner.len() + 2;
                    continue;
                }
                if (b[i] == b'*' || b[i] == b'_') && rest.len() > 2 && !rest[1..].starts_with(' ') && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
                    let m = &rest[..1];
                    if let Some(e) = rest[1..].find(m).filter(|e| *e > 0) {
                        flush(&mut text, &mut out);
                        out.push(view! { <i>{go(&rest[1..1 + e], depth + 1)}</i> }.into_any());
                        i += e + 2;
                        continue;
                    }
                }
                if rest.starts_with('[') {
                    if let Some(close) = rest.find("](") {
                        if let Some(end) = rest[close + 2..].find(')') {
                            let (label, url) = (&rest[1..close], &rest[close + 2..close + 2 + end]);
                            if url.starts_with("https://") || url.starts_with("http://") || url.starts_with('/') {
                                flush(&mut text, &mut out);
                                out.push(view! { <a href=url.to_string() target="_blank" rel="noopener">{go(label, depth + 1)}</a> }.into_any());
                                i += close + 3 + end;
                                continue;
                            }
                        }
                    }
                }
            }
            let ch = rest.chars().next().unwrap_or(' ');
            text.push(ch);
            i += ch.len_utf8();
        }
        flush(&mut text, &mut out);
        out
    }
    go(s, 0).into_iter().collect_view().into_any()
}
