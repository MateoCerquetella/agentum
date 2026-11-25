//! Your model (Ollama). One request at a time across the whole app: the chat
//! and other interactive calls (foreground) go before background work (rules,
//! brief, plans). Calls use the lowest thinking level unless chat opts in.
use crate::state::App;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use std::{sync::{atomic::{AtomicI64, AtomicUsize, Ordering}, Arc, Mutex}, time::Duration};
use tokio::sync::{Mutex as AMutex, OwnedMutexGuard};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Prio { Fg, Bg }

#[derive(Default)]
pub struct Gate {
    slot: Arc<AMutex<()>>,
    fg_waiting: AtomicUsize,
    bg_waiting: AtomicUsize,
    pub last_ok: AtomicI64,
    pub last_err: AtomicI64,
    pub last_error: Mutex<String>,
    cache: Mutex<Vec<(String, i64, String)>>,
}
pub struct Slot { _g: OwnedMutexGuard<()> }
struct Waiter<'a>(&'a AtomicUsize);
impl Drop for Waiter<'_> {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::SeqCst); }
}

impl Gate {
    /// Wait for the model. Background callers also wait while any
    /// foreground caller is queued. None on timeout (model busy).
    pub async fn acquire(&self, prio: Prio, timeout: Duration) -> Option<Slot> {
        let deadline = tokio::time::Instant::now() + timeout;
        let ctr = if prio == Prio::Fg { &self.fg_waiting } else { &self.bg_waiting };
        ctr.fetch_add(1, Ordering::SeqCst);
        let _waiting = Waiter(ctr);
        loop {
            if prio == Prio::Fg || self.fg_waiting.load(Ordering::SeqCst) == 0 {
                if let Ok(g) = self.slot.clone().try_lock_owned() { break Some(Slot { _g: g }); }
            }
            if tokio::time::Instant::now() >= deadline { break None; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    pub fn busy(&self) -> bool { self.slot.try_lock().is_err() }
    fn ok(&self) { self.last_ok.store(crate::util::now_ts(), Ordering::SeqCst); }
    fn err(&self, e: &str) { self.last_err.store(crate::util::now_ts(), Ordering::SeqCst); *self.last_error.lock().unwrap() = e.chars().take(200).collect(); }
}

/// Ollama's base URL from the configured OpenAI-style URL (".../v1").
pub fn base(app: &App) -> String {
    let u = app.setting("CERQ_MAIL_LLM_URL");
    u.trim_end_matches('/').trim_end_matches("/v1").to_string()
}
pub fn model(app: &App) -> String { app.setting("CERQ_MAIL_LLM_MODEL") }
pub fn thinking(app: &App, enabled: bool) -> Value {
    let model = model(app);
    let name = model.rsplit('/').next().unwrap_or(&model).split(':').next();
    if name == Some("gpt-oss") { json!(if enabled { "medium" } else { "low" }) } else { json!(enabled) }
}
pub fn configured(app: &App) -> bool { !app.setting("CERQ_MAIL_LLM_URL").is_empty() && !model(app).is_empty() }

pub fn strip_think(s: &str) -> String {
    let re = regex::Regex::new(r"(?s)<think>.*?</think>").unwrap();
    let t = re.replace_all(s, "");
    let t = match t.find("</think>") { Some(i) => t[i + 8..].to_string(), None => t.to_string() };
    t.trim().to_string()
}

/// One answer (no streaming). Cached 5 min per identical prompt.
pub async fn complete(app: &App, messages: Vec<Value>, max_tokens: u32, prio: Prio) -> Option<String> {
    complete_with(app, messages, max_tokens, prio, if prio == Prio::Fg { 15 } else { 5 }, 90).await
}
/// A long answer (the journal's standup, meeting prep, week review): waits up to `wait` s for the model and `run` s for the answer.
pub async fn complete_long(app: &App, messages: Vec<Value>, max_tokens: u32, prio: Prio) -> Option<String> {
    complete_with(app, messages, max_tokens, prio, if prio == Prio::Fg { 30 } else { 120 }, 240).await
}
async fn complete_with(app: &App, messages: Vec<Value>, max_tokens: u32, prio: Prio, wait: u64, run: u64) -> Option<String> {
    if !configured(app) { return None; }
    let key = format!("{}|{max_tokens}|{}", model(app), serde_json::to_string(&messages).unwrap_or_default());
    let now = crate::util::now_ts();
    if let Some((_, _, a)) = app.gate.cache.lock().unwrap().iter().find(|(k, t, _)| *k == key && now - t < 300) { return Some(a.clone()); }
    let _slot = app.gate.acquire(prio, Duration::from_secs(wait)).await?;
    let body = json!({"model": model(app), "messages": messages, "stream": false, "think": false, "options": {"temperature": 0, "num_predict": max_tokens}});
    let r = app.http.post(format!("{}/api/chat", base(app))).json(&body).timeout(Duration::from_secs(run)).send().await;
    let out = match r {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.ok().map(|v| strip_think(v["message"]["content"].as_str().unwrap_or(""))),
        Ok(r) => { app.gate.err(&format!("HTTP {}", r.status())); None }
        Err(e) => { app.gate.err(&e.to_string()); None }
    };
    match out {
        Some(a) if !a.is_empty() => {
            app.gate.ok();
            let mut c = app.gate.cache.lock().unwrap();
            if c.len() > 64 { c.remove(0); }
            c.push((key, now, a.clone()));
            Some(a)
        }
        Some(_) => { app.gate.ok(); None }
        None => None,
    }
}

/// Streamed chat (Ollama NDJSON). Yields each parsed line; holds the model
/// slot until the stream ends. `body` gets model/stream/think filled in.
pub async fn stream(app: &App, mut body: Value, prio: Prio) -> anyhow::Result<impl Stream<Item = anyhow::Result<Value>> + Send + 'static> {
    if !configured(app) { anyhow::bail!("No model is set up. Pick one in Settings → Your model."); }
    let slot = app.gate.acquire(prio, Duration::from_secs(if prio == Prio::Fg { 60 } else { 10 })).await
        .ok_or_else(|| anyhow::anyhow!("Your model is busy with another request. Try again in a moment."))?;
    body["model"] = json!(model(app));
    body["stream"] = json!(true);
    if body.get("think").is_none() { body["think"] = json!(false); }
    let r = app.http.post(format!("{}/api/chat", base(app))).json(&body).timeout(Duration::from_secs(600)).send().await
        .map_err(|e| { app.gate.err(&e.to_string()); anyhow::anyhow!("Couldn't reach your model: {e}") })?;
    if !r.status().is_success() {
        let st = r.status(); let t = r.text().await.unwrap_or_default();
        app.gate.err(&format!("HTTP {st}"));
        anyhow::bail!("Your model answered {st}: {}", t.chars().take(200).collect::<String>());
    }
    app.gate.ok();
    let mut bytes = r.bytes_stream();
    Ok(async_stream(move |tx| async move {
        let _slot = slot;
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let chunk = tokio::select! { _ = tx.closed() => return, chunk = bytes.next() => chunk };
            let Some(chunk) = chunk else { return; };
            let chunk = match chunk { Ok(c) => c, Err(e) => { let _ = tx.send(Err(anyhow::anyhow!(e))).await; return; } };
            buf.extend_from_slice(&chunk);
            while let Some(i) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=i).collect();
                if let Ok(v) = serde_json::from_slice::<Value>(&line) {
                    let done = v["done"].as_bool().unwrap_or(false);
                    if tx.send(Ok(v)).await.is_err() { return; }
                    if done { return; }
                }
            }
        }
    }))
}

/// A tiny channel-backed stream helper (avoids pulling in async-stream).
fn async_stream<F, Fut>(f: F) -> impl Stream<Item = anyhow::Result<Value>> + Send + 'static
where F: FnOnce(tokio::sync::mpsc::Sender<anyhow::Result<Value>>) -> Fut, Fut: std::future::Future<Output = ()> + Send + 'static {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(f(tx));
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

pub async fn list_models(app: &App, url: &str) -> anyhow::Result<Vec<String>> {
    let b = url.trim_end_matches('/').trim_end_matches("/v1");
    let v: Value = app.http.get(format!("{b}/api/tags")).timeout(Duration::from_secs(6)).send().await?.error_for_status()?.json().await?;
    Ok(v["models"].as_array().cloned().unwrap_or_default().iter().filter_map(|m| m["name"].as_str().map(String::from)).filter(|n| !n.contains("embed")).collect())
}

pub fn status(app: &App) -> Value {
    let g = &app.gate;
    let (ok, err) = (g.last_ok.load(Ordering::SeqCst), g.last_err.load(Ordering::SeqCst));
    let now = crate::util::now_ts();
    let avail = if ok == 0 && err == 0 { "unknown" } else if ok >= err { if now - ok < 600 { "online" } else { "unknown" } } else { "offline" };
    json!({"configured": configured(app), "model": model(app), "availability": avail, "busy": g.busy(),
        "waiting": {"foreground": g.fg_waiting.load(Ordering::SeqCst), "background": g.bg_waiting.load(Ordering::SeqCst)},
        "last_success": if ok > 0 { json!(ok) } else { Value::Null }, "last_error": g.last_error.lock().unwrap().clone()})
}

/// Pull the first JSON array/object out of a model answer.
pub fn extract_json(s: &str) -> Option<Value> {
    let s = strip_think(s);
    // Whichever comes first: an object holding lists stays the object.
    let mut order = [('[', ']'), ('{', '}')];
    if s.find('{').unwrap_or(usize::MAX) < s.find('[').unwrap_or(usize::MAX) { order.reverse(); }
    for (open, close) in order {
        if let (Some(a), Some(b)) = (s.find(open), s.rfind(close)) {
            if b > a { if let Ok(v) = serde_json::from_str(&s[a..=b]) { return Some(v); } }
        }
    }
    None
}

/// A JSON object or list the model stopped writing halfway (it ran out of tokens): close
/// the open string and brackets, dropping a dangling key or comma. None if that doesn't parse.
pub fn repair_json(s: &str) -> Option<Value> {
    let s = strip_think(s);
    let start = s.find(['{', '['])?;
    let body = &s[start..];
    let (mut stack, mut in_str, mut esc) = (Vec::new(), false, false);
    for ch in body.chars() {
        if in_str { if esc { esc = false } else if ch == '\\' { esc = true } else if ch == '"' { in_str = false } continue; }
        match ch { '"' => in_str = true, '{' => stack.push('}'), '[' => stack.push(']'), '}' | ']' => { stack.pop(); } _ => {} }
    }
    let mut t = body.trim_end().to_string();
    if in_str { t.push('"'); }
    for _ in 0..3 {
        let mut u = t.trim_end().trim_end_matches([',', ':']).to_string();
        // a key left without its value: "…", "key"  → drop it
        if u.ends_with('"') { if let Some(i) = u[..u.len() - 1].rfind('"') { if u[..i].trim_end().ends_with([',', '{']) && stack.last() == Some(&'}') && !u[..i].trim_end().ends_with(':') { let k = u[..i].trim_end().to_string(); if k.ends_with(',') { u = k.trim_end_matches(',').to_string(); } } } }
        let mut w = u.clone();
        for c in stack.iter().rev() { w.push(*c); }
        if let Ok(v) = serde_json::from_str::<Value>(&w) { return Some(v); }
        t = u;
        // cut back to the last complete element
        match t.rfind(',') { Some(i) => t.truncate(i), None => break }
        let (mut st2, mut ins, mut e2) = (Vec::new(), false, false);
        for ch in t.chars() { if ins { if e2 { e2 = false } else if ch == '\\' { e2 = true } else if ch == '"' { ins = false } continue; } match ch { '"' => ins = true, '{' => st2.push('}'), '[' => st2.push(']'), '}' | ']' => { st2.pop(); } _ => {} } }
        stack = st2;
    }
    None
}

#[cfg(test)]
mod json_tests {
    #[test]
    fn repairs_a_cut_answer() {
        let v = super::repair_json("{\"summary\": \"ok\", \"ui\": {\"t\": \"stack\", \"children\": [{\"t\": \"text\", \"text\": \"a\"}, {\"t\": \"callout\", \"text\": \"cut her").unwrap();
        assert_eq!(v["summary"], "ok");
        assert_eq!(v["ui"]["children"][0]["text"], "a");
    }
    #[test]
    fn extract_json_prefers_the_outer() {
        let v = super::extract_json("Sure: {\"ui\": {\"t\": \"stack\", \"children\": [{\"t\": \"text\"}]}}").unwrap();
        assert!(v.is_object());
        assert!(super::extract_json("[1, 2]").unwrap().is_array());
        assert!(super::extract_json("```json\n[{\"a\": [1]}]\n```").unwrap().is_array());
    }
}
