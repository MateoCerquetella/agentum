//! A thread's timeline: messages, thinking, tool calls with their output and diffs, plans,
//! questions, turn footers. Streaming text shows as it arrives; the view sticks to the bottom
//! unless you scrolled up.
use agentum_proto::api::{EventView, Partial, ThreadSummary};
use agentum_proto::{AskKind, AskOption, EventKind, FileDiff, PlanEntry, StopReason, ToolKind};
use agentum_ui::prelude::*;
use std::collections::BTreeMap;

#[derive(Clone, PartialEq)]
enum Item {
    User { seq: u64, html: String, queued: bool, ev: EventView },
    Message { seq: u64, html: String, ts: i64, ev: EventView },
    Thinking { seq: u64, text: String },
    Tool { seq: u64, item: String, name: String, kind: ToolKind, title: String, input: Value, done: Option<(bool, Option<String>, Vec<FileDiff>)> },
    Plan { seq: u64, entries: Vec<PlanEntry> },
    Ask { seq: u64, id: String, kind: AskKind, title: String, detail: Option<String>, options: Vec<AskOption>, answer: Option<(String, Option<String>)> },
    Turn { seq: u64, stop: StopReason, ms: u64, cost: Option<f64>, tokens: u64 },
    /// `full`: a limit window used up (its name and when it resets), when the error is a refused turn.
    Error { seq: u64, text: String, full: Option<(String, Option<i64>)> },
    Notice { seq: u64, text: String },
}
impl Item {
    fn key(&self) -> String {
        match self {
            Item::Tool { seq, done, .. } => format!("{seq}:{}", done.is_some()),
            Item::Ask { seq, answer, .. } => format!("{seq}:{}", answer.is_some()),
            Item::User { seq, queued, .. } => format!("{seq}:{queued}"),
            Item::Message { seq, .. } | Item::Thinking { seq, .. } | Item::Plan { seq, .. } | Item::Turn { seq, .. } | Item::Error { seq, .. } | Item::Notice { seq, .. } => seq.to_string(),
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn fold(events: &[EventView]) -> Vec<Item> {
    let mut limits: Vec<agentum_proto::RateLimit> = vec![];
    let mut out: Vec<Item> = vec![];
    let mut tools: BTreeMap<String, usize> = BTreeMap::new();
    let mut asks: BTreeMap<String, usize> = BTreeMap::new();
    let mut users: BTreeMap<u64, usize> = BTreeMap::new();
    for e in events {
        let seq = e.event.seq;
        match &e.event.kind {
            EventKind::User { text, queued, .. } => {
                users.insert(seq, out.len());
                out.push(Item::User { seq, html: e.html.clone().unwrap_or_else(|| format!("<p>{}</p>", esc(text))), queued: *queued, ev: e.clone() });
            }
            EventKind::Dequeued { seq_of_user, dropped } => {
                if *dropped {
                    // A stop dropped it: it was never sent.
                    if let Some(i) = users.get(seq_of_user).copied() {
                        if let Some(Item::User { html, queued, .. }) = out.get_mut(i) {
                            *queued = false;
                            *html = format!("<s>{html}</s>");
                        }
                    }
                } else if let Some(Item::User { queued, .. }) = users.get(seq_of_user).and_then(|i| out.get_mut(*i)) {
                    *queued = false;
                }
            }
            EventKind::Message { text, .. } => out.push(Item::Message { seq, html: e.html.clone().unwrap_or_else(|| format!("<p>{}</p>", esc(text))), ts: e.event.ts, ev: e.clone() }),
            EventKind::Thinking { text, .. } => out.push(Item::Thinking { seq, text: text.clone() }),
            EventKind::Tool { item, name, kind, title, input } => {
                tools.insert(item.clone(), out.len());
                out.push(Item::Tool { seq, item: item.clone(), name: name.clone(), kind: *kind, title: title.clone(), input: input.clone(), done: None });
            }
            EventKind::ToolDone { item, ok, output, diffs } => {
                if let Some(Item::Tool { done, .. }) = tools.get(item).and_then(|i| out.get_mut(*i)) {
                    *done = Some((*ok, output.clone(), diffs.clone()));
                }
            }
            EventKind::Plan { entries } => {
                // One plan, kept current: the newest replaces the last.
                if let Some(i) = out.iter().rposition(|x| matches!(x, Item::Plan { .. })) {
                    out.remove(i);
                }
                out.push(Item::Plan { seq, entries: entries.clone() })
            }
            EventKind::Ask { id, kind, title, detail, options, .. } => {
                asks.insert(id.clone(), out.len());
                out.push(Item::Ask { seq, id: id.clone(), kind: *kind, title: title.clone(), detail: detail.clone(), options: options.clone(), answer: None });
            }
            EventKind::Answered { id, option, text } => {
                if let Some(Item::Ask { answer, .. }) = asks.get(id).and_then(|i| out.get_mut(*i)) {
                    *answer = Some((option.clone(), text.clone()));
                }
            }
            EventKind::TurnDone { stop, usage, cost_usd, duration_ms, .. } => {
                let tokens = usage.as_ref().map(|u| u.input + u.output + u.cache_read + u.cache_write).unwrap_or(0);
                out.push(Item::Turn { seq, stop: *stop, ms: *duration_ms, cost: *cost_usd, tokens })
            }
            EventKind::RateLimits { limits: l } => limits = l.clone(),
            EventKind::Error { message, .. } => {
                let m = message.to_lowercase();
                let refused = m.contains("429") || m.contains("usage limit") || m.contains("rate limit") || m.contains("too many requests") || m.contains("quota");
                let full = refused.then(|| limits.iter().filter(|w| w.used_percent >= 99.5).max_by_key(|w| w.resets_at.unwrap_or(0)).map(|w| (w.window.clone(), w.resets_at))).flatten();
                out.push(Item::Error { seq, text: message.clone(), full })
            }
            EventKind::Notice { text } => out.push(Item::Notice { seq, text: text.clone() }),
            _ => {}
        }
    }
    out
}

fn tool_icon(k: ToolKind) -> &'static str {
    match k {
        ToolKind::Read => "file",
        ToolKind::Edit => "edit",
        ToolKind::Exec => "terminal",
        ToolKind::Search => "search",
        ToolKind::Fetch => "globe",
        ToolKind::Task => "bot",
        ToolKind::Other => "tool",
    }
}

/// A unified diff, lines coloured.
#[component]
pub fn DiffView(diff: String) -> impl IntoView {
    view! {
        <pre class="diff">{diff.lines().map(|l| {
            let cls = if l.starts_with("+++") || l.starts_with("---") { "diff-file" } else if l.starts_with('+') { "diff-add" } else if l.starts_with('-') { "diff-del" } else if l.starts_with("@@") { "diff-hunk" } else { "" };
            view! { <span class=cls>{l.to_string()}"\n"</span> }
        }).collect_view()}</pre>
    }
}

fn diff_text(d: &FileDiff) -> String {
    if let Some(u) = &d.unified {
        return u.clone();
    }
    let mut s = format!("--- {}\n+++ {}\n", d.path, d.path);
    for l in d.old.as_deref().unwrap_or("").lines() {
        s += &format!("-{l}\n");
    }
    for l in d.new.as_deref().unwrap_or("").lines() {
        s += &format!("+{l}\n");
    }
    s
}

/// Message HTML, with plugin-drawn fences (```ui) cut out and drawn by their plugin.
fn message_body(html: &str) -> AnyView {
    const OPEN: &str = "<agentum-fence data-lang=\"";
    if !html.contains(OPEN) {
        return view! { <div class="md" inner_html=html.to_string()/> }.into_any();
    }
    let mut parts: Vec<AnyView> = vec![];
    let mut rest = html;
    while let Some(i) = rest.find(OPEN) {
        parts.push(view! { <div class="md" inner_html=rest[..i].to_string()/> }.into_any());
        let after = &rest[i + OPEN.len()..];
        let Some(q) = after.find("\">") else { break };
        let lang = &after[..q];
        let body = &after[q + 2..];
        let Some(end) = body.find("</agentum-fence>") else { break };
        let code = html_unescape(&body[..end]);
        parts.push(crate::plugins::code_fence(lang, &code).unwrap_or_else(|| view! { <pre><code>{code.clone()}</code></pre> }.into_any()));
        rest = &body[end + "</agentum-fence>".len()..];
    }
    parts.push(view! { <div class="md" inner_html=rest.to_string()/> }.into_any());
    parts.into_iter().collect_view().into_any()
}

fn html_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

fn input_summary(name: &str, input: &Value) -> String {
    for k in ["command", "file_path", "path", "pattern", "url", "query", "description", "prompt"] {
        if let Some(s) = input[k].as_str() {
            return s.chars().take(400).collect();
        }
    }
    if input.is_null() || input == &json!({}) {
        return name.to_string();
    }
    input.to_string().chars().take(400).collect()
}

#[component]
fn ToolRow(item: Item, thread: String, live: Signal<Option<String>>) -> impl IntoView {
    let Item::Tool { name, kind, title, input, done, .. } = item else { return ().into_any() };
    let open = RwSignal::new(false);
    let status = match &done {
        None => "tool-run",
        Some((true, ..)) => "tool-ok",
        Some((false, ..)) => "tool-bad",
    };
    let head = if title.is_empty() { name.clone() } else { title.clone() };
    // The argument only when the title doesn't already say it.
    let summary = Some(input_summary(&name, &input)).filter(|s| *s != head && !head.contains(s.as_str())).unwrap_or_default();
    let (output, diffs) = done.clone().map(|(_, o, d)| (o, d)).unwrap_or((None, vec![]));
    let has_diff = !diffs.is_empty();
    let _ = thread;
    view! {
        <div class=format!("tool {status}")>
            <button class="tool-head row" on:click=move |_| open.update(|o| *o = !*o)>
                <Icon name=tool_icon(kind) size=13/>
                <span class="tool-name">{head}</span>
                <span class="tool-arg truncate grow subdue">{summary}</span>
                {done.is_none().then(|| view! { <span class="spin-dot" aria-label="Running"/> })}
                {has_diff.then(|| view! { <span class="badge">"diff"</span> })}
            </button>
            <Show when=move || open.get() || (has_diff && agentum_ui::store::flag("agentum.diffs.open"))>
                {let o = output.clone(); let d = diffs.clone(); move || view! {
                    {d.iter().map(|x| view! { <DiffView diff=diff_text(x)/> }).collect_view()}
                    {o.clone().filter(|s| !s.is_empty()).map(|s| view! { <pre class="tool-out">{s}</pre> })}
                }}
            </Show>
            <Show when=move || done.is_none() && live.get().is_some()>
                <pre class="tool-out live">{move || live.get().unwrap_or_default()}</pre>
            </Show>
        </div>
    }.into_any()
}

#[component]
fn AskCard(item: Item, thread: String) -> impl IntoView {
    let Item::Ask { id, kind, title, detail, options, answer, .. } = item else { return ().into_any() };
    let text = RwSignal::new(String::new());
    let done = answer.clone();
    let is_done = done.is_some();
    let id2 = id.clone();
    let t2 = thread.clone();
    view! {
        <div class=if is_done { "ask-card done" } else { "ask-card" }>
            <div class="row" style="gap:6px"><Icon name=if kind == AskKind::Permission { "lock" } else { "help" }/><b>{title}</b></div>
            {detail.map(|d| view! { <pre class="ask-detail">{d}</pre> })}
            {match done {
                Some((opt, t)) => {
                    let label = options.iter().find(|o| o.id == opt).map(|o| o.label.clone()).unwrap_or(opt);
                    view! { <div class="small subdue">"Answered: "{label}{t.map(|t| format!(" · {t}"))}</div> }.into_any()
                }
                None => view! {
                    <div class="row wrap" style="gap:6px">
                        {options.into_iter().map(|o| {
                            let (th, ask, oid) = (thread.clone(), id.clone(), o.id.clone());
                            let cls = match o.kind.as_str() { "deny" => "btn danger", "allow" | "allow-always" => "btn primary", _ => "btn" };
                            view! { <button class=cls on:click=move |_| { let t = text.get_untracked(); super::answer(th.clone(), ask.clone(), oid.clone(), (!t.is_empty()).then_some(t)) }>{o.label}</button> }
                        }).collect_view()}
                    </div>
                    {(kind == AskKind::Question).then(move || view! {
                        <div class="row" style="gap:6px">
                            <input class="grow" placeholder="Or answer in your words" prop:value=text on:input=move |e| text.set(event_target_value(&e))
                                on:keydown={ let (th, ask) = (t2.clone(), id2.clone()); move |e: leptos::ev::KeyboardEvent| if e.key() == "Enter" { super::answer(th.clone(), ask.clone(), "other".into(), Some(text.get_untracked())) } }/>
                        </div>
                    })}
                }.into_any(),
            }}
        </div>
    }.into_any()
}

#[component]
pub fn Timeline(
    events: RwSignal<Vec<EventView>>,
    partial: RwSignal<BTreeMap<String, Partial>>,
    more_before: RwSignal<bool>,
    loaded: RwSignal<bool>,
    summary: Signal<Option<ThreadSummary>>,
    at: Signal<Option<String>>,
    on_older: Callback<()>,
) -> impl IntoView {
    let items = Memo::new(move |_| events.with(|e| fold(e)));
    let thread = move || summary.with(|s| s.as_ref().map(|s| s.id.clone()).unwrap_or_default());
    let scroller = NodeRef::<leptos::html::Div>::new();
    let stick = RwSignal::new(true);
    // Keep to the bottom while new things arrive, unless scrolled up.
    Effect::new(move |_| {
        items.track();
        partial.track();
        if !stick.get_untracked() {
            return;
        }
        if let Some(el) = scroller.get_untracked() {
            request_animation_frame(move || el.set_scroll_top(el.scroll_height()));
        }
    });
    // ?at=<seq>: scroll to that event (from search).
    Effect::new(move |_| {
        let Some(seq) = at.get() else { return };
        items.track();
        stick.set(false);
        request_animation_frame(move || {
            if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(&format!("ev-{seq}"))) {
                el.scroll_into_view();
                let _ = el.class_list().add_1("flash");
            }
        });
    });
    let on_scroll = move |_| {
        if let Some(el) = scroller.get_untracked() {
            stick.set(el.scroll_height() - el.scroll_top() - el.client_height() < 80);
        }
    };
    view! {
        <div class="timeline" node_ref=scroller on:scroll=on_scroll style="flex:1;min-height:0;overflow-y:auto;padding:12px 16px">
            <Show when=move || more_before.get()>
                <button class="btn small" style="margin:0 auto 8px;display:block" on:click=move |_| on_older.run(())>"Earlier"</button>
            </Show>
            <Show when=move || loaded.get() && items.with(|i| i.is_empty()) && partial.with(|p| p.is_empty())>
                <div class="empty">"Nothing here yet."</div>
            </Show>
            <For each=move || items.get() key=|i| i.key() let:item>
                {
                    let th = thread();
                    match item.clone() {
                        Item::User { seq, html, queued, ev } => view! {
                            <div class=if queued { "msg user queued" } else { "msg user" } id=format!("ev-{seq}")>
                                <div class="md" inner_html=html/>
                                <div class="msg-meta">{if queued { "Queued".into_any() } else { crate::plugins::message_meta(&ev).into_any() }}</div>
                            </div>
                        }.into_any(),
                        Item::Message { seq, html, ev, .. } => view! {
                            <div class="msg agent" id=format!("ev-{seq}")>
                                {message_body(&html)}
                                <div class="msg-meta">{crate::plugins::message_meta(&ev)}</div>
                            </div>
                        }.into_any(),
                        Item::Thinking { seq, text } => view! {
                            <details class="thinking" id=format!("ev-{seq}")><summary>"Thinking"</summary><div class="thinking-text">{text}</div></details>
                        }.into_any(),
                        it @ Item::Tool { .. } => {
                            let key = if let Item::Tool { item, .. } = &it { format!("tool:{item}") } else { String::new() };
                            let live = Signal::derive(move || partial.with(|p| p.get(&key).map(|x| x.text.clone())));
                            view! { <ToolRow item=it thread=th live=live/> }.into_any()
                        }
                        Item::Plan { entries, .. } => view! {
                            <div class="plan">{entries.into_iter().map(|e| view! {
                                <div class=format!("plan-item {}", e.status)><Icon name=match e.status.as_str() { "done" => "check", "in-progress" => "dot", _ => "circle" } size=12/>{e.text}</div>
                            }).collect_view()}</div>
                        }.into_any(),
                        it @ Item::Ask { .. } => view! { <AskCard item=it thread=th/> }.into_any(),
                        Item::Turn { stop, ms, cost, tokens, .. } => view! {
                            <div class="turn-foot small subdue">
                                {match stop { StopReason::End => "Done", StopReason::Interrupted => "Stopped", StopReason::Error => "Failed", StopReason::MaxTurns => "Hit the turn limit" }}
                                {(ms > 0).then(|| format!(" · {:.1}s", ms as f64 / 1000.0))}
                                {(tokens > 0).then(|| format!(" · {} tokens", agentum_ui::fmt::tokens(tokens)))}
                                {cost.filter(|c| *c > 0.0).map(|c| format!(" · ${c:.3}"))}
                            </div>
                        }.into_any(),
                        Item::Error { text, full, .. } => match full {
                            Some((window, resets)) => {
                                let agent = summary.with_untracked(|s| s.as_ref().map(|s| s.agent.label()).unwrap_or("The agent"));
                                let when = resets.map(|r| format!(" It resets {}.", when_label(r))).unwrap_or_default();
                                view! {
                                    <div class="limit-card">
                                        <b>{format!("{agent}'s {window} limit is used up.")}</b>{when}
                                        <div class="small subdue">"Switch to another agent in the model chip below, or send again after the reset. ("{text}")"</div>
                                    </div>
                                }.into_any()
                            }
                            None => view! { <div class="err small">{text}</div> }.into_any(),
                        },
                        Item::Notice { text, .. } => view! { <div class="small subdue notice">{text}</div> }.into_any(),
                    }
                }
            </For>
            {move || partial.get().into_iter().filter(|(k, p)| !k.starts_with("tool:") && !p.text.is_empty()).map(|(_, p)| {
                if p.thinking {
                    view! { <details class="thinking live" open><summary>"Thinking…"</summary><div class="thinking-text">{p.text}</div></details> }.into_any()
                } else {
                    view! { <div class="msg agent streaming"><div class="md streaming-text">{p.text}</div></div> }.into_any()
                }
            }).collect_view()}
            <Show when=move || summary.with(|s| s.as_ref().is_some_and(|s| s.state.busy())) && partial.with(|p| p.is_empty())>
                <div class="small subdue working-line"><span class="spin-dot"/>" Working…"</div>
            </Show>
        </div>
    }
}

/// "Thu 26 Nov 23:22" (local time) for milliseconds.
fn when_label(ms: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    const D: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {} {} {:02}:{:02}", D[d.get_day() as usize % 7], d.get_date(), M[d.get_month() as usize % 12], d.get_hours(), d.get_minutes())
}
