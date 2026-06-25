//! LIVE integration test: drive a REAL headless CDP-Chromium and assert the
//! `node_at_point` op — the agent-browser annotate picker's hit-test — resolves a
//! known element at a viewport pixel and captures a sharp element screenshot.
//!
//! `#[ignore]` — launches a real Chromium process (needs `npx playwright install
//! chromium` or system Chrome), so it never runs in CI. Run it explicitly:
//!
//!   cargo test -p agentum-server --test node_at_point_live -- --ignored --nocapture
//!
//! Uses the production op path (`cdp_driver::run_browser_op`), the same one the MCP
//! tool and the `/api/cdp-browser/node-at-point` route call. Self-cleaning: tears
//! the shared browser down at the end.

use agentum_server::cdp_browser;
use agentum_server::cdp_driver::run_browser_op;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "launches a real Chromium process; run with --ignored"]
async fn node_at_point_resolves_a_known_element_and_captures_it() {
    // Launch (or reuse) the shared headless CDP browser via the production path.
    // A missing Chromium install is a SKIP, not a failure.
    if let Err(e) = cdp_browser::ensure_local_cdp_browser().await {
        eprintln!("SKIP: could not launch headless CDP-Chromium: {e:#}");
        return;
    }

    // A page with a button at a KNOWN viewport box: left 50..250, top 60..140.
    let url = "data:text/html,<body style='margin:0'>\
        <button id='go' style='position:absolute;left:50px;top:60px;width:200px;height:80px'>Click</button>\
        </body>";
    let nav = run_browser_op("navigate", &json!({ "url": url }))
        .await
        .expect("navigate to the test page");
    eprintln!("navigate: {nav}");

    // Hit-test a pixel INSIDE the button (its center ~150,100) and capture it.
    let r = run_browser_op(
        "node_at_point",
        &json!({ "x": 150.0, "y": 100.0, "capture": true }),
    )
    .await
    .expect("node_at_point call");
    eprintln!("node_at_point: {r}");

    assert_eq!(r["ok"], true, "should resolve a node at the button pixel");

    // Label names the element (e.g. `button#go`).
    let label = r["label"].as_str().unwrap_or("");
    assert!(
        label.contains("button"),
        "label should name the element, got {label:?}"
    );

    // Clip is roughly the button's box (200x80; lenient for UA border/padding).
    let w = r["clip"]["width"].as_f64().unwrap_or(0.0);
    let h = r["clip"]["height"].as_f64().unwrap_or(0.0);
    assert!(w >= 100.0, "clip width should be ~200, got {w}");
    assert!(h >= 40.0, "clip height should be ~80, got {h}");

    // `capture:true` wrote a real PNG and returned its path + base64.
    let path = r["path"].as_str().expect("screenshot path present");
    assert!(
        std::path::Path::new(path).exists(),
        "screenshot file should exist on disk: {path}"
    );
    assert!(
        r["image_b64"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "image_b64 should be present"
    );
    assert!(
        r["image_width"].as_u64().unwrap_or(0) > 0,
        "captured PNG should report a width"
    );
    eprintln!("OK: label={label}, clip={w}x{h}, png={path}");

    // Tear the shared browser down so the test leaves no process behind.
    let _ = cdp_browser::stop_local_cdp_browser().await;
}
