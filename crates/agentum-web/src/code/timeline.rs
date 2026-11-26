//! A thread's timeline: messages, thinking, tool calls with their output and diffs, plans,
//! questions, turn footers. Streaming text shows as it arrives; the view sticks to the bottom
//! unless you scrolled up.
use agentum_proto::api::{EventView, Partial, ThreadSummary};
use agentum_proto::{AskKind, AskOption, EventKind, FileDiff, PlanEntry, StopReason, ToolKind};
use agentum_ui::prelude::*;
use crate::code::rich::{rich_of, Rich};
use std::collections::{BTreeMap, BTreeSet};

/// A queued message: `steer` it into the running turn, or `drop` it.
fn queued_action(thread: &str, seq: u64, action: &'static str) {
    let url = format!("/code/threads/{thread}/queued/{seq}");
    spawn_local(async move {
        if let Err(e) = api_post::<serde_json::Value>(&url, &serde_json::json!({"action": action})).await {
            toast_bad(e.message);
        }
    });
}

#[derive(Clone, PartialEq)]
enum Item {
    User { seq: u64, html: String, queued: bool, ev: EventView },
    Message { seq: u64, html: String, ts: i64, ev: EventView },
    Thinking { seq: u64, text: String },
    Tool { seq: u64, item: String, name: String, kind: ToolKind, title: String, input: Value, done: Option<(bool, Option<String>, Vec<FileDiff>)>, cards: Vec<Rich> },
    /// A run of tool calls in a row, folded into one quiet line (as the Empirical Factory does).
    Work { seq: u64, tools: Vec<Item> },
    Plan { seq: u64, entries: Vec<PlanEntry> },
    Ask { seq: u64, id: String, kind: AskKind, title: String, detail: Option<String>, options: Vec<AskOption>, answer: Option<(String, Option<String>)> },
    /// `steps`: the tool calls in the turn, `failed` of them failing.
    Turn { seq: u64, stop: StopReason, ms: u64, cost: Option<f64>, tokens: u64, steps: usize, failed: usize },
    /// `full`: a limit window used up (its name and when it resets), when the error is a refused turn.
    Error { seq: u64, text: String, full: Option<(String, Option<i64>)> },
    Notice { seq: u64, text: String },
}
impl Item {
    fn key(&self) -> String {
        match self {
            Item::Tool { seq, done, .. } => format!("{seq}:{}", done.is_some()),
            // Stable while the run grows: the group is not redrawn, so its spinners don't restart.
            Item::Work { seq, .. } => format!("w{seq}"),
            Item::Ask { seq, answer, .. } => format!("{seq}:{}", answer.is_some()),
            Item::User { seq, queued, .. } => format!("{seq}:{queued}"),
            Item::Message { seq, .. } | Item::Thinking { seq, .. } | Item::Plan { seq, .. } | Item::Turn { seq, .. } | Item::Error { seq, .. } | Item::Notice { seq, .. } => seq.to_string(),
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `known`: the limits of the thread's account as last reported (until the thread reports its own).
fn fold(events: &[EventView], known: &[agentum_proto::RateLimit]) -> Vec<Item> {
    let mut limits: Vec<agentum_proto::RateLimit> = known.to_vec();
    let mut out: Vec<Item> = vec![];
    let mut tools: BTreeMap<String, usize> = BTreeMap::new();
    let mut asks: BTreeMap<String, usize> = BTreeMap::new();
    let mut users: BTreeMap<u64, usize> = BTreeMap::new();
    // A card already shown (the same test run or file list) isn't repeated further down.
    let mut shown: BTreeSet<String> = BTreeSet::new();
    let mut turn_start = 0;
    for e in events {
        let seq = e.event.seq;
        match &e.event.kind {
            EventKind::User { text, queued, .. } => {
                users.insert(seq, out.len());
                out.push(Item::User { seq, html: e.html.clone().unwrap_or_else(|| format!("<p>{}</p>", esc(text))), queued: *queued, ev: e.clone() });
            }
            EventKind::Dequeued { seq_of_user, dropped } => {
                if *dropped {
                    // A stop (or its Remove) dropped it: it was never sent.
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
                out.push(Item::Tool { seq, item: item.clone(), name: name.clone(), kind: *kind, title: title.clone(), input: input.clone(), done: None, cards: vec![] });
            }
            EventKind::ToolDone { item, ok, output, diffs } => {
                if let Some(Item::Tool { done, kind, cards, .. }) = tools.get(item).and_then(|i| out.get_mut(*i)) {
                    *done = Some((*ok, output.clone(), diffs.clone()));
                    // Only commands: a file that was read can look like a test run or a git log.
                    if *kind == ToolKind::Exec {
                        *cards = rich_of(output.as_deref().unwrap_or_default()).into_iter().filter(|r| shown.insert(format!("{r:?}"))).collect();
                    }
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
                let ran = || out.get(turn_start..).unwrap_or_default().iter().filter_map(|x| if let Item::Tool { done, .. } = x { Some(done) } else { None });
                let (steps, failed) = (ran().count(), ran().filter(|d| matches!(d, Some((false, ..)))).count());
                out.push(Item::Turn { seq, stop: *stop, ms: *duration_ms, cost: *cost_usd, tokens, steps, failed });
                turn_start = out.len();
            }
            EventKind::RateLimits { limits: l, .. } => limits = l.clone(),
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
    // Tool calls in a row become one group; thinking between them stays inside it.
    let mut grouped: Vec<Item> = Vec::with_capacity(out.len());
    for it in out {
        match (&it, grouped.last_mut()) {
            (Item::Tool { .. } | Item::Thinking { .. }, Some(Item::Work { tools, .. })) => tools.push(it),
            (Item::Tool { seq, .. }, _) => grouped.push(Item::Work { seq: *seq, tools: vec![it] }),
            _ => grouped.push(it),
        }
    }
    grouped
}

/// A command's program, for the group's line: `cd x; D=y; grep …` is "grep".
fn program(cmd: &str) -> String {
    cmd.split(['\n', ';', '&', '|', '('])
        .filter_map(|seg| {
            let w = seg.split_whitespace().next()?;
            (!matches!(w, "cd" | "set" | "export" | "then" | "do" | "else" | "fi" | "done") && !w.contains('=')).then_some(w)
        })
        .next()
        .map(|w| w.trim_matches(['"', '\'']).rsplit('/').next().unwrap_or(w).chars().take(20).collect())
        .unwrap_or_else(|| "command".into())
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
    let chipped = crate::code::rich::chips(html);
    let html = chipped.as_str();
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

/// "Worked · 5 steps · grep 3 · read 2 · 1 failed", which opens to the steps; the cards their output came
/// to (the newest of each kind) stay below it, open or not. It reads its steps from the timeline, so it
/// stays the same element while the run grows. Open while running, on its last few steps.
#[component]
fn WorkGroup(seq: u64, items: Memo<Vec<Item>>, thread: String, partial: RwSignal<BTreeMap<String, Partial>>, opened: RwSignal<BTreeSet<u64>>) -> impl IntoView {
    const TAIL: usize = 5;
    let thread = StoredValue::new(thread);
    let tools = Memo::new(move |_| {
        items.with(|v| v.iter().find_map(|x| match x {
            Item::Work { seq: s, tools } if *s == seq => Some(tools.clone()),
            _ => None,
        }))
        .unwrap_or_default()
    });
    let running = Memo::new(move |_| tools.with(|t| t.iter().any(|t| matches!(t, Item::Tool { done: None, .. }))));
    let failed = move || tools.with(|t| t.iter().filter(|t| matches!(t, Item::Tool { done: Some((false, ..)), .. })).count());
    Effect::new(move |_| {
        let diff = tools.with(|t| t.iter().any(|t| matches!(t, Item::Tool { done: Some((_, _, d)), .. } if !d.is_empty())));
        if diff && agentum_ui::store::flag("agentum.diffs.open") {
            opened.update(|o| {
                o.insert(seq);
            });
        }
    });
    let summary = move || {
        tools.with(|tools| {
            let mut kinds: Vec<(String, usize)> = vec![];
            for t in tools {
                let Item::Tool { kind, name, input, .. } = t else { continue };
                let word = match kind {
                    ToolKind::Read => "read".into(),
                    ToolKind::Edit => "edit".into(),
                    ToolKind::Exec => program(input["command"].as_str().unwrap_or(name)),
                    ToolKind::Search => "search".into(),
                    ToolKind::Fetch => "fetch".into(),
                    ToolKind::Task => "task".into(),
                    ToolKind::Other => name.to_lowercase(),
                };
                match kinds.iter_mut().find(|(w, _)| *w == word) {
                    Some((_, n)) => *n += 1,
                    None => kinds.push((word, 1)),
                }
            }
            let n = tools.iter().filter(|t| matches!(t, Item::Tool { .. })).count();
            kinds.sort_by_key(|k| std::cmp::Reverse(k.1));
            let what = kinds.into_iter().take(4).map(|(w, c)| if c > 1 { format!("{w} {c}") } else { w }).collect::<Vec<_>>().join(" · ");
            (n, what)
        })
    };
    let cards = move || {
        let mut cards: Vec<Rich> = vec![];
        tools.with(|t| {
            for t in t {
                let Item::Tool { cards: c, .. } = t else { continue };
                for r in c {
                    cards.retain(|x| std::mem::discriminant(x) != std::mem::discriminant(r));
                    cards.push(r.clone());
                }
            }
        });
        cards
    };
    let by_hand = move || opened.with(|o| o.contains(&seq));
    let open = move || running.get() || by_hand();
    // Open because it runs, not by you: only the newest steps.
    let hidden = move || if by_hand() { 0 } else { tools.with(|t| t.len().saturating_sub(TAIL)) };
    let toggle = move |_| {
        opened.update(|o| {
            if !o.remove(&seq) {
                o.insert(seq);
            }
        })
    };
    view! {
        <div class="work" class:open=open>
            <button class="work-head" on:click=toggle aria-expanded=move || open().to_string()>
                <Icon name="chevron-right" size=12 class="chev"/>
                <span>{move || { let (n, _) = summary(); format!("{} · {n} step{}", if running.get() { "Working" } else { "Worked" }, if n == 1 { "" } else { "s" }) }}</span>
                <span class="work-what truncate">{move || format!("· {}", summary().1)}</span>
                {move || (failed() > 0).then(|| view! { <span class="neg">{format!("· {} failed", failed())}</span> })}
                <Show when=move || running.get()><span class="spin-dot" aria-label="Running"/></Show>
            </button>
            <Show when=open>
                <div class="work-steps">
                    <Show when=move || hidden() != 0>
                        <button class="work-more" on:click=move |_| opened.update(|o| { o.insert(seq); })>{move || format!("{} earlier", hidden())}</button>
                    </Show>
                    <For each=move || { let h = hidden(); tools.with(|t| t[h.min(t.len())..].to_vec()) } key=|i| i.key() let:it>
                        {match it {
                            Item::Thinking { seq, text } => view! {
                                <details class="thinking" id=format!("ev-{seq}")><summary>"Thinking"</summary><div class="thinking-text">{text}</div></details>
                            }.into_any(),
                            it => {
                                let key = if let Item::Tool { item, .. } = &it { format!("tool:{item}") } else { String::new() };
                                let live = Signal::derive(move || partial.with(|p| p.get(&key).map(|x| x.text.clone())));
                                view! { <ToolRow item=it thread=thread.get_value() live=live/> }.into_any()
                            }
                        }}
                    </For>
                </div>
            </Show>
            {move || { let c = cards(); (!c.is_empty()).then(|| view! { <div class="rich-cards">{c.into_iter().map(|r| view! { <RichCard r=r/> }).collect_view()}</div> }) }}
        </div>
    }
}

/// What a command's output came to: a test run, changed files, commits, errors.
#[component]
fn RichCard(r: Rich) -> impl IntoView {
    let pct = |n: u32, of: u32| format!("width:{}%", n as f64 * 100.0 / of.max(1) as f64);
    match r {
        Rich::Tests { pass, fail, skip, files, secs, failures } => {
            let total = pass + fail + skip;
            let bad = fail > 0;
            let meta = format!("{total} tests{}{}", files.map(|f| format!(" · {f} files")).unwrap_or_default(), secs.filter(|s| *s > 0.0).map(|s| if s < 1.0 { format!(" · {}ms", (s * 1000.0).round()) } else if s < 10.0 { format!(" · {s:.1}s") } else { format!(" · {}s", s.round()) }).unwrap_or_default());
            view! {
                <div class="rich" class:bad=bad>
                    <div class="rich-head">
                        <Icon name=if bad { "alert" } else { "check-circle" } size=15 class=if bad { "neg" } else { "pos" }/>
                        <b>{if bad { format!("{fail} test{} failed", if fail == 1 { "" } else { "s" }) } else { "Tests passed".into() }}</b>
                        <span class="rich-meta">{meta}</span>
                    </div>
                    <div class="rich-bar" role="img" aria-label=format!("{pass} passed, {fail} failed, {skip} skipped")>
                        <span class="ok" style=pct(pass, total)/>
                        {bad.then(|| view! { <span class="bad" style=format!("{};min-width:2%", pct(fail, total))/> })}
                        {(skip > 0).then(|| view! { <span class="skip" style=pct(skip, total)/> })}
                    </div>
                    <div class="rich-counts">
                        <span>{format!("{pass} passed")}</span>
                        {bad.then(|| view! { <span class="neg">{format!("{fail} failed")}</span> })}
                        {(skip > 0).then(|| view! { <span>{format!("{skip} skipped")}</span> })}
                    </div>
                    {(!failures.is_empty()).then(|| view! {
                        <ul class="rich-list neg">{failures.into_iter().map(|f| view! { <li class="mono">{f}</li> }).collect_view()}</ul>
                    })}
                </div>
            }.into_any()
        }
        Rich::Changes { files, plus, minus } => {
            let max = files.iter().map(|f| f.plus.unwrap_or(0) + f.minus.unwrap_or(0)).max().unwrap_or(0).max(1);
            let more = files.len().saturating_sub(12);
            view! {
                <div class="rich">
                    <div class="rich-head">
                        <Icon name="file" size=15/>
                        <b>{format!("{} file{} changed", files.len(), if files.len() == 1 { "" } else { "s" })}</b>
                        {plus.map(|p| view! { <span class="rich-meta"><span class="pos">{format!("+{p}")}</span>" "<span class="neg">{format!("−{}", minus.unwrap_or(0))}</span></span> })}
                    </div>
                    <ul class="rich-files">{files.into_iter().take(12).map(|f| {
                        let n = f.plus.unwrap_or(0) + f.minus.unwrap_or(0);
                        view! {
                            <li>
                                <span class=format!("st st-{}", f.status)>{f.status.to_string()}</span>
                                <span class="mono truncate">{f.path}</span>
                                {f.plus.map(|p| view! {
                                    <span class="rich-meta"><span class="rich-bar small"><span class="ok" style=pct(p, max)/><span class="bad" style=pct(f.minus.unwrap_or(0), max)/></span>{n}</span>
                                })}
                            </li>
                        }
                    }).collect_view()}</ul>
                    {(more > 0).then(|| view! { <div class="rich-meta">{format!("and {more} more")}</div> })}
                </div>
            }.into_any()
        }
        Rich::Commits(commits) => view! {
            <div class="rich">
                <div class="rich-head"><Icon name="git" size=15/><b>{format!("{} commit{}", commits.len(), if commits.len() == 1 { "" } else { "s" })}</b></div>
                <ol class="rich-commits">{commits.into_iter().map(|(sha, subject)| view! {
                    <li><code class="chip-sha" title=sha.clone()>{sha.chars().take(7).collect::<String>()}</code>" "{subject}</li>
                }).collect_view()}</ol>
            </div>
        }.into_any(),
        Rich::Errors(errors) => view! {
            <div class="rich bad">
                <div class="rich-head"><Icon name="alert" size=15 class="neg"/><b>{format!("{} error{}", errors.len(), if errors.len() == 1 { "" } else { "s" })}</b></div>
                <ul class="rich-list mono">{errors.into_iter().map(|e| { let t = e.clone(); view! { <li class="truncate" title=t>{e}</li> } }).collect_view()}</ul>
            </div>
        }.into_any(),
    }
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
    // The account of the thread's machine: its limits say which window a refused turn hit.
    let ov = crate::code::store::overview();
    let known = Memo::new(move |_| {
        let (agent, machine) = summary.with(|s| s.as_ref().map(|s| (s.agent, s.machine.clone())))?;
        ov.with(|o| o.as_ref().and_then(|o| agentum_proto::api::limits_for(&o.limits, agent, &machine)).map(|l| l.limits.clone()))
    });
    let items = Memo::new(move |_| known.with(|k| events.with(|e| fold(e, k.as_deref().unwrap_or_default()))));
    let thread = move || summary.with(|s| s.as_ref().map(|s| s.id.clone()).unwrap_or_default());
    // The tool groups opened by hand (by their first event), kept while the timeline redraws.
    let opened = RwSignal::new(BTreeSet::<u64>::new());
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
                                {match &ev.event.kind {
                                    EventKind::User { images, .. } if !images.is_empty() => Some(view! {
                                        <div class="msg-imgs">{images.iter().filter(|i| i.starts_with("data:image/") || i.starts_with("https://")).map(|src| view! {
                                            { let big = RwSignal::new(false); view! { <img src=src.clone() alt="Attached image" loading="lazy" title="Click to enlarge" class:big=move || big.get() on:click=move |_| big.update(|b| *b = !*b)/> } }
                                        }).collect_view()}</div>
                                    }),
                                    _ => None,
                                }}
                                <div class="md" inner_html=html/>
                                <div class="msg-meta">{if queued {
                                    let (t1, t2) = (th.clone(), th.clone());
                                    view! {
                                        <span>"Queued"</span>
                                        <button class="q-act" title="Into the running turn now" on:click=move |_| queued_action(&t1, seq, "steer")><Icon name="bolt" size=12/>"Steer"</button>
                                        <button class="q-act" title="Take it out of the queue" on:click=move |_| queued_action(&t2, seq, "drop")><Icon name="x" size=12/>"Remove"</button>
                                    }.into_any()
                                } else { crate::plugins::message_meta(&ev).into_any() }}</div>
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
                        Item::Work { seq, .. } => view! { <WorkGroup seq=seq items=items thread=th partial=partial opened=opened/> }.into_any(),
                        Item::Tool { .. } => ().into_any(),
                        Item::Plan { entries, .. } => view! {
                            <div class="plan">{entries.into_iter().map(|e| view! {
                                <div class=format!("plan-item {}", e.status)><Icon name=match e.status.as_str() { "done" => "check", "in-progress" => "dot", _ => "circle" } size=12/>{e.text}</div>
                            }).collect_view()}</div>
                        }.into_any(),
                        it @ Item::Ask { .. } => view! { <AskCard item=it thread=th/> }.into_any(),
                        Item::Turn { stop, ms, cost, tokens, steps, failed, .. } => view! {
                            <div class="turn-foot small subdue">
                                {match stop { StopReason::End => "Done", StopReason::Interrupted => "Stopped", StopReason::Error => "Failed", StopReason::MaxTurns => "Hit the turn limit" }}
                                {(steps > 0).then(|| format!(" · {steps} step{}", if steps == 1 { "" } else { "s" }))}
                                {(failed > 0).then(|| view! { " · " <span class="neg">{format!("{failed} failed")}</span> })}
                                {(ms > 0).then(|| format!(" · {:.1}s", ms as f64 / 1000.0))}
                                {(tokens > 0).then(|| format!(" · {} tokens", agentum_ui::fmt::tokens(tokens)))}
                                {cost.filter(|c| *c > 0.0).map(|c| format!(" · ${c:.3}"))}
                            </div>
                        }.into_any(),
                        Item::Error { text, full, .. } => match full {
                            Some((window, resets)) => {
                                let when = resets.map(|r| format!(" It resets {}.", when_label(r))).unwrap_or_default();
                                view! {
                                    <div class="limit-card">
                                        <b>{move || format!("{}'s {window} limit is used up.", summary.with(|s| s.as_ref().map(|s| s.agent.label()).unwrap_or("The agent")))}</b>{when}
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
