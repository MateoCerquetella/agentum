//! Pure, browser-independent helpers: small string/JSON/byte transforms used by
//! the op dispatch and node ops, unit-tested without a live CDP connection.
use super::*;

// --- pure helpers (unit-tested without a browser) ----------------------------

/// JS read for [`cdp_snapshot`] — returns a JSON string the driver parses. Text is
/// capped so a huge page can't blow up the MCP response.
pub(crate) const SNAPSHOT_EXPR: &str = "JSON.stringify({url:location.href,title:document.title,\
text:((document.body&&document.body.innerText)||'').slice(0,20000)})";

/// JS-string-literal encode (safe to embed in an eval'd expression). Mirrors the
/// desktop bridge's `js_string`.
pub(crate) fn js_string(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

/// Build `Emulation.setDeviceMetricsOverride` params from optional viewport args,
/// or `None` when the op requested no override. Both `width` and `height` are
/// required to override; dimensions floor at 1 (Chrome rejects 0), `mobile`
/// defaults false (desktop layout), `deviceScaleFactor` defaults 1.0. Pure so the
/// responsive-capture mapping is unit-tested without a browser.
pub(crate) fn device_metrics_params(args: &Value) -> Option<Value> {
    let width = args.get("width").and_then(Value::as_u64)?;
    let height = args.get("height").and_then(Value::as_u64)?;
    let dsf = args
        .get("deviceScaleFactor")
        .and_then(Value::as_f64)
        .filter(|d| *d > 0.0)
        .unwrap_or(1.0);
    let mobile = args.get("mobile").and_then(Value::as_bool).unwrap_or(false);
    Some(json!({
        "width": width.max(1),
        "height": height.max(1),
        "deviceScaleFactor": dsf,
        "mobile": mobile,
    }))
}

/// The JS predicate (returns bool) for a `wait` condition, or `None` for
/// `network_idle`/unknown (handled out of band). Pure so it's unit-tested.
pub(crate) fn wait_predicate_expr(condition: &str, arg: &str) -> Option<String> {
    match condition {
        "selector" => Some(format!("!!document.querySelector({})", js_string(arg))),
        "text" => Some(format!(
            "!!(document.body&&document.body.innerText.indexOf({})>=0)",
            js_string(arg)
        )),
        "url" => Some(format!("location.href.indexOf({})>=0", js_string(arg))),
        _ => None,
    }
}

/// Parse the `{u,t}` JSON string a navigate's `Runtime.evaluate` returns into
/// (final_url, title). Tolerant of a missing/garbled result.
pub(crate) fn parse_url_title(eval_result: &Value) -> (String, String) {
    let raw = eval_result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let parsed: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    (
        parsed
            .get("u")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        parsed
            .get("t")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    )
}

/// Read (width, height) from a PNG's IHDR header (big-endian u32s at byte 16/20),
/// or `None` if it isn't a PNG. Avoids pulling in an image-decode dependency.
pub(crate) fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[0..8] != SIG {
        return None;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((w, h))
}
