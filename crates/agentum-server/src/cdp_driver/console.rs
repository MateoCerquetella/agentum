//! Console + network diagnostics: a long-lived per-browser listener that
//! buffers `console.*` / runtime-exception entries and failed network requests
//! (each stamped with the snapshot generation), plus the in-flight-request and
//! document-status tracking the `wait`/`get_console` ops read. Self-contained:
//! it opens its own CDP WebSocket (no `CdpConn`), reaching back into the parent
//! only for `current_generation`. `use super::*` provides that + shared types.

use super::*;

// --- console + network diagnostics (long-lived listener) ---------------------

/// One buffered console/log entry, stamped with the snapshot generation current
/// when it arrived (so `get_console(since_generation)` can scope to a snapshot).
#[derive(Debug, Clone, PartialEq)]
struct ConsoleEntry {
    level: String,
    text: String,
    source: String,
    url: Option<String>,
    line: Option<i64>,
    generation: u64,
}

/// One buffered network failure (HTTP >=400 or a transport error).
#[derive(Debug, Clone, PartialEq)]
struct NetFailure {
    url: String,
    status: i64,
    error: String,
    generation: u64,
}

#[derive(Default)]
struct ConsoleState {
    console: VecDeque<ConsoleEntry>,
    network: VecDeque<NetFailure>,
    /// requestId → url, so `Network.loadingFailed` (which lacks a url) can report one.
    request_urls: HashMap<String, String>,
    /// In-flight request count (req sent − finished/failed) for `network_idle` waits.
    in_flight: i64,
    /// Status of the most recent main-document response, for `navigate`'s http_status.
    last_doc_status: Option<i64>,
}

const MAX_CONSOLE: usize = 1000;
const MAX_NETFAIL: usize = 500;
const MAX_REQ_MAP: usize = 4000;

fn console_state() -> &'static Mutex<ConsoleState> {
    static STATE: OnceLock<Mutex<ConsoleState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ConsoleState::default()))
}

/// Rank a level for `min_level` filtering. error > warning > info (unknown → info).
fn level_rank(level: &str) -> u8 {
    match level {
        "error" => 3,
        "warning" => 2,
        _ => 1,
    }
}

/// Normalize a `consoleAPICalled` type / `Log` level to error|warning|info.
fn normalize_level(raw: &str) -> &'static str {
    match raw {
        "error" | "assert" => "error",
        "warning" => "warning",
        _ => "info",
    }
}

fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Join `Runtime.consoleAPICalled` args into a readable line.
fn console_args_text(args: Option<&Value>) -> String {
    let Some(args) = args.and_then(Value::as_array) else {
        return String::new();
    };
    args.iter()
        .map(|a| {
            a.get("value")
                .map(value_to_text)
                .or_else(|| {
                    a.get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build a [`ConsoleEntry`] from a CDP event, or `None` if it isn't one we capture.
fn console_entry_from_event(method: &str, params: &Value, generation: u64) -> Option<ConsoleEntry> {
    match method {
        "Runtime.consoleAPICalled" => {
            let raw = params.get("type").and_then(Value::as_str).unwrap_or("log");
            let frame = params
                .get("stackTrace")
                .and_then(|s| s.get("callFrames"))
                .and_then(Value::as_array)
                .and_then(|f| f.first());
            Some(ConsoleEntry {
                level: normalize_level(raw).to_string(),
                text: console_args_text(params.get("args")),
                source: "console".into(),
                url: frame
                    .and_then(|f| f.get("url"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                line: frame
                    .and_then(|f| f.get("lineNumber"))
                    .and_then(Value::as_i64),
                generation,
            })
        }
        "Runtime.exceptionThrown" => {
            let d = params.get("exceptionDetails")?;
            let text = d
                .get("exception")
                .and_then(|e| e.get("description"))
                .and_then(Value::as_str)
                .or_else(|| d.get("text").and_then(Value::as_str))
                .unwrap_or("uncaught exception")
                .to_string();
            Some(ConsoleEntry {
                level: "error".into(),
                text,
                source: "exception".into(),
                url: d.get("url").and_then(Value::as_str).map(str::to_string),
                line: d.get("lineNumber").and_then(Value::as_i64),
                generation,
            })
        }
        "Log.entryAdded" => {
            let e = params.get("entry")?;
            Some(ConsoleEntry {
                level: normalize_level(e.get("level").and_then(Value::as_str).unwrap_or("info"))
                    .to_string(),
                text: e
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                source: e
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or("log")
                    .to_string(),
                url: e.get("url").and_then(Value::as_str).map(str::to_string),
                line: e.get("lineNumber").and_then(Value::as_i64),
                generation,
            })
        }
        _ => None,
    }
}

/// A failed HTTP response (status >= 400) → a [`NetFailure`], else `None`.
fn net_failure_from_response(params: &Value, generation: u64) -> Option<NetFailure> {
    let resp = params.get("response")?;
    let status = resp.get("status").and_then(Value::as_i64).unwrap_or(0);
    if status < 400 {
        return None;
    }
    Some(NetFailure {
        url: resp
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        status,
        error: resp
            .get("statusText")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        generation,
    })
}

fn push_console(entry: ConsoleEntry) {
    let mut s = console_state().lock().expect("console state poisoned");
    s.console.push_back(entry);
    while s.console.len() > MAX_CONSOLE {
        s.console.pop_front();
    }
}

fn push_netfailure(f: NetFailure) {
    let mut s = console_state().lock().expect("console state poisoned");
    s.network.push_back(f);
    while s.network.len() > MAX_NETFAIL {
        s.network.pop_front();
    }
}

/// Dispatch one CDP event into the diagnostics buffers. Returns whether it matched
/// (handy for unit tests).
fn ingest_event(method: &str, params: &Value) -> bool {
    let generation = current_generation();
    if let Some(entry) = console_entry_from_event(method, params, generation) {
        push_console(entry);
        return true;
    }
    match method {
        "Network.requestWillBeSent" => {
            let mut s = console_state().lock().expect("console state poisoned");
            s.in_flight += 1;
            if let (Some(id), Some(url)) = (
                params.get("requestId").and_then(Value::as_str),
                params
                    .get("request")
                    .and_then(|r| r.get("url"))
                    .and_then(Value::as_str),
            ) {
                if s.request_urls.len() > MAX_REQ_MAP {
                    s.request_urls.clear();
                }
                s.request_urls.insert(id.to_string(), url.to_string());
            }
            true
        }
        "Network.responseReceived" => {
            // Capture the main-document status for `navigate`'s http_status.
            if params.get("type").and_then(Value::as_str) == Some("Document") {
                if let Some(st) = params
                    .get("response")
                    .and_then(|r| r.get("status"))
                    .and_then(Value::as_i64)
                {
                    console_state()
                        .lock()
                        .expect("console state poisoned")
                        .last_doc_status = Some(st);
                }
            }
            if let Some(f) = net_failure_from_response(params, generation) {
                push_netfailure(f);
            }
            true
        }
        "Network.loadingFinished" => {
            decrement_in_flight();
            true
        }
        "Network.loadingFailed" => {
            decrement_in_flight();
            let url = params
                .get("requestId")
                .and_then(Value::as_str)
                .and_then(|id| {
                    console_state()
                        .lock()
                        .expect("console state poisoned")
                        .request_urls
                        .get(id)
                        .cloned()
                })
                .unwrap_or_default();
            push_netfailure(NetFailure {
                url,
                status: 0,
                error: params
                    .get("errorText")
                    .and_then(Value::as_str)
                    .unwrap_or("loading failed")
                    .to_string(),
                generation,
            });
            true
        }
        _ => false,
    }
}

fn decrement_in_flight() {
    let mut s = console_state().lock().expect("console state poisoned");
    if s.in_flight > 0 {
        s.in_flight -= 1;
    }
}

/// Current in-flight request count (for `network_idle` waits).
pub(super) fn in_flight_requests() -> i64 {
    console_state()
        .lock()
        .expect("console state poisoned")
        .in_flight
}

/// Status of the most recent main-document response (for `navigate` http_status).
pub(super) fn last_document_status() -> Option<i64> {
    console_state()
        .lock()
        .expect("console state poisoned")
        .last_doc_status
}

/// Reset the tracked main-document status — called at the start of a navigation so
/// its `http_status` can't report a stale value from an earlier page (F6 fix).
pub(super) fn clear_last_doc_status() {
    console_state()
        .lock()
        .expect("console state poisoned")
        .last_doc_status = None;
}

/// Start the diagnostics listener for the local browser once (idempotent). Runs
/// for the process lifetime, reconnecting if the CDP socket drops, so console /
/// network events are captured continuously rather than only during an op.
pub(super) fn ensure_console_listener(base: &str) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let base = base.to_string();
    tokio::spawn(async move {
        loop {
            let _ = run_console_listener(&base).await;
            // socket dropped (navigation/close) — back off and reconnect.
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    });
}

/// One connect→listen pass: enable the diagnostics domains and feed every event
/// into [`ingest_event`] until the socket closes.
async fn run_console_listener(base: &str) -> Result<()> {
    let ws_url = discover_page_ws_url(base).await?;
    let (ws, _) = tokio_tungstenite::connect_async(&ws_url).await?;
    let (mut write, mut read) = ws.split();
    for (id, method) in [
        (1, "Runtime.enable"),
        (2, "Log.enable"),
        (3, "Network.enable"),
    ] {
        write
            .send(Message::Text(
                json!({ "id": id, "method": method }).to_string(),
            ))
            .await?;
    }
    while let Some(frame) = read.next().await {
        let Message::Text(txt) = frame? else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else {
            continue;
        };
        if let Some(method) = v.get("method").and_then(Value::as_str) {
            let params = v.get("params").cloned().unwrap_or(Value::Null);
            ingest_event(method, &params);
        }
    }
    Ok(())
}

/// `get_console`: buffered console entries + network failures, filtered by
/// `min_level` (default "warning") and `since_generation` (default 0 = all).
pub(crate) async fn cdp_get_console(args: &Value) -> Result<Value> {
    let min_rank = level_rank(
        args.get("min_level")
            .and_then(Value::as_str)
            .unwrap_or("warning"),
    );
    let since = args
        .get("since_generation")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let s = console_state().lock().expect("console state poisoned");
    let entries: Vec<Value> = s
        .console
        .iter()
        .filter(|e| e.generation >= since && level_rank(&e.level) >= min_rank)
        .map(|e| {
            let mut o = json!({ "level": e.level, "text": e.text, "source": e.source });
            if let Some(u) = &e.url {
                o["url"] = json!(u);
            }
            if let Some(l) = e.line {
                o["line"] = json!(l);
            }
            o
        })
        .collect();
    let network_failures: Vec<Value> = s
        .network
        .iter()
        .filter(|f| f.generation >= since)
        .map(|f| json!({ "url": f.url, "status": f.status, "error": f.error }))
        .collect();
    Ok(json!({ "entries": entries, "network_failures": network_failures }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_level_and_rank() {
        assert_eq!(normalize_level("error"), "error");
        assert_eq!(normalize_level("assert"), "error");
        assert_eq!(normalize_level("warning"), "warning");
        assert_eq!(normalize_level("log"), "info");
        assert_eq!(normalize_level("debug"), "info");
        assert!(level_rank("error") > level_rank("warning"));
        assert!(level_rank("warning") > level_rank("info"));
        // default min_level "warning" excludes info.
        assert!(level_rank("info") < level_rank("warning"));
    }

    #[test]
    fn console_event_parsing_covers_console_exception_and_log() {
        let e = console_entry_from_event(
            "Runtime.consoleAPICalled",
            &json!({ "type": "error", "args": [{"type":"string","value":"boom"}],
                     "stackTrace": {"callFrames":[{"url":"http://x/app.js","lineNumber":12}]} }),
            5,
        )
        .expect("console entry");
        assert_eq!(e.level, "error");
        assert_eq!(e.text, "boom");
        assert_eq!(e.source, "console");
        assert_eq!(e.url.as_deref(), Some("http://x/app.js"));
        assert_eq!(e.line, Some(12));
        assert_eq!(e.generation, 5);

        let l = console_entry_from_event(
            "Runtime.consoleAPICalled",
            &json!({ "type": "log", "args": [{"type":"string","value":"hi"}] }),
            1,
        )
        .unwrap();
        assert_eq!(l.level, "info");

        let x = console_entry_from_event(
            "Runtime.exceptionThrown",
            &json!({ "exceptionDetails": { "exception": {"description":"TypeError: x"},
                     "url":"u", "lineNumber": 3 } }),
            2,
        )
        .unwrap();
        assert_eq!(x.level, "error");
        assert!(x.text.contains("TypeError"));
        assert_eq!(x.source, "exception");

        let log = console_entry_from_event(
            "Log.entryAdded",
            &json!({ "entry": { "source":"network", "level":"warning", "text":"slow", "url":"u" } }),
            3,
        )
        .unwrap();
        assert_eq!(log.level, "warning");
        assert_eq!(log.source, "network");

        assert!(console_entry_from_event("Page.loadEventFired", &json!({}), 1).is_none());
    }

    #[test]
    fn console_args_join_strings_and_objects() {
        let t = console_args_text(Some(&json!([
            {"type":"string","value":"count"},
            {"type":"number","value":42},
            {"type":"object","description":"[object Object]"}
        ])));
        assert!(t.contains("count"));
        assert!(t.contains("42"));
        assert!(t.contains("[object Object]"));
    }

    #[test]
    fn net_failure_only_for_4xx_5xx() {
        let f = net_failure_from_response(
            &json!({ "response": { "url":"http://x/missing.js", "status":404, "statusText":"Not Found" } }),
            7,
        )
        .expect("404 is a failure");
        assert_eq!(f.status, 404);
        assert_eq!(f.url, "http://x/missing.js");
        assert_eq!(f.error, "Not Found");
        assert_eq!(f.generation, 7);
        assert!(
            net_failure_from_response(&json!({ "response": { "url":"u", "status":200 } }), 1)
                .is_none()
        );
    }
}
