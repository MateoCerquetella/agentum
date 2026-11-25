//! Mail › composer: new mail, replies and forwards, with attachments, the Writing tools (your model
//! on what you wrote, streamed into a preview you apply or discard), an AI draft for replies, and
//! the Review sheet that shows exactly what goes out before anything is sent.
use super::lib::*;
use super::use_mail;
use agentum_ui::prelude::*;
use regex_lite::Regex;
use send_wrapper::SendWrapper;
use std::sync::OnceLock;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

#[derive(Clone, PartialEq)]
struct Upload {
    token: String,
    filename: String,
    size: u64,
}
#[derive(Clone, PartialEq)]
struct Wt {
    label: String,
    out: String,
    /// writing | ready | failed | changed
    state: &'static str,
    error: String,
    before: String,
    subj_before: String,
    head: String,
    tail: String,
    picked: bool,
    text: String,
    mode: String,
}

fn subj_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)^\s*(?:subject|asunto)\s*:\s*(.*)\n+").unwrap())
}

fn del_upload(t: String) {
    spawn_local(async move {
        let _ = api_delete::<Value>(&format!("/mail/upload/{}", enc(&t))).await;
    });
}

async fn upload(file: web_sys::File) -> Result<Upload, String> {
    let fd = web_sys::FormData::new().map_err(|_| "no FormData".to_string())?;
    fd.append_with_blob_and_filename("file", &file, &file.name()).map_err(|_| "Upload failed".to_string())?;
    let r = gloo_net::http::Request::post("/api/mail/upload").credentials(web_sys::RequestCredentials::SameOrigin).body(fd).map_err(|e| e.to_string())?.send().await.map_err(|e| e.to_string())?;
    let j: Value = r.json().await.unwrap_or(Value::Null);
    if !r.ok() || j["token"].as_str().is_none() {
        return Err(j["error"].as_str().unwrap_or("Upload failed").to_string());
    }
    Ok(Upload { token: j["token"].as_str().unwrap_or("").into(), filename: j["filename"].as_str().unwrap_or(&file.name()).into(), size: j["size"].as_u64().unwrap_or(file.size() as u64) })
}

/// Read an NDJSON /mail/write stream: chunks until done; anything else is an error.
async fn stream_write(body: Value, signal: web_sys::AbortSignal, on_text: impl Fn(String)) -> Result<String, String> {
    let r = gloo_net::http::Request::post("/api/mail/write")
        .credentials(web_sys::RequestCredentials::SameOrigin)
        .abort_signal(Some(&signal))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| if signal.aborted() { "Stopped before completion".to_string() } else { e.to_string() })?;
    if !r.ok() {
        let st = r.status();
        let m = r.json::<Value>().await.ok().and_then(|j| j["error"].as_str().map(str::to_string)).unwrap_or_else(|| format!("HTTP {st}"));
        return Err(m);
    }
    let stream = r.body().ok_or("Invalid writing response")?;
    let reader: web_sys::ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
    let (mut pending, mut out, mut completed) = (Vec::<u8>::new(), String::new(), false);
    let frame = |line: &[u8], out: &mut String, completed: &mut bool| -> Result<(), String> {
        let line = String::from_utf8_lossy(line);
        if line.trim().is_empty() {
            return Ok(());
        }
        let ev: Value = serde_json::from_str(&line).map_err(|_| "Invalid writing response".to_string())?;
        if *completed {
            return Err("Unexpected data after writing completed".into());
        }
        match ev["type"].as_str() {
            Some("error") => Err(ev["error"].as_str().unwrap_or("Writing failed").to_string()),
            Some("done") => {
                *completed = true;
                Ok(())
            }
            Some("chunk") if ev["text"].is_string() => {
                out.push_str(ev["text"].as_str().unwrap_or(""));
                on_text(out.clone());
                Ok(())
            }
            _ => Err("Invalid writing response".into()),
        }
    };
    let res: Result<(), String> = async {
        loop {
            let chunk = JsFuture::from(reader.read()).await.map_err(|_| if signal.aborted() { "Stopped before completion".to_string() } else { "Connection ended before writing completed".to_string() })?;
            let done = js_sys::Reflect::get(&chunk, &"done".into()).ok().and_then(|d| d.as_bool()).unwrap_or(true);
            if let Ok(v) = js_sys::Reflect::get(&chunk, &"value".into()) {
                if !v.is_undefined() {
                    pending.extend(js_sys::Uint8Array::new(&v).to_vec());
                }
            }
            while let Some(nl) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=nl).collect();
                frame(&line[..line.len() - 1], &mut out, &mut completed)?;
            }
            if done {
                if !pending.is_empty() {
                    let rest = std::mem::take(&mut pending);
                    frame(&rest, &mut out, &mut completed)?;
                }
                return Ok(());
            }
        }
    }
    .await;
    let _ = reader.cancel();
    res?;
    if !completed {
        return Err("Connection ended before writing completed".into());
    }
    if out.trim().is_empty() {
        return Err("Your model returned no text".into());
    }
    Ok(out)
}

#[component]
pub fn Composer(d: Draft, on_close: Callback<()>) -> impl IntoView {
    let c = use_mail();
    let reply = d.reply_to.clone();
    let is_fwd = d.forward_of.is_some();
    let default_acct = { let a = c.accounts(); a.iter().find(|a| a.default).or(a.first()).map(|a| a.id.clone()).unwrap_or_default() };
    let from = RwSignal::new(if d.account.is_empty() { default_acct } else { d.account.clone() });
    let to = RwSignal::new(d.to.clone());
    let cc = RwSignal::new(d.cc.clone());
    let bcc = RwSignal::new(String::new());
    let show_cc = RwSignal::new(!d.cc.is_empty());
    let subject = RwSignal::new(d.subject.clone());
    let body = RwSignal::new(d.body.clone());
    let atts = RwSignal::new(Vec::<Upload>::new());
    let hint = RwSignal::new(String::new());
    let drafting = RwSignal::new(false);
    // None closed; Some(true/false) = on the selection / on what you wrote.
    let menu = RwSignal::new(None::<bool>);
    let wt = RwSignal::new(None::<Wt>);
    let l2 = RwSignal::new("Spanish".to_string());
    let review = RwSignal::new(false);
    let sending = RwSignal::new(false);
    let armed = RwSignal::new(false);
    let ta = NodeRef::<leptos::html::Textarea>::new();
    let to_ref = NodeRef::<leptos::html::Input>::new();
    let sheet = NodeRef::<leptos::html::Div>::new();
    let abort = StoredValue::new(None::<SendWrapper<web_sys::AbortController>>);
    let pending_sel = StoredValue::new(None::<(u32, u32)>);
    let orig_body = StoredValue::new(d.body.clone());
    spawn_local(async move {
        let l = lang2().await;
        let _ = l2.try_set(l);
    });
    Effect::new(move |_| {
        if review.get() {
            if let Some(s) = sheet.get() {
                let _ = s.focus();
            }
        }
    });
    // After the body changes from code: put the selection where it belongs.
    Effect::new(move |_| {
        body.track();
        if let (Some((a, b)), Some(t)) = (pending_sel.get_value(), ta.get_untracked()) {
            pending_sel.set_value(None);
            let _ = t.focus();
            let _ = t.set_selection_range(a, b);
        }
    });

    let reply2 = reply.clone();
    let ai_draft = move || {
        let Some(rp) = reply2.clone() else { return };
        drafting.set(true);
        spawn_local(async move {
            let h = hint.get_untracked();
            let mut req = json!({"account": rp.account, "folder": rp.folder, "uid": rp.uid});
            if !h.trim().is_empty() {
                req["hint"] = json!(h.trim());
            }
            match api_post::<Value>("/mail/ai-reply", &req).await {
                Ok(j) => {
                    pending_sel.set_value(Some((0, 0)));
                    body.set(format!("{}{}", j["draft"].as_str().unwrap_or(""), quote(&rp)));
                }
                Err(e) => toast_bad(if e.message.is_empty() { "Draft failed".into() } else { e.message }),
            }
            let _ = drafting.try_set(false);
        });
    };
    // Once per composer: the AI draft, the forwarded attachments, the focus.
    if d.ai_draft {
        ai_draft();
    }
    if let Some(f) = d.forward_of.clone() {
        spawn_local(async move {
            for a in f.attachments.iter().filter(|x| !x.is_inline) {
                let r: Result<Upload, String> = async {
                    let res = gloo_net::http::Request::get(&f.att_url(a.index)).credentials(web_sys::RequestCredentials::SameOrigin).send().await.map_err(|e| e.to_string())?;
                    let raw: web_sys::Response = res.into();
                    let blob: web_sys::Blob = JsFuture::from(raw.blob().map_err(|_| "blob")?).await.map_err(|_| "blob")?.unchecked_into();
                    let parts = js_sys::Array::of1(&blob);
                    let opts = web_sys::FilePropertyBag::new();
                    let ty = if a.content_type.is_empty() { blob.type_() } else { a.content_type.clone() };
                    opts.set_type(&ty);
                    let file = web_sys::File::new_with_blob_sequence_and_options(&parts, &a.filename, &opts).map_err(|_| "file")?;
                    upload(file).await
                }
                .await;
                match r {
                    Ok(u) => atts.update(|x| x.push(u)),
                    Err(_) => toast_bad(format!("Could not attach {}", a.filename)),
                }
            }
        });
    }
    let has_to = !d.to.is_empty();
    request_animation_frame(move || {
        if has_to {
            if let Some(t) = ta.get_untracked() {
                let _ = t.focus();
                let _ = t.set_selection_range(0, 0);
                t.set_scroll_top(0);
            }
        } else if let Some(i) = to_ref.get_untracked() {
            let _ = i.focus();
        }
    });

    let discard = move |force: bool| {
        let b = body.get_untracked();
        if !force && !b.trim().is_empty() && orig_body.with_value(|o| b != *o) && !armed.get_untracked() {
            armed.set(true);
            gloo_timers::callback::Timeout::new(4000, move || {
                let _ = armed.try_set(false);
            })
            .forget();
            return;
        }
        if let Some(a) = abort.get_value() {
            a.abort();
        }
        for u in atts.get_untracked() {
            del_upload(u.token);
        }
        on_close.run(());
    };

    // ---- Writing tools: on the selection, or on your own text (never the quote or the forwarded mail).
    let reply3 = reply.clone();
    let own_end = move |v: &str| -> usize {
        if let Some(rp) = &reply3 {
            let q = quote(rp);
            if v.ends_with(&q) {
                return v.len() - q.len();
            }
        }
        v.find(FWD_MARK).unwrap_or(v.len())
    };
    let reply4 = reply.clone();
    let run_wt = move |mode: &'static str, label: String| {
        menu.set(None);
        let Some(el) = ta.get_untracked() else { return };
        let v = body.get_untracked();
        // The selection is in UTF-16 units: map it to byte offsets.
        let u16_to_byte = |u: u32| -> usize {
            let mut n = 0u32;
            for (i, ch) in v.char_indices() {
                if n >= u {
                    return i;
                }
                n += ch.len_utf16() as u32;
            }
            v.len()
        };
        let (sa, sb) = (el.selection_start().ok().flatten().unwrap_or(0), el.selection_end().ok().flatten().unwrap_or(0));
        let picked = sb > sa;
        let (a, b) = if picked { (u16_to_byte(sa), u16_to_byte(sb)) } else { (0, own_end(&v)) };
        let text = v[a..b].to_string();
        if text.trim().is_empty() {
            toast_bad(if mode == "write" { "Type a few notes first, then Write it for me" } else { "Write something first, or select the part to change" });
            return;
        }
        wt.set(Some(Wt { mode: mode.into(), label, out: String::new(), state: "writing", error: String::new(), before: v.clone(), subj_before: subject.get_untracked(), head: v[..a].to_string(), tail: v[b..].to_string(), picked, text: text.clone() }));
        let Ok(ac) = web_sys::AbortController::new() else { return };
        let sig = ac.signal();
        abort.set_value(Some(SendWrapper::new(ac)));
        let ctx: String = reply4.as_ref().map(|r| r.body.chars().take(2500).collect()).unwrap_or_default();
        let req = json!({"mode": mode, "text": text, "subject": subject.get_untracked(), "to": to.get_untracked(), "context": ctx});
        spawn_local(async move {
            let r = stream_write(req, sig, move |o| {
                let _ = wt.try_update(|x| {
                    if let Some(x) = x {
                        x.out = o
                    }
                });
            })
            .await;
            match r {
                Ok(out) => {
                    let _ = wt.try_update(|x| {
                        if let Some(x) = x {
                            x.out = out;
                            x.state = "ready";
                        }
                    });
                }
                Err(e) => {
                    let _ = wt.try_update(|x| {
                        if let Some(x) = x {
                            x.state = "failed";
                            x.error = e.clone();
                        }
                    });
                    toast_bad(e);
                }
            }
            let _ = abort.try_set_value(None);
        });
    };
    let apply_wt = move || {
        let Some(w) = wt.get_untracked() else { return };
        if body.get_untracked() != w.before || subject.get_untracked() != w.subj_before {
            wt.update(|x| {
                if let Some(x) = x {
                    x.state = "changed"
                }
            });
            return;
        }
        // "Write it for me" may start with "Subject: …": that goes to the subject field, not the body.
        if w.mode == "write" {
            if let Some(cap) = subj_re().captures(&w.out) {
                if w.subj_before.trim().is_empty() {
                    subject.set(cap[1].trim().to_string());
                }
            }
        }
        let shown = if w.mode != "write" {
            w.out.clone()
        } else {
            let t = w.out.trim_start().to_lowercase();
            if (t.starts_with("subject") || t.starts_with("asunto")) && !w.out.contains('\n') { String::new() } else { subj_re().replace(&w.out, "").to_string() }
        };
        let trail = if w.picked { w.text[w.text.trim_end().len()..].to_string() } else { String::new() };
        let result = format!("{}{}", shown.trim_end(), trail);
        let a = w.head.encode_utf16().count() as u32;
        pending_sel.set_value(Some((a, a + result.encode_utf16().count() as u32)));
        body.set(format!("{}{}{}", w.head, result, w.tail));
        wt.set(None);
        let (before, sb) = (w.before.clone(), w.subj_before.clone());
        c.show_undo(format!("{} applied", w.label), move || {
            pending_sel.set_value(Some((0, 0)));
            body.set(before.clone());
            subject.set(sb.clone());
        });
    };

    // ---- Review, then send.
    let open_review = move || {
        if abort.with_value(Option::is_some) {
            toast_bad("Your model is still writing: wait, or press Stop");
            return;
        }
        if to.get_untracked().trim().is_empty() {
            toast_bad("Add a recipient");
            return;
        }
        menu.set(None);
        review.set(true);
    };
    let reply5 = reply.clone();
    let send = move || {
        if sending.get_untracked() {
            return;
        }
        if to.get_untracked().trim().is_empty() {
            toast_bad("Add a recipient");
            return;
        }
        sending.set(true);
        let mut req = json!({
            "account_id": from.get_untracked(), "to": to.get_untracked(), "cc": cc.get_untracked(), "bcc": bcc.get_untracked(),
            "subject": subject.get_untracked(), "body": body.get_untracked(), "attachments": atts.get_untracked().into_iter().map(|a| a.token).collect::<Vec<_>>(),
        });
        if let Some(rp) = &reply5 {
            if !rp.message_id.is_empty() {
                req["in_reply_to"] = json!(rp.message_id);
            }
            let refs = [rp.references.as_str(), rp.message_id.as_str()].iter().filter(|x| !x.is_empty()).copied().collect::<Vec<_>>().join(" ");
            if !refs.is_empty() {
                req["references"] = json!(refs);
            }
            req["source"] = json!({"account": rp.account, "folder": rp.folder, "uid": rp.uid});
        }
        spawn_local(async move {
            match api_post::<Value>("/mail/send", &req).await {
                Ok(_) => {
                    // A reply only leaves "To reply" once the sync sees it in Sent: sync now.
                    spawn_local(async {
                        let _ = api_post::<Value>("/mail/run", &json!({})).await;
                    });
                    toast("Sent");
                    on_close.run(());
                }
                Err(e) => {
                    toast_bad(if e.message.is_empty() { "Send failed".into() } else { e.message });
                    let _ = sending.try_set(false);
                }
            }
        });
    };

    let prio = Signal::derive(move || if c.front.get() == "compose" { 2 } else { 1 });
    c.pane(
        "compose",
        prio,
        Callback::new(move |_| {
            if menu.get_untracked().is_some() {
                menu.set(None)
            } else if review.get_untracked() {
                review.set(false)
            } else {
                discard(false)
            }
        }),
        None,
    );

    let send2 = send.clone();
    let on_key = move |e: leptos::ev::KeyboardEvent| {
        if e.key() == "Enter" && (e.meta_key() || e.ctrl_key()) {
            e.prevent_default();
            if review.get_untracked() { send2() } else { open_review() }
        }
    };
    let wt_items = move || {
        let l = l2.get();
        let mut v: Vec<(&'static str, String, Option<&'static str>)> = vec![
            ("write", "Write it for me".into(), Some("your notes → an email")),
            ("proofread", "Proofread".into(), None),
            ("rewrite", "Rewrite".into(), None),
            ("friendly", "Friendlier".into(), None),
            ("professional", "More professional".into(), None),
            ("concise", "Shorter".into(), None),
        ];
        if l != "English" {
            v.push(("english", "Translate to English".into(), None));
        }
        v.push(("spanish", format!("Translate to {l}"), None));
        v
    };
    let prow = |k: &'static str, v: String| (!v.is_empty()).then(|| view! { <div class="ml-prow"><b>{k}</b><span>{v}</span></div> });
    let title = if reply.is_some() { "Reply" } else if is_fwd { "Forward" } else { "New email" };
    let has_reply = reply.is_some();
    let ai_draft2 = ai_draft.clone();
    let send3 = send.clone();
    view! {
        <div class="ml-compose" on:keydown=on_key on:pointerdown=move |e| {
            if menu.get_untracked().is_some() && !e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).is_some_and(|t| t.closest(".ml-wt").ok().flatten().is_some()) { menu.set(None) }
        }>
            <div class="ml-pbar">
                <button type="button" class=move || if armed.get() { "btn ghost armed" } else { "btn ghost" } on:click=move |_| discard(false)><Icon name="chevronL" size=14/>{move || if armed.get() { "Discard draft?" } else { "Discard" }}</button>
                <span class="hi grow">{move || if review.get() { "Review before sending" } else { title }}</span>
            </div>
            <Show when=move || review.get()>
                <div class="ml-review" node_ref=sheet tabindex="-1">
                    <div class="ml-pmeta">
                        {move || {
                            let fa = c.accounts().into_iter().find(|a| a.id == from.get());
                            view! {
                                {prow("From", fa.map(|a| format!("{} · {}", a.name, a.from)).unwrap_or_default())}
                                {prow("To", to.get())}{prow("Cc", cc.get())}{prow("Bcc", bcc.get())}
                                {prow("Subject", { let s = subject.get(); if s.is_empty() { "(no subject)".into() } else { s } })}
                                {prow("Attached", atts.get().iter().map(|a| a.filename.clone()).collect::<Vec<_>>().join(", "))}
                            }
                        }}
                    </div>
                    <div class="ml-pbody">{move || { let b = body.get(); if b.is_empty() { view! { <i class="subdue">"(empty message)"</i> }.into_any() } else { b.into_any() } }}</div>
                    <div class="ml-formbar">
                        <span class="grow"/><span class="subdue small">"⌘/Ctrl+Enter sends"</span>
                        <button type="button" class="btn" on:click=move |_| review.set(false)>"Edit"</button>
                        <button type="button" class="btn primary" disabled=move || sending.get() on:click={ let s = send3.clone(); move |_| s() }><Icon name="send" size=14/>{move || if sending.get() { "Sending…" } else { "Send" }}</button>
                    </div>
                </div>
            </Show>
            <form class="ml-form" style:display=move || if review.get() { "none" } else { "" } on:submit=move |e| { e.prevent_default(); open_review() }>
                <label>"From "
                    <select class="pick-field" aria-label="From" on:change=move |e| from.set(event_target_value(&e))>
                        {move || c.accounts().into_iter().map(|a| { let id = a.id.clone(); let sel = from.get() == id; view! { <option value=id selected=sel>{format!("{} · {}", a.name, a.from)}</option> } }).collect_view()}
                    </select>
                </label>
                <label>"To "<input name="to" node_ref=to_ref prop:value=to on:input=move |e| to.set(event_target_value(&e)) placeholder="name@example.com, …" required/></label>
                <Show when=move || show_cc.get()>
                    <label>"Cc "<input name="cc" prop:value=cc on:input=move |e| cc.set(event_target_value(&e))/></label>
                    <label>"Bcc "<input name="bcc" prop:value=bcc on:input=move |e| bcc.set(event_target_value(&e))/></label>
                </Show>
                <label>"Subject "<input name="subject" prop:value=subject on:input=move |e| subject.set(event_target_value(&e))/></label>
                <textarea node_ref=ta name="body" rows="14" placeholder="Write your email" prop:value=body on:input=move |e| body.set(event_target_value(&e))/>
                <Show when=move || !atts.with(Vec::is_empty)>
                    <div class="ml-atts">
                        <For each=move || atts.get() key=|a| a.token.clone() let:a>
                            {
                                let tk = a.token.clone();
                                view! {
                                    <span class="ml-att"><Icon name="paperclip" size=13/>{a.filename.clone()}" "<small>{kb(a.size)}</small>
                                        <button type="button" class="icon-btn" title="Remove" aria-label="Remove" on:click=move |_| { let t = tk.clone(); atts.update(|x| x.retain(|y| y.token != t)); del_upload(t) }><Icon name="x" size=12/></button>
                                    </span>
                                }
                            }
                        </For>
                    </div>
                </Show>
                {move || wt.get().map(|w| view! {
                    <section class="ml-wtres" aria-label="Writing suggestion">
                        <p role="status" class="small">{match w.state {
                            "writing" => format!("{} · Writing suggestion… Your draft is kept.", w.label),
                            "ready" => format!("{} · Ready to review. Apply only if you want to replace your text.", w.label),
                            "changed" => "Your draft changed while this suggestion was written. Copy the text you want to keep, or generate a new suggestion.".to_string(),
                            _ => format!("Failed · {}{} Your draft was kept.", w.error, if w.out.is_empty() { "" } else { " Partial suggestion retained for reference." }),
                        }}</p>
                        <textarea aria-label="Suggested text" readonly prop:value=w.out.clone() rows="8"/>
                        <div class="row">
                            <button type="button" class="btn primary" disabled=w.state != "ready" on:click=move |_| apply_wt()>"Apply suggestion"</button>
                            <button type="button" class="btn" on:click=move |_| { if let Some(a) = abort.get_value() { a.abort() } wt.set(None) }>"Discard"</button>
                        </div>
                    </section>
                })}
                <div class="ml-formbar">
                    <Show when=move || !show_cc.get()><button type="button" class="btn ghost" on:click=move |_| show_cc.set(true)>"Cc/Bcc"</button></Show>
                    <label class="btn ghost"><Icon name="paperclip" size=14/>"Attach"
                        <input type="file" multiple hidden on:change=move |e| {
                            let input = event_target::<web_sys::HtmlInputElement>(&e);
                            let files: Vec<web_sys::File> = input.files().map(|l| (0..l.length()).filter_map(|i| l.get(i)).collect()).unwrap_or_default();
                            input.set_value("");
                            spawn_local(async move {
                                for f in files {
                                    let name = f.name();
                                    match upload(f).await {
                                        Ok(u) => atts.update(|x| x.push(u)),
                                        Err(err) => toast_bad(format!("{name}: {err}")),
                                    }
                                }
                            });
                        }/>
                    </label>
                    <span class="ml-wt">
                        <button type="button" class="btn ghost" title="Your model on what you wrote, or on the part you selected"
                            on:click=move |_| {
                                if let Some(a) = abort.get_value() { a.abort(); return }
                                if menu.get_untracked().is_some() { menu.set(None); return }
                                let sel = ta.get_untracked().is_some_and(|t| t.selection_end().ok().flatten() > t.selection_start().ok().flatten());
                                menu.set(Some(sel));
                            }>
                            {move || { let w = wt.with(|w| w.as_ref().is_some_and(|w| w.state == "writing")); view! { <Icon name=if w { "stop" } else { "sparkle" } size=14/>{if w { "Stop" } else { "Writing tools" }} } }}
                        </button>
                        {move || menu.get().map(|on_sel| { let rw = run_wt.clone(); view! {
                            <div class="ml-wtmenu" role="menu">
                                <div class="subdue small">{if on_sel { "On the selected text" } else { "On what you wrote" }}</div>
                                {wt_items().into_iter().enumerate().map(|(i, (k, t, sub))| { let rw = rw.clone(); let t2 = t.clone(); view! {
                                    <button type="button" role="menuitem" autofocus=i == 0 on:click=move |_| rw(k, t2.clone())>{t}{sub.map(|s| view! { <small>{s}</small> })}</button>
                                }}).collect_view()}
                            </div>
                        }})}
                    </span>
                    {has_reply.then(|| { let ad = ai_draft2.clone(); view! {
                        <input class="ml-hint" placeholder="AI: what to say (optional)" prop:value=hint on:input=move |e| hint.set(event_target_value(&e))/>
                        <button type="button" class="btn ghost" disabled=move || drafting.get() on:click=move |_| ad()><Icon name="sparkle" size=14/>{move || if drafting.get() { "Drafting…" } else { "AI draft" }}</button>
                    }})}
                    <span class="grow"/>
                    <span class="subdue small ml-hide-narrow">"⌘/Ctrl+Enter to review"</span>
                    <button type="submit" class="btn primary"><Icon name="eye" size=14/>"Review"</button>
                </div>
            </form>
        </div>
    }
}
