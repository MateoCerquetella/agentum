//! agentum desktop — a native window over an in-process agentum daemon.
//!
//! Boots `agentum-server` on a free loopback port (plain HTTP, auth disabled —
//! only this machine can reach a loopback bind), waits for it to start
//! listening, then opens a webview window on the embedded dashboard. The daemon
//! runs on a background Tokio runtime; the GUI event loop owns the main OS
//! thread (required on macOS).

use std::net::TcpListener;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

fn main() -> Result<()> {
    // Background runtime for the daemon. `enable_all` gives it the IO + timer
    // drivers axum/tokio need; the GUI stays on the main OS thread below.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;

    // Reserve a free loopback port: bind :0, read the assigned port, drop the
    // listener so the daemon can take it. The tiny TOCTOU window is harmless on
    // loopback for a single-user desktop app.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").context("reserve free port")?;
        l.local_addr()?.port()
    };
    let url = format!("http://127.0.0.1:{port}/");

    // Spawn the daemon. It runs forever; if it exits early we log and the window
    // simply shows the webview's connection-error page.
    rt.spawn(async move {
        if let Err(e) = run_daemon(port).await {
            eprintln!("agentum-desktop: daemon exited: {e:#}");
        }
    });

    // Block until the daemon is listening (or give up after a few seconds and
    // open the window anyway — the webview retries on reload). Avoids a flash of
    // the "can't connect" page on a cold start.
    wait_for_listener(&rt, port, Duration::from_secs(10));

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("agentum")
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 820.0))
        .build(&event_loop)
        .context("create window")?;
    let _webview = WebViewBuilder::new(&window)
        .with_url(&url)
        .build()
        .context("create webview")?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}

/// Open the local store (creating + migrating the DB if needed) and serve the
/// API + embedded dashboard on `127.0.0.1:port`, plain HTTP with auth disabled.
async fn run_daemon(port: u16) -> Result<()> {
    let (store, _db_path) = agentum_store::open_default()
        .await
        .context("open agentum database")?;
    let addr = format!("127.0.0.1:{port}")
        .parse()
        .context("parse bind addr")?;
    // cert_addr is unused when tls=false; bind it to an ephemeral port anyway so
    // the field carries a valid value.
    let cert_addr = "127.0.0.1:0".parse().context("parse cert addr")?;
    agentum_server::serve(
        agentum_server::ServeOptions {
            addr,
            cert_addr,
            tls: false,
            no_auth: true,
        },
        store,
    )
    .await
}

/// Poll the loopback port until the daemon is accepting connections or the
/// deadline passes. A bare TCP connect is enough to know the listener is up; we
/// don't pull in an HTTP client just for the readiness probe.
fn wait_for_listener(rt: &tokio::runtime::Runtime, port: u16, budget: Duration) {
    let deadline = Instant::now() + budget;
    rt.block_on(async move {
        loop {
            if Instant::now() >= deadline {
                return;
            }
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}
