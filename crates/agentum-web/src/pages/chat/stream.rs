//! POST a JSON body and read the answer as server-sent events (EventSource can't POST): each
//! `data:` line is one JSON event for `on`.
use serde_json::Value;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

fn err(e: JsValue) -> String {
    js_sys::Reflect::get(&e, &"message".into()).ok().and_then(|m| m.as_string()).unwrap_or_else(|| format!("{e:?}"))
}

/// Ok when the stream ends; Err("") when `signal` aborted it.
pub async fn post_stream(path: &str, body: &Value, signal: &web_sys::AbortSignal, mut on: impl FnMut(Value)) -> Result<(), String> {
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_body(&JsValue::from_str(&body.to_string()));
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    init.set_signal(Some(signal));
    let headers = web_sys::Headers::new().map_err(err)?;
    headers.set("Content-Type", "application/json").map_err(err)?;
    init.set_headers(&headers);
    let w = web_sys::window().ok_or("no window")?;
    let aborted = || signal.aborted();
    let res: web_sys::Response = match JsFuture::from(w.fetch_with_str_and_init(&format!("/api{path}"), &init)).await {
        Ok(r) => r.unchecked_into(),
        Err(_) if aborted() => return Err(String::new()),
        Err(e) => return Err(err(e)),
    };
    if !res.ok() {
        let text = match res.text() {
            Ok(p) => JsFuture::from(p).await.ok().and_then(|t| t.as_string()).unwrap_or_default(),
            Err(_) => String::new(),
        };
        let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_else(|| res.status_text());
        return Err(if res.status() == 401 { "You're signed out. Reload to sign in.".into() } else { msg });
    }
    let Some(stream) = res.body() else { return Err("no answer".into()) };
    let reader: web_sys::ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
    let mut buf: Vec<u8> = vec![];
    loop {
        let chunk = match JsFuture::from(reader.read()).await {
            Ok(c) => c,
            Err(_) if aborted() => return Err(String::new()),
            Err(e) => return Err(err(e)),
        };
        if js_sys::Reflect::get(&chunk, &"done".into()).ok().and_then(|d| d.as_bool()).unwrap_or(true) {
            break;
        }
        if let Ok(v) = js_sys::Reflect::get(&chunk, &"value".into()) {
            buf.extend(js_sys::Uint8Array::new(&v).to_vec());
        }
        while let Some(i) = buf.windows(2).position(|w| w == b"\n\n") {
            let block: Vec<u8> = buf.drain(..i + 2).collect();
            let text = String::from_utf8_lossy(&block);
            let data: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("data:")).map(|l| l.trim_start()).collect();
            if !data.is_empty() {
                if let Ok(v) = serde_json::from_str::<Value>(&data.join("\n")) {
                    on(v);
                }
            }
        }
    }
    Ok(())
}
