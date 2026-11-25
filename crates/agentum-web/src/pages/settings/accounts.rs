//! Accounts: each mail and calendar account, whether it syncs, and Google's reconnect (its OAuth
//! client is a desktop app: after consent Google lands on a loopback page that doesn't load, and
//! you paste that address here).
use agentum_ui::prelude::*;

#[component]
pub fn Accounts() -> impl IntoView {
    let d = RwSignal::new(None::<Value>);
    let flow = RwSignal::new(None::<(String, String)>);
    let pasted = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let load = move || {
        spawn_local(async move {
            match api_get::<Value>("/accounts").await {
                Ok(x) => d.set(Some(x)),
                Err(e) => toast_bad(e.message),
            }
        })
    };
    load();
    subscribe(&["app"]);
    on_ws(move |m| {
        if let agentum_proto::api::WsOut::App { payload } = m {
            if matches!(payload["kind"].as_str(), Some("accounts" | "mail" | "calendar")) {
                load();
            }
        }
    });
    let begin = move |email: String| {
        spawn_local(async move {
            match api_post::<Value>("/accounts/google/start", &json!({"email": email})).await {
                Ok(r) => {
                    let url = r["url"].as_str().unwrap_or("").to_string();
                    if let Some(w) = web_sys::window() {
                        let _ = w.open_with_url_and_target_and_features(&url, "_blank", "noopener");
                    }
                    pasted.set(String::new());
                    flow.set(Some((email, url)));
                }
                Err(e) => toast_bad(e.message),
            }
        })
    };
    let finish = move |_| {
        busy.set(true);
        let url = pasted.get_untracked();
        spawn_local(async move {
            match api_post::<Value>("/accounts/google/finish", &json!({"url": url})).await {
                Ok(r) => {
                    let what: Vec<&str> = r["updated"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
                    toast(format!("{}: {} reconnected. Syncing…", r["email"].as_str().unwrap_or(""), what.join(" and ")));
                    flow.set(None);
                    gloo_timers::callback::Timeout::new(4000, load).forget();
                }
                Err(e) => toast_bad(e.message),
            }
            busy.set(false);
        });
    };
    let status = |x: &Value, what: String| {
        if x.is_null() {
            return ().into_any();
        }
        let err = x["error"].as_str().filter(|e| !e.is_empty()).map(str::to_string);
        let unchecked = x["checked"] == false;
        let dot = if err.is_some() { "var(--negative)" } else if unchecked { "var(--subdue)" } else { "var(--positive)" };
        view! {
            <div class="row small">
                <span style=format!("width:8px;height:8px;border-radius:50%;display:inline-block;flex:none;background:{dot}")/>
                <span class="muted">{what}</span>
                {match err {
                    Some(e) => {
                        let l = e.to_lowercase();
                        let short = if l.contains("invalid_grant") || l.contains("expired") || l.contains("revoked") { "sign-in expired".to_string() } else { e.clone() };
                        view! { <span class="err truncate" title=e>{short}</span> }.into_any()
                    }
                    None => view! { <span class="subdue">{if unchecked { "not checked yet" } else { "syncing" }}</span> }.into_any(),
                }}
            </div>
        }
        .into_any()
    };
    move || {
        let Some(d) = d.get() else { return view! { <div class="subdue">"Loading…"</div> }.into_any() };
        let ready = d["google_ready"] == true;
        let accounts = d["accounts"].as_array().cloned().unwrap_or_default();
        view! {
            <div class="col" style="gap:0">
                {accounts.is_empty().then(|| view! { <div class="muted small">"No accounts yet."</div> })}
                {accounts.into_iter().map(|a| {
                    let email = a["email"].as_str().unwrap_or("").to_string();
                    let mail_what = a["mail"]["auth"].as_str().map(|au| format!("Mail ({})", if au == "password" { "app password" } else { au }));
                    let google = a["google"] == true;
                    let (e1, e2, e3) = (email.clone(), email.clone(), email.clone());
                    view! {
                        <div class="set-row">
                            <div class="set-label"><div class="hi">{a["name"].as_str().unwrap_or("").to_string()}</div><div class="small subdue truncate">{email.clone()}</div></div>
                            <div class="set-ctl col" style="gap:4px">
                                {status(&a["mail"], mail_what.unwrap_or_default())}
                                {status(&a["calendar"], "Calendars".into())}
                                <Show when={ let e = e1.clone(); move || google && flow.get().is_none_or(|f| f.0 != e) }>
                                    <button class="btn" style="align-self:flex-start;margin-top:4px" disabled=!ready on:click={ let e = e2.clone(); move |_| begin(e.clone()) }>"Reconnect Google"</button>
                                </Show>
                                {move || flow.get().filter(|f| f.0 == e3).map(|(email, url)| view! {
                                    <div class="card col" style="padding:12px;margin-top:6px">
                                        <div class="small">"1. Sign in as "<span class="hi">{email}</span>" in the tab that opened ("<a href=url target="_blank" rel="noreferrer">"open it again"</a>") and allow access."</div>
                                        <div class="small">"2. Google then sends you to a page that "<b>"won't load"</b>" (http://127.0.0.1:8765/…). That's expected: copy its whole address from the address bar and paste it here."</div>
                                        <div class="row">
                                            <input class="grow" autofocus placeholder="http://127.0.0.1:8765/?state=…&code=…" prop:value=pasted on:input=move |e| pasted.set(event_target_value(&e))/>
                                            <button class="btn primary" disabled=move || busy.get() || !pasted.get().contains("code=") on:click=finish>"Finish"</button>
                                            <button class="btn ghost" on:click=move |_| flow.set(None)>"Cancel"</button>
                                        </div>
                                    </div>
                                })}
                            </div>
                        </div>
                    }
                }).collect_view()}
                {(!ready).then(|| view! { <div class="small err" style="margin-top:10px">"Google sign-in needs GOOGLE_OAUTH_CLIENT_ID and GOOGLE_OAUTH_CLIENT_SECRET in the hub's environment."</div> })}
                <div class="small subdue" style="margin-top:12px">"Google sign-ins that die after a week: the OAuth app in Google Cloud Console is in “Testing”. Set Google Auth Platform → Audience → Publishing status to “In production” and they stop expiring."</div>
            </div>
        }.into_any()
    }
}
