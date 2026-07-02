//! DOM node operations over a CdpConn: viewport, load-wait, backend-node
//! resolution, geometry, and ref-addressed click/type.
use super::*;

/// Apply a viewport override on `conn` when the op carries viewport args, so the
/// page lays out at the requested breakpoint before the snapshot/screenshot. A
/// no-op when no viewport was requested. The override is scoped to this
/// short-lived connection, so it clears on disconnect.
pub(crate) async fn apply_viewport(conn: &mut CdpConn, args: &Value) -> Result<()> {
    if let Some(metrics) = device_metrics_params(args) {
        conn.call("Emulation.setDeviceMetricsOverride", metrics)
            .await?;
    }
    Ok(())
}

/// Poll the page until it reaches `wait_until` (load|domcontentloaded|
/// network_idle) or `timeout_ms` elapses. Best-effort: a timeout doesn't fail the
/// navigation, it just means the page was still busy.
pub(crate) async fn wait_for_load(conn: &mut CdpConn, wait_until: &str, timeout_ms: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut idle_polls = 0u32;
    loop {
        let ready = conn
            .call(
                "Runtime.evaluate",
                json!({ "expression": "document.readyState", "returnByValue": true }),
            )
            .await
            .ok()
            .and_then(|r| {
                r.get("result")
                    .and_then(|x| x.get("value"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let interactive = ready == "interactive" || ready == "complete";
        let done = match wait_until {
            "domcontentloaded" => interactive,
            "network_idle" => {
                if interactive && in_flight_requests() <= 2 {
                    idle_polls += 1;
                    idle_polls >= 3
                } else {
                    idle_polls = 0;
                    false
                }
            }
            _ => ready == "complete",
        };
        if done || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Resolve a `backendDOMNodeId` to a JS RemoteObject `objectId` so we can call
/// element methods on it. Enables the DOM domain first (idempotent). `None` means
/// the node is gone (the page navigated) — the caller treats that as a stale ref.
pub(crate) async fn resolve_node_object(
    conn: &mut CdpConn,
    backend_node_id: i64,
) -> Result<Option<String>> {
    let _ = conn.call("DOM.enable", json!({})).await;
    let Ok(resolved) = conn
        .call(
            "DOM.resolveNode",
            json!({ "backendNodeId": backend_node_id }),
        )
        .await
    else {
        return Ok(None);
    };
    Ok(resolved
        .get("object")
        .and_then(|o| o.get("objectId"))
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// Scroll the resolved element into view and return its viewport-center (CSS px),
/// or `None` when it has no layout box (display:none / zero size). Coordinates are
/// in the same CSS-px space `Input.dispatchMouseEvent` expects.
async fn node_center(conn: &mut CdpConn, object_id: &str) -> Result<Option<(f64, f64)>> {
    let func = "function(){this.scrollIntoView({block:'center',inline:'center'});\
        var r=this.getBoundingClientRect();\
        return JSON.stringify({x:r.left+r.width/2,y:r.top+r.height/2,w:r.width,h:r.height});}";
    let res = conn
        .call(
            "Runtime.callFunctionOn",
            json!({ "objectId": object_id, "functionDeclaration": func, "returnByValue": true }),
        )
        .await?;
    let Some(raw) = res
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let rect: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let w = rect.get("w").and_then(Value::as_f64).unwrap_or(0.0);
    let h = rect.get("h").and_then(Value::as_f64).unwrap_or(0.0);
    if w <= 0.0 || h <= 0.0 {
        return Ok(None);
    }
    Ok(Some((
        rect.get("x").and_then(Value::as_f64).unwrap_or(0.0),
        rect.get("y").and_then(Value::as_f64).unwrap_or(0.0),
    )))
}

/// A `Page.captureScreenshot` `clip` for the resolved element (its viewport rect),
/// or `None` when it has no layout box. Scrolls it into view first.
pub(crate) async fn node_clip(conn: &mut CdpConn, object_id: &str) -> Result<Option<Value>> {
    let func = "function(){this.scrollIntoView({block:'center',inline:'center'});\
        var r=this.getBoundingClientRect();\
        return JSON.stringify({x:r.left,y:r.top,w:r.width,h:r.height});}";
    let res = conn
        .call(
            "Runtime.callFunctionOn",
            json!({ "objectId": object_id, "functionDeclaration": func, "returnByValue": true }),
        )
        .await?;
    let Some(raw) = res
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let rect: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let w = rect.get("w").and_then(Value::as_f64).unwrap_or(0.0);
    let h = rect.get("h").and_then(Value::as_f64).unwrap_or(0.0);
    if w <= 0.0 || h <= 0.0 {
        return Ok(None);
    }
    Ok(Some(json!({
        "x": rect.get("x").and_then(Value::as_f64).unwrap_or(0.0),
        "y": rect.get("y").and_then(Value::as_f64).unwrap_or(0.0),
        "width": w,
        "height": h,
        "scale": 1,
    })))
}

/// Click an element by snapshot ref with a TRUSTED mouse event at its center
/// (falls back to a synthetic `.click()` when it has no layout box). A stale ref
/// returns `stale_ref` so the agent re-snapshots.
pub(crate) async fn click_ref(conn: &mut CdpConn, ref_id: &str) -> Result<Value> {
    let Some(backend) = resolve_ref(ref_id) else {
        return Ok(stale_ref(ref_id));
    };
    let Some(object_id) = resolve_node_object(conn, backend).await? else {
        return Ok(stale_ref(ref_id));
    };
    match node_center(conn, &object_id).await? {
        Some((x, y)) => {
            for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
                conn.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": kind, "x": x, "y": y, "button": "left",
                            "buttons": buttons, "clickCount": 1 }),
                )
                .await?;
            }
            Ok(json!({ "ok": true, "ref": ref_id }))
        }
        None => {
            // No layout box (off-screen/hidden) — synthetic click is the best effort.
            conn.call(
                "Runtime.callFunctionOn",
                json!({ "objectId": object_id, "functionDeclaration": "function(){this.click();}" }),
            )
            .await?;
            Ok(json!({ "ok": true, "ref": ref_id, "synthetic": true }))
        }
    }
}

/// Type into an element by snapshot ref using TRUSTED key input: focus, then
/// `Input.insertText` (fires the input/change events frameworks listen for — unlike
/// a raw `el.value=`). `submit` presses Enter after. Stale ref → `stale_ref`.
pub(crate) async fn type_ref(
    conn: &mut CdpConn,
    ref_id: &str,
    text: &str,
    submit: bool,
) -> Result<Value> {
    let Some(backend) = resolve_ref(ref_id) else {
        return Ok(stale_ref(ref_id));
    };
    let Some(object_id) = resolve_node_object(conn, backend).await? else {
        return Ok(stale_ref(ref_id));
    };
    conn.call(
        "Runtime.callFunctionOn",
        json!({ "objectId": object_id, "functionDeclaration": "function(){this.focus();}" }),
    )
    .await?;
    if !text.is_empty() {
        conn.call("Input.insertText", json!({ "text": text }))
            .await?;
    }
    if submit {
        for kind in ["keyDown", "keyUp"] {
            conn.call(
                "Input.dispatchKeyEvent",
                json!({ "type": kind, "key": "Enter", "code": "Enter",
                        "windowsVirtualKeyCode": 13, "text": "\r" }),
            )
            .await?;
        }
    }
    Ok(json!({ "ok": true, "ref": ref_id, "submitted": submit }))
}

/// The standard stale-ref response — the ref's generation is gone, so the agent
/// must call `snapshot` again to get fresh refs.
pub(crate) fn stale_ref(ref_id: &str) -> Value {
    json!({ "ok": false, "error": "stale_ref", "ref": ref_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_ref_response_shape() {
        let v = stale_ref("e3_2");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "stale_ref");
        assert_eq!(v["ref"], "e3_2");
    }
}
