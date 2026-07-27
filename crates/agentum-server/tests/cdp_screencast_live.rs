//! LIVE integration test: drive a REAL headless CDP-Chromium through the
//! in-agentum screencast bridge and assert real `0x62` frames come back, then
//! that an input command dispatches without tearing the bridge down.
//!
//! `#[ignore]` — this launches a real Chromium process (needs
//! `npx playwright install chromium`) in a real tmux session, so it never runs in
//! CI. Run it explicitly:
//!
//!   cargo test -p agentum-server --test cdp_screencast_live -- --ignored --nocapture
//!
//! It uses the exact production path: [`cdp_browser::ensure_local_cdp_browser`]
//! (headless since 009c-3) to launch + resolve the CDP endpoint, then
//! [`cdp_screencast::run_screencast_bridge`] to attach, screencast, and bridge
//! input — the same code the WS route runs. Self-cleaning: tears the shared
//! browser down at the end.

use std::time::Duration;

use agentum_server::cdp_browser;
use agentum_server::cdp_screencast::{InputCommand, ScreencastOptions, run_screencast_bridge};
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;

/// Validate that `bytes` is a well-formed `0x62` screencast frame and return its
/// image-byte length — the same header layout `browser-screencast-protocol.ts`
/// decodes. A real frame has a non-empty JPEG payload.
fn assert_valid_frame(bytes: &[u8]) -> usize {
    assert!(bytes.len() >= 16, "frame shorter than the 16-byte header");
    assert_eq!(bytes[0], 0x62, "kind byte");
    assert_eq!(bytes[1], 1, "version byte");
    assert_eq!(bytes[2], 1, "opcode = Frame");
    assert!(matches!(bytes[3], 1 | 2), "format = jpeg|png");
    let md_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    assert_eq!(
        u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
        0,
        "reserved must be 0 or the pane drops the frame"
    );
    let image = &bytes[16 + md_len..];
    assert!(
        !image.is_empty(),
        "a real screencast frame carries image bytes"
    );
    image.len()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "launches a real Chromium process; run with --ignored"]
async fn headless_browser_screencasts_into_the_bridge_and_takes_input() {
    // Launch (or reuse) the shared headless CDP browser via the production path.
    // A missing Chromium install is a SKIP, not a failure — the message tells the
    // operator how to enable the test.
    let endpoint = match cdp_browser::ensure_local_cdp_browser().await {
        Ok(ep) => ep,
        Err(e) => {
            eprintln!("SKIP: could not launch headless CDP-Chromium: {e:#}");
            return;
        }
    };
    eprintln!("CDP endpoint: {endpoint}");

    // Frames use the same latest-wins `watch` sink the WS route uses (a slow
    // consumer never stalls Chrome); `None` is the pre-first-frame sentinel.
    let (frame_tx, mut frame_rx) = watch::channel::<Option<Vec<u8>>>(None);
    let (input_tx, input_rx) = mpsc::channel::<InputCommand>(8);

    // Run the bridge exactly as the WS route does.
    let mut bridge = tokio::spawn(async move {
        run_screencast_bridge(&endpoint, ScreencastOptions::default(), input_rx, frame_tx).await
    });

    // The first frame must arrive promptly (CDP emits one on screencast start).
    // `changed()` fires on the first real frame (the `None` initial value is
    // pre-seen); take the freshest frame, mirroring the WS route's consumer.
    match timeout(Duration::from_secs(15), frame_rx.changed()).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            let outcome = timeout(Duration::from_secs(2), &mut bridge).await;
            panic!("bridge closed before its first frame: {outcome:?}");
        }
        Err(_) => panic!("a screencast frame within 15s"),
    }
    let first = frame_rx
        .borrow_and_update()
        .clone()
        .expect("first change carries frame bytes");
    let n = assert_valid_frame(&first);
    eprintln!("first frame OK: {n} image bytes");

    // Drive navigation through the input back-channel; a real navigation repaints,
    // so we should keep getting frames — proving input reaches the same instance.
    input_tx
        .send(InputCommand::Goto {
            url: "data:text/html,<h1 style='font-size:80px'>agentum-009c-3</h1>".into(),
        })
        .await
        .expect("send nav command");

    timeout(Duration::from_secs(15), frame_rx.changed())
        .await
        .expect("a frame after navigation within 15s")
        .expect("bridge still streaming after input");
    let after_nav = frame_rx
        .borrow_and_update()
        .clone()
        .expect("post-nav change carries frame bytes");
    assert_valid_frame(&after_nav);
    eprintln!("post-navigation frame OK");

    // Clean close: dropping the input sender ends the bridge.
    drop(input_tx);
    let _ = timeout(Duration::from_secs(5), bridge).await;

    // Tear the shared browser down so the test leaves no process behind.
    let _ = cdp_browser::stop_local_cdp_browser().await;
}
