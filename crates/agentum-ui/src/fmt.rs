//! Times and sizes for people.

/// Now, in milliseconds since the epoch.
pub fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

/// "now", "5m", "3h", "2d", or "Nov 4" for a time in seconds.
pub fn ago(ts_secs: i64) -> String {
    ago_ms(ts_secs * 1000)
}

/// Like [`ago`], for milliseconds.
pub fn ago_ms(ts_ms: i64) -> String {
    if ts_ms <= 0 {
        return String::new();
    }
    let s = (now_ms() - ts_ms) / 1000;
    if s < 60 {
        "now".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else if s < 86_400 * 30 {
        format!("{}d", s / 86_400)
    } else {
        let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts_ms as f64));
        const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("{} {}", M[d.get_month() as usize % 12], d.get_date())
    }
}

/// "14:05" (local time) for milliseconds.
pub fn clock(ts_ms: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts_ms as f64));
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}

/// "1.2 MB".
pub fn bytes(n: u64) -> String {
    let f = n as f64;
    if f < 1024.0 {
        format!("{n} B")
    } else if f < 1024.0 * 1024.0 {
        format!("{:.0} KB", f / 1024.0)
    } else if f < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", f / 1024.0 / 1024.0)
    } else {
        format!("{:.1} GB", f / 1024.0 / 1024.0 / 1024.0)
    }
}

/// "12.3k" tokens.
pub fn tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    }
}
