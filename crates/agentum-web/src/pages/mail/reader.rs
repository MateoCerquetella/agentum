//! Mail › the open email: actions, meta, attachments and the body in a sandboxed frame (no
//! scripts, no same-origin); pictures show, image attachments as thumbnails.
use super::lib::*;
use super::{use_mail, Reading};
use crate::pages::Pane;
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;

const PICS: [&str; 10] = ["image/png", "image/jpeg", "image/jpg", "image/gif", "image/webp", "image/avif", "image/bmp", "image/heic", "image/heif", "image/pjpeg"];

#[component]
pub fn Reader(r: Reading, on_close: Callback<()>) -> impl IntoView {
    let c = use_mail();
    let pane = NodeRef::<leptos::html::Div>::new();
    let m = RwSignal::new(None::<Full>);
    let err = RwSignal::new(String::new());
    let dark = RwSignal::new(None::<bool>);
    let theme = RwSignal::new(None::<(String, String, String)>);
    let item = r.item.clone();
    let set = r.set;
    let then = r.then;

    // Opening it reads it: the dot goes now; the server marks it read after answering.
    {
        let k = item.key();
        set.update(|ms| {
            if let Some(ms) = ms {
                for y in ms.iter_mut().filter(|y| y.key() == k) {
                    y.unread = false;
                }
            }
        });
    }
    hush();
    {
        let item = item.clone();
        spawn_local(async move {
            let url = format!("/mail/read?account={}&folder={}&uid={}", enc(&item.account), enc(&item.folder), item.uid);
            match api_get::<Value>(&url).await {
                Ok(v) => {
                    let full = Full::of(&v, &item);
                    // Frame look: theme colours from the pane itself (computed rgb).
                    let el: Option<web_sys::Element> = pane.get_untracked().map(Into::into).or_else(|| web_sys::window()?.document()?.body().map(Into::into));
                    let cs = el.and_then(|e| web_sys::window()?.get_computed_style(&e).ok().flatten());
                    let bg = cs.as_ref().and_then(|s| s.get_property_value("background-color").ok()).filter(|b| !b.is_empty() && b != "rgba(0, 0, 0, 0)").unwrap_or_else(|| token_rgb("bg"));
                    let fg = cs.as_ref().and_then(|s| s.get_property_value("color").ok()).unwrap_or_else(|| token_rgb("text"));
                    let _ = theme.try_set(Some((bg, fg, token_rgb("primary"))));
                    let _ = m.try_set(Some(full.clone()));
                    c.refresh_counts();
                    if let Some(t) = then {
                        let mut d = reply_draft(&full, if t == "ai" { "reply" } else { t }, &c.mine());
                        d.ai_draft = t == "ai";
                        c.compose(d);
                    }
                }
                Err(e) => {
                    let _ = err.try_set(e.message);
                }
            }
        });
    }

    let thread = r.thread.clone().unwrap_or_else(|| Thread { key: item.key(), m: Msg { account: item.account.clone(), folder: item.folder.clone(), uid: item.uid, ..Default::default() }, all: vec![Msg { account: item.account.clone(), folder: item.folder.clone(), uid: item.uid, ..Default::default() }] });
    let side = c.side;
    let act = {
        let item = item.clone();
        let thread = thread.clone();
        Callback::new(move |a: &'static str| {
            if matches!(a, "reply" | "all" | "forward") {
                if let Some(full) = m.get_untracked() {
                    c.compose(reply_draft(&full, a, &c.mine()));
                }
                return;
            }
            if a == "ask" {
                navigate("chat", if side == Pane::Right { Where::Left } else { Where::Right }, &[("q", "Summarize this email and suggest what I need to do."), ("ctx", &format!("mail:{}:{}:{}", item.account, item.uid, item.folder))]);
                return;
            }
            let thread = thread.clone();
            spawn_local(async move {
                if a == "delete" && !confirm("Delete this email?", "It goes to the account's Trash; the Rules activity log can undo it.", "Delete", true).await {
                    return;
                }
                if c.run(vec![thread], a, set, false).await {
                    if a == "mark_unread" {
                        toast("Marked unread");
                    }
                    on_close.run(());
                }
            });
        })
    };
    let prio = Signal::derive(move || if c.front.get() == "reader" { 2 } else { 1 });
    c.pane(
        "reader",
        prio,
        on_close,
        Some(Callback::new(move |k: String| {
            if m.with_untracked(Option::is_none) {
                return false;
            }
            let a = match k.as_str() {
                "r" => "reply",
                "a" => "all",
                "f" => "forward",
                "e" => "archive",
                "#" => "delete",
                "!" => "spam",
                "u" => "mark_unread",
                _ => return false,
            };
            act.run(a);
            true
        })),
    );

    let is_dark_now = move || dark.get().unwrap_or_else(|| theme.with(|t| t.as_ref().map(|t| is_dark(&t.0)).unwrap_or(true)));
    let look = move || {
        let (m, t) = (m.get()?, theme.get()?);
        let l = mail_look(&m, is_dark_now(), &Theme { bg: t.0, fg: t.1, link: t.2 });
        let csp = "default-src 'none'; style-src 'unsafe-inline'; img-src data: https: http:; font-src data:";
        // Links open in a new tab (base target; the sandbox lets only that popup out), without telling the site where you came from.
        let body = if m.body_html.is_empty() { format!("<pre style=\"white-space:pre-wrap;font:inherit\">{}</pre>", esc(&m.body)) } else { m.body_html.clone() };
        let doc = format!("<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\"><meta name=\"referrer\" content=\"no-referrer\"><base target=\"_blank\"><style>{}</style>{body}", l.css);
        Some((doc, format!("color-scheme:{};background:{}", l.scheme, l.frame_bg)))
    };
    let btn = move |k: &'static str, icon: &'static str, title: &'static str, label: Option<&'static str>| {
        view! {
            <button type="button" class=if label.is_some() { "btn ghost" } else { "icon-btn" } title=title aria-label=title on:click=move |_| act.run(k) disabled=move || m.with(Option::is_none)>
                <Icon name=icon size=15 class=if icon == "circle" { "fill" } else { "" }/>
                {label.map(|l| view! { <span class="ml-hide-narrow">{l}</span> })}
            </button>
        }
    };
    let account = item.account.clone();
    view! {
        <div class="ml-reader" node_ref=pane>
            <div class="ml-pbar">
                <button type="button" class="btn ghost" on:click=move |_| on_close.run(())><Icon name="chevronL" size=14/>"Back"</button>
                <span class="ml-rsubj grow truncate hi" title=move || m.with(|m| m.as_ref().map(|m| m.subject.clone()).unwrap_or_default())>{move || m.with(|m| m.as_ref().map(|m| m.subject.clone()).unwrap_or_default())}</span>
                <span class="ml-ractions">
                    <button type="button" class="btn ghost" title="Ask your local model about this email" on:click=move |_| act.run("ask")><Icon name="sparkle" size=14/>"Ask Agentum"</button>
                    {btn("reply", "reply", "Reply (r)", Some("Reply"))}
                    {btn("all", "replyAll", "Reply all (a)", Some("All"))}
                    {btn("forward", "forward", "Forward (f)", Some("Forward"))}
                    {btn("archive", "archive", "Archive (e)", None)}
                    {btn("mark_unread", "circle", "Mark unread (u)", None)}
                    {btn("spam", "spam", "Spam (!)", None)}
                    {btn("delete", "trash", "Delete (#)", None)}
                    <button type="button" class="icon-btn" disabled=move || m.with(Option::is_none) title=move || if is_dark_now() { "Show this email light" } else { "Show this email dark" } on:click=move |_| dark.set(Some(!is_dark_now()))>
                        <Icon name=Signal::derive(move || if is_dark_now() { "sun".to_string() } else { "moon".to_string() }) size=15/>
                    </button>
                </span>
            </div>
            {move || {
                if !err.get().is_empty() {
                    return view! { <div class="empty err">"Could not open: "{err.get()}</div> }.into_any();
                }
                let Some(full) = m.get() else { return view! { <div class="empty">"Loading…"</div> }.into_any() };
                // Pictures already inside the email (cid:) aren't listed again.
                let atts: Vec<Att> = full.attachments.iter().filter(|a| !a.embedded).cloned().collect();
                let pics: Vec<Att> = atts.iter().filter(|a| PICS.contains(&a.content_type.to_lowercase().as_str())).cloned().collect();
                let acct = c.accounts().into_iter().find(|a| a.id == account);
                let color = c.color(&account);
                let f2 = full.clone();
                view! {
                    <div class="ml-meta">
                        <div><b class="hi">{full.from_name.clone()}</b>" <"{full.from_address.clone()}"> "<span class="subdue" title=full.date.clone()>{long_when(&full.date)}</span></div>
                        <div class="subdue">
                            {acct.map(|a| view! { <span class="ml-acct" style=format!("--acct:{color}")>{a.name}</span> })}
                            {format!(" To: {}{}", full.to, if full.cc.is_empty() { String::new() } else { format!(" · Cc: {}", full.cc) })}
                        </div>
                    </div>
                    {(!atts.is_empty()).then(|| view! {
                        <div class="ml-atts">
                            {atts.iter().map(|a| view! {
                                <a class="ml-att" download=a.filename.clone() href=f2.att_url(a.index)><Icon name="paperclip" size=13/>{a.filename.clone()}" "<small>{kb(a.size)}</small></a>
                            }).collect_view()}
                        </div>
                    })}
                    {(!pics.is_empty()).then(|| view! {
                        <div class="ml-pics">
                            {pics.iter().map(|a| view! {
                                <a href=full.att_url(a.index) download=a.filename.clone() title=a.filename.clone()>
                                    <img src=full.att_url(a.index) alt=a.filename.clone() loading="lazy"
                                        on:error=|e| { if let Some(p) = event_target::<web_sys::HtmlElement>(&e).parent_element() { let _ = p.set_attribute("hidden", ""); } }/>
                                </a>
                            }).collect_view()}
                        </div>
                    })}
                    {move || look().map(|(doc, style)| view! {
                        <iframe class="ml-frame" title="Email" sandbox="allow-popups allow-popups-to-escape-sandbox" referrerpolicy="no-referrer" srcdoc=doc style=style/>
                    })}
                }.into_any()
            }}
        </div>
    }
}
