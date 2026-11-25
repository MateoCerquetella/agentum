//! Mail › Reply Zero, Clean, Cold email and Analytics tabs.
use super::inbox::{AccountSelect, Confirm, Row};
use super::lib::*;
use super::{use_mail, List};
use agentum_ui::prelude::*;
use std::collections::{BTreeSet, HashSet};

fn err_view(e: String) -> AnyView {
    view! { <div class="empty err">{e}</div> }.into_any()
}
fn loading() -> AnyView {
    view! { <div class="empty">"Loading…"</div> }.into_any()
}

// ---------------------------------------------------------- Reply Zero ----
#[component]
pub fn ReplyZero() -> impl IntoView {
    let c = use_mail();
    let to_reply: List = RwSignal::new(None);
    let awaiting: List = RwSignal::new(None);
    let err = RwSignal::new(String::new());
    let load = move || {
        spawn_local(async move {
            match api_get::<Value>("/mail/reply-zero").await {
                Ok(r) => {
                    let _ = to_reply.try_set(Some(Msg::list(&r["to_reply"])));
                    let aw = Msg::list(&r["awaiting"]).into_iter().map(|mut m| { m.from_name = format!("→ {}", m.to); m }).collect();
                    let _ = awaiting.try_set(Some(aw));
                }
                Err(e) => {
                    let _ = err.try_set(e.message);
                }
            }
        });
    };
    Effect::new(move |_| {
        c.ver.track();
        load();
    });
    c.use_rows(move || {
        let mut v: Vec<(Thread, List)> = to_reply.get().unwrap_or_default().iter().map(|m| (single(m), to_reply)).collect();
        v.extend(awaiting.get().unwrap_or_default().iter().map(|m| (single(m), awaiting)));
        v
    });
    let dismiss = move |ms: Vec<Msg>| {
        let ids: Vec<String> = ms.iter().map(|m| m.msgid.clone()).filter(|x| !x.is_empty()).collect();
        spawn_local(async move {
            let body = if ids.len() == 1 { json!({"msgid": ids[0]}) } else { json!({"msgids": ids}) };
            match api_post::<Value>("/mail/dismiss", &body).await {
                Ok(_) => {
                    let g: HashSet<String> = ids.iter().cloned().collect();
                    for l in [to_reply, awaiting] {
                        l.update(|x| {
                            if let Some(x) = x {
                                x.retain(|m| !g.contains(&m.msgid))
                            }
                        });
                    }
                    c.sel.set(BTreeSet::new());
                    let n = ids.len();
                    c.show_undo(format!("Dismissed {n}"), move || {
                        let ids = ids.clone();
                        spawn_local(async move {
                            match api_post::<Value>("/mail/undismiss", &json!({"msgids": ids})).await {
                                Ok(_) => load(),
                                Err(e) => toast_bad(e.message),
                            }
                        });
                    });
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let done = move |m: Msg| -> AnyView {
        view! { <button type="button" class="btn ghost small" title="Done: it leaves this list (the email stays in your inbox)" on:click=move |e| { e.stop_propagation(); dismiss(vec![m.clone()]) }><Icon name="check" size=13/>"Done"</button> }.into_any()
    };
    let picked = move || to_reply.with(|t| c.sel.with(|s| t.iter().flatten().filter(|m| s.contains(&m.key())).cloned().collect::<Vec<_>>()));
    view! {
        {move || {
            if !err.get().is_empty() && to_reply.with(Option::is_none) {
                return err_view(err.get());
            }
            let (Some(tr), Some(aw)) = (to_reply.get(), awaiting.get()) else { return loading() };
            let n = tr.len();
            let tr2 = tr.clone();
            view! {
                <div class="ml-cols">
                    <section>
                        <h3>"To reply "<span class="subdue">{n}</span></h3>
                        <p class="subdue small">"People who wrote to you directly and haven't had an answer."</p>
                        {(n > 0).then(|| view! {
                            <div class="ml-selbar">
                                <label class="row"><input type="checkbox" prop:checked=move || picked().len() == n on:change=move |e| c.sel.set(if event_target_checked(&e) { tr2.iter().map(Msg::key).collect() } else { BTreeSet::new() })/>" All"</label>
                                <span class="subdue grow">{move || { let p = picked().len(); if p > 0 { format!("{p} selected") } else { "Select to dismiss".into() } }}</span>
                                <button type="button" class="btn ghost" disabled=move || picked().is_empty() title="Mark the selected as done: they leave To reply (the emails stay in your inbox)" on:click=move |_| dismiss(picked())><Icon name="check" size=14/>"Dismiss"</button>
                            </div>
                        })}
                        <ul class="ml-rows">
                            {if tr.is_empty() { view! { <li class="empty">"Inbox zero. Nothing waiting on you."</li> }.into_any() }
                             else { tr.iter().map(|m| view! { <Row t=single(m) set=to_reply extra=done(m.clone())/> }).collect_view().into_any() }}
                        </ul>
                    </section>
                    <section>
                        <h3>"Awaiting reply "<span class="subdue">{aw.len()}</span></h3>
                        <p class="subdue small">"Mail you sent that hasn't been answered."</p>
                        <ul class="ml-rows">
                            {if aw.is_empty() { view! { <li class="empty">"Nobody owes you a reply."</li> }.into_any() }
                             else { aw.iter().map(|m| { let extra = view! { <span class="mr-chip">{format!("{}d", m.days.unwrap_or(0))}</span>{done(m.clone())} }.into_any(); view! { <Row t=single(m) set=awaiting selectable=false extra=extra/> } }).collect_view().into_any() }}
                        </ul>
                    </section>
                </div>
            }.into_any()
        }}
    }
}

// --------------------------------------------------------------- Clean ----
#[component]
pub fn Clean() -> impl IntoView {
    let c = use_mail();
    let account = RwSignal::new(String::new());
    let senders = RwSignal::new(None::<Vec<Value>>);
    let err = RwSignal::new(String::new());
    let days = RwSignal::new(30u32);
    let ocat = RwSignal::new(String::new());
    let oacct = RwSignal::new(String::new());
    let res = RwSignal::new(None::<(String, u64)>);
    let busy = RwSignal::new(String::new());
    Effect::new(move |_| {
        let a = account.get();
        c.ver.track();
        spawn_local(async move {
            let q = if a.is_empty() { String::new() } else { format!("&account={}", enc(&a)) };
            match api_get::<Value>(&format!("/mail/senders?limit=120{q}")).await {
                Ok(r) => {
                    let _ = senders.try_set(Some(r["senders"].as_array().cloned().unwrap_or_default()));
                }
                Err(e) => {
                    let _ = err.try_set(e.message);
                }
            }
        });
    });
    let old = move || json!({"days": days.get_untracked(), "category": ocat.get_untracked(), "account": oacct.get_untracked()});
    let count = move |_| {
        let mut b = old();
        b["dry_run"] = json!(true);
        spawn_local(async move {
            match api_post::<Value>("/mail/bulk/archive-old", &b).await {
                Ok(r) => {
                    let mut cats: Vec<(String, u64)> = r["categories"].as_object().into_iter().flatten().map(|(k, n)| (k.clone(), n.as_u64().unwrap_or(0))).collect();
                    cats.sort_by(|a, b| b.1.cmp(&a.1));
                    let top = cats.iter().take(4).map(|(k, n)| format!("{} {n}", cat(k))).collect::<Vec<_>>().join(", ");
                    let n = r["count"].as_u64().unwrap_or(0);
                    res.set(Some((if n > 0 { format!("{n} emails{}", if top.is_empty() { String::new() } else { format!(" ({top})") }) } else { "Nothing that old.".into() }, n)));
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let archive_old = move |_| {
        let Some((_, n)) = res.get_untracked() else { return };
        let b = old();
        spawn_local(async move {
            if !confirm(format!("Archive {n} emails?"), "Inbox mail older than the age you picked. Starred mail is never touched; every archive is in the Rules activity log and can be undone.", "Archive", false).await {
                return;
            }
            busy.set("old".into());
            match api_post::<Value>("/mail/bulk/archive-old", &b).await {
                Ok(r) => {
                    let e = r["errors"].as_array().map_or(0, |e| e.len());
                    res.set(Some((format!("Archived {}{}.", r["done"].as_u64().unwrap_or(0), if e > 0 { format!(", {e} failed") } else { String::new() }), 0)));
                }
                Err(e) => toast_bad(e.message),
            }
            let _ = busy.try_set(String::new());
        });
    };
    let sender_act = move |s: Value, what: &'static str| {
        let addr = s["from_addr"].as_str().unwrap_or("").to_string();
        let d = json!({"account": s["accts"].as_str().unwrap_or("").split(',').next().unwrap_or(""), "sender": addr});
        busy.set(format!("{what}{addr}"));
        spawn_local(async move {
            let r: Result<(), String> = async {
                match what {
                    "archive" => {
                        let r = api_post::<Value>("/mail/bulk/archive", &d).await.map_err(|e| e.message)?;
                        toast(format!("Archived {}", r["archived"].as_u64().unwrap_or(0)));
                        senders.update(|x| if let Some(x) = x { x.retain(|y| y["from_addr"] != addr.as_str()) });
                    }
                    "unsubscribe" => {
                        let r = api_post::<Value>("/mail/bulk/unsubscribe", &d).await.map_err(|e| e.message)?;
                        if r["ok"] == true {
                            toast(format!("Unsubscribed ({})", r["method"].as_str().unwrap_or("")));
                        } else if let Some(u) = r["url"].as_str() {
                            if let Some(w) = web_sys::window() {
                                let _ = w.open_with_url_and_target_and_features(u, "_blank", "noopener,noreferrer");
                            }
                            toast("Opened their unsubscribe page");
                        } else {
                            return Err(r["error"].as_str().unwrap_or("Unsubscribe failed").to_string());
                        }
                    }
                    _ => {
                        api_post::<Value>("/mail/bulk/block", &d).await.map_err(|e| e.message)?;
                        senders.update(|x| if let Some(x) = x { for y in x.iter_mut().filter(|y| y["from_addr"] == addr.as_str()) { y["blocked"] = json!(true) } });
                        toast("Future mail from them gets archived");
                    }
                }
                Ok(())
            }
            .await;
            if let Err(e) = r {
                toast_bad(e);
            }
            let _ = busy.try_set(String::new());
        });
    };
    let reset = move || res.set(None);
    let is_busy = Signal::derive(move || !busy.get().is_empty());
    view! {
        <div class="ml-clean">
            <section>
                <h3>"Archive old mail"</h3>
                <div class="ml-bar">
                    <span>"Archive inbox mail older than"</span>
                    <select class="pick-field pick-inline" aria-label="Older than" on:change=move |e| { days.set(event_target_value(&e).parse().unwrap_or(30)); reset() }>
                        {[(7, "1 week"), (30, "1 month"), (90, "3 months"), (365, "1 year")].into_iter().map(|(d, l)| view! { <option value=d.to_string() selected=move || days.get() == d>{l}</option> }).collect_view()}
                    </select>
                    <span>"in"</span>
                    <select class="pick-field pick-inline" aria-label="Category" on:change=move |e| { ocat.set(event_target_value(&e)); reset() }>
                        <option value="">"every category"</option>
                        {CAT.iter().map(|(k, l)| view! { <option value=*k>{*l}</option> }).collect_view()}
                    </select>
                    <AccountSelect value=oacct.into() on_change=Callback::new(move |v| { oacct.set(v); reset() }) all="all accounts"/>
                    <button type="button" class="btn" on:click=count>"Count"</button>
                    {move || res.get().filter(|r| r.1 > 0).map(|(_, n)| view! {
                        <button type="button" class="btn primary" disabled=move || busy.get() == "old" on:click=archive_old>{move || if busy.get() == "old" { "Archiving…".to_string() } else { format!("Archive {n}") }}</button>
                    })}
                    {move || res.get().map(|(t, _)| view! { <span class="subdue">{t}</span> })}
                </div>
                <p class="subdue small">"Starred mail is never touched. Every archive is in the Rules activity log and can be undone."</p>
            </section>
            <h3>"Senders"</h3>
            <div class="ml-bar">
                <AccountSelect value=account.into() on_change=Callback::new(move |v| account.set(v))/>
                <span class="subdue small">"Who fills your inbox. Archive everything from a sender, unsubscribe in one click, or block them for good."</span>
            </div>
            {move || {
                if !err.get().is_empty() && senders.with(Option::is_none) {
                    return err_view(err.get());
                }
                let Some(list) = senders.get() else { return loading() };
                view! {
                    <div class="ml-tablewrap">
                        <table class="ml-table">
                            <thead><tr><th>"Sender"</th><th>"Emails"</th><th>"Unread"</th><th>"Last"</th><th>"Type"</th><th/></tr></thead>
                            <tbody>
                                {list.into_iter().map(|s| {
                                    let (s1, s2, s3) = (s.clone(), s.clone(), s.clone());
                                    let addr = s["from_addr"].as_str().unwrap_or("").to_string();
                                    let name = s["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(&addr).to_string();
                                    let ct = s["cat"].as_str().unwrap_or("").to_string();
                                    view! {
                                        <tr>
                                            <td><span class="hi">{name}</span><br/><small class="subdue">{addr.clone()}</small></td>
                                            <td>{s["n"].to_string()}</td><td>{format!("{}%", s["unread_pct"].as_f64().map(|p| p.to_string()).unwrap_or_else(|| "0".into()))}</td>
                                            <td>{ago(s["last"].as_i64().unwrap_or(0))}</td>
                                            <td><span class=format!("ml-pill c-{ct}")>{cat(&ct)}</span></td>
                                            <td class="ml-rowacts">
                                                <Confirm ask="Sure? Archive all" disabled=is_busy on_confirm=Callback::new(move |_| sender_act(s1.clone(), "archive"))>"Archive all"</Confirm>
                                                {(s["unsub"] == true).then(|| view! { <Confirm ask="Sure? Unsubscribe" disabled=is_busy on_confirm=Callback::new(move |_| sender_act(s2.clone(), "unsubscribe"))>"Unsubscribe"</Confirm> })}
                                                {if s["blocked"] == true { view! { <span class="subdue small">"blocked"</span> }.into_any() }
                                                 else { view! { <Confirm ask="Sure? Block" disabled=is_busy on_confirm=Callback::new(move |_| sender_act(s3.clone(), "block"))>"Block"</Confirm> }.into_any() }}
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

// ---------------------------------------------------------------- Cold ----
const COLD_MODES: [(&str, &str); 4] = [("off", "Off"), ("list", "Just list them here"), ("label", "Label \"Cold email\""), ("archive_label", "Label and archive")];
#[component]
pub fn Cold() -> impl IntoView {
    let c = use_mail();
    let mode = RwSignal::new(String::new());
    let ms: List = RwSignal::new(None);
    let err = RwSignal::new(String::new());
    let load = move || {
        spawn_local(async move {
            match api_get::<Value>("/mail/cold").await {
                Ok(r) => {
                    let _ = mode.try_set(r["cold"]["mode"].as_str().unwrap_or("off").to_string());
                    let _ = ms.try_set(Some(Msg::list(&r["messages"])));
                }
                Err(e) => {
                    let _ = err.try_set(e.message);
                }
            }
        });
    };
    Effect::new(move |_| {
        c.ver.track();
        load();
    });
    c.use_rows(move || ms.get().unwrap_or_default().iter().map(|m| (single(m), ms)).collect());
    let save = move |m: &'static str| {
        spawn_local(async move {
            match api_post::<Value>("/mail/cold", &json!({"mode": m})).await {
                Ok(_) => {
                    toast("Saved");
                    load();
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let allow = move |m: Msg| {
        spawn_local(async move {
            match api_post::<Value>("/mail/cold", &json!({"allow": m.from_addr})).await {
                Ok(_) => {
                    ms.update(|x| if let Some(x) = x { x.retain(|y| y.from_addr != m.from_addr) });
                    toast("Sender allowed");
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    view! {
        {move || (!err.get().is_empty() && ms.with(Option::is_none)).then(|| err_view(err.get()))}
        <div>
            <div class="ml-bar"><span>"When a stranger pitches you:"</span>
                <div class="ml-seg">{COLD_MODES.iter().map(|(k, l)| { let k = *k; view! { <button type="button" aria-pressed=move || (mode.get() == k).to_string() on:click=move |_| save(k)>{*l}</button> } }).collect_view()}</div>
            </div>
            <p class="subdue small">"Cold email = an unknown sender (you've never written to them) with a sales-style pitch. Mark a sender \"Not cold\" and they're trusted from then on."</p>
            {move || match ms.get() {
                None => loading(),
                Some(list) if list.is_empty() => view! { <ul class="ml-rows"><li class="empty">"No cold email found."</li></ul> }.into_any(),
                Some(list) => view! { <ul class="ml-rows">{list.iter().map(|m| {
                    let m2 = m.clone();
                    let extra = view! { <button type="button" class="btn ghost small" on:click=move |e| { e.stop_propagation(); allow(m2.clone()) }>"Not cold"</button> }.into_any();
                    view! { <Row t=single(m) set=ms selectable=false extra=extra/> }
                }).collect_view()}</ul> }.into_any(),
            }}
        </div>
    }
}

// ----------------------------------------------------------- Analytics ----
// Received and sent per day; colours checked for colour-blind separation on the dark surface.
const IN: &str = "#6a90e0";
const OUT: &str = "#c0862a";
fn day_chart(days: &[Value]) -> impl IntoView + use<> {
    let n = |d: &Value, k: &str| d[k].as_f64().unwrap_or(0.0);
    let max = days.iter().map(|d| n(d, "received").max(n(d, "sent"))).fold(1.0f64, f64::max);
    let (w, h) = (600.0, 140.0);
    let slot = w / (days.len().max(1) as f64);
    let bw = (slot / 2.0 - 1.5).max(1.0);
    let y = move |v: f64| h - (v / max) * (h - 2.0);
    let first = days.first().and_then(|d| d["day"].as_str()).map(|d| d.get(5..).unwrap_or(d).to_string());
    let last = days.last().and_then(|d| d["day"].as_str()).map(|d| d.get(5..).unwrap_or(d).to_string());
    view! {
        <figure class="ml-chart">
            <span class="ml-axis">{max}</span>
            <svg viewBox=format!("0 0 {w} {h}") preserveAspectRatio="none" role="img" aria-label="Emails received and sent per day, last 30 days">
                {days.iter().enumerate().map(|(i, d)| {
                    let x = i as f64 * slot + 1.0;
                    let (r, s) = (n(d, "received"), n(d, "sent"));
                    view! {
                        <g>
                            <title>{format!("{}: {r} in, {s} out", d["day"].as_str().unwrap_or(""))}</title>
                            <rect x=i as f64 * slot y=0 width=slot height=h fill="transparent"/>
                            {(r > 0.0).then(|| view! { <rect x=x y=y(r) width=bw height=h - y(r) fill=IN/> })}
                            {(s > 0.0).then(|| view! { <rect x=x + bw + 1.0 y=y(s) width=bw height=h - y(s) fill=OUT/> })}
                        </g>
                    }
                }).collect_view()}
            </svg>
            {first.map(|f| view! { <div class="row ml-axis"><span class="grow">{f}</span><span>{last.unwrap_or_default()}</span></div> })}
            <figcaption class="subdue small"><i class="ml-key" style=format!("background:{IN}")/>" received "<i class="ml-key" style=format!("background:{OUT}")/>" sent"</figcaption>
        </figure>
    }
}
fn hbars(rows: Vec<(String, u64)>) -> impl IntoView + use<> {
    let max = rows.iter().map(|r| r.1).max().unwrap_or(1).max(1);
    rows.into_iter().map(|(l, n)| view! { <div class="ml-hbar" title=format!("{l}: {n}")><span class="truncate">{l.clone()}</span><i style=format!("width:{}%", 100.0 * n as f64 / max as f64)/><b>{n}</b></div> }).collect_view()
}
#[component]
pub fn Analytics() -> impl IntoView {
    let c = use_mail();
    let a = RwSignal::new(None::<Value>);
    let err = RwSignal::new(String::new());
    Effect::new(move |_| {
        c.ver.track();
        spawn_local(async move {
            match api_get::<Value>("/mail/analytics?days=30").await {
                Ok(v) => {
                    let _ = a.try_set(Some(v));
                }
                Err(e) => {
                    let _ = err.try_set(e.message);
                }
            }
        });
    });
    move || {
        if !err.get().is_empty() && a.with(Option::is_none) {
            return err_view(err.get());
        }
        let Some(a) = a.get() else { return loading() };
        let days = a["days"].as_array().cloned().unwrap_or_default();
        let sum = |k: &str| days.iter().map(|d| d[k].as_u64().unwrap_or(0)).sum::<u64>();
        let cats: Vec<(String, u64)> = a["categories"].as_array().into_iter().flatten().map(|x| (cat(x["category"].as_str().unwrap_or("")), x["n"].as_u64().unwrap_or(0))).collect();
        let doms: Vec<(String, u64)> = a["top_domains"].as_array().into_iter().flatten().map(|x| (x["dom"].as_str().unwrap_or("").to_string(), x["n"].as_u64().unwrap_or(0))).collect();
        let med = a["median_reply_hours"].as_f64().map(|h| format!("{h}h")).unwrap_or_else(|| "–".into());
        view! {
            <div>
                <div class="ml-kpis">
                    <div><b>{a["inbox_total"].to_string()}</b><span>"in inbox"</span></div>
                    <div><b>{a["inbox_unread"].to_string()}</b><span>"unread"</span></div>
                    <div><b>{sum("received")}</b><span>"received · 30d"</span></div>
                    <div><b>{sum("sent")}</b><span>"sent · 30d"</span></div>
                    <div><b>{med}</b><span>"your median reply time"</span></div>
                </div>
                <h3>"Last 30 days"</h3>
                {day_chart(&days)}
                <div class="ml-cols">
                    <section><h3>"What lands in your inbox"</h3>{hbars(cats)}</section>
                    <section><h3>"Top sending domains"</h3>{hbars(doms)}</section>
                </div>
            </div>
        }.into_any()
    }
}
