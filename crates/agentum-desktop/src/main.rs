//! agentum desktop — a native Tauri window over an in-process agentum daemon.
//!
//! Boots `agentum-server` on a free loopback port (plain HTTP, auth disabled —
//! only this machine can reach a loopback bind), waits for it to start
//! listening, then opens a Tauri webview window on the embedded dashboard. The
//! daemon runs on a background Tokio runtime; Tauri's event loop owns the main
//! OS thread (required on macOS).
//!
//! Features: system tray icon (hide-to-tray on close), native menu bar
//! (File/View/Help), updater plugin, window state persistence, graceful
//! shutdown with daemon teardown, and `--headless` mode for daemon-only.
//!
//! ## Dual-mode binary
//!
//! - **Windowed (default):** Double-click the app or run `agentum-desktop`
//!   → Tauri window with embedded dashboard, tray icon, native menus.
//! - **Headless:** `agentum-desktop --headless [--port PORT]`
//!   → Daemon only, no window. Useful for VPS/server deployments where
//!   you want the same binary but no GUI.

// Hide the extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;

// ── CLI ──────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "agentum-desktop",
    about = "Native desktop shell for agentum",
    version
)]
struct Cli {
    /// Run in headless mode (daemon only, no GUI window)
    #[arg(long, env = "AGENTUM_HEADLESS")]
    headless: bool,

    /// Port for the daemon (default: auto-assign free port).
    /// When headless, consider binding to a fixed port for scriptability.
    #[arg(long, short = 'p', default_value_t = 0)]
    port: u16,

    /// Bind address (default: 127.0.0.1 for security).
    /// Set to 0.0.0.0 for LAN access when headless.
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,
}

// ── main ─────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set the data directory to the platform-appropriate path before
    // anything else touches the filesystem. agentum_store::open_default()
    // reads AGENTUM_DATA_DIR; if unset it falls back to the platform
    // default, which is the same logic we use here — setting it explicitly
    // gives us ownership of the log path and makes the directory visible
    // to "Open Data Directory" / "Open Logs" menu items.
    if let Some(data_dir) = data_dir() {
        std::fs::create_dir_all(&data_dir).ok();
        // SAFETY: set_var before any threads read it, during single-threaded init.
        unsafe {
            std::env::set_var("AGENTUM_DATA_DIR", &data_dir);
        }
    }

    // A macOS app launched from Finder/LaunchServices inherits a minimal PATH
    // (/usr/bin:/bin:/usr/sbin:/sbin) that omits Homebrew — so the bundled
    // daemon can't find tmux/git/agent CLIs. Prepend the usual locations.
    #[cfg(target_os = "macos")]
    {
        let extra = "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/local/sbin";
        let cur = std::env::var("PATH").unwrap_or_default();
        if !cur.split(':').any(|p| p == "/opt/homebrew/bin") {
            // SAFETY: single-threaded init, before any daemon thread reads PATH.
            unsafe {
                std::env::set_var("PATH", format!("{extra}:{cur}"));
            }
        }
    }

    // Reserve the port (0 = auto-assign a free one).
    let port = if cli.port == 0 {
        let l = TcpListener::bind("127.0.0.1:0")
            .context("No free loopback port available — is another process using all ports?")?;
        l.local_addr()?.port()
    } else {
        // Verify the port is free before handing off to the daemon.
        TcpListener::bind(SocketAddr::new(cli.bind, cli.port))
            .with_context(|| format!("Port {} is already in use", cli.port))?;
        cli.port
    };

    // Background runtime for the daemon.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to create async runtime — check system resources")?;

    // Start the daemon.
    let daemon_handle = rt.spawn(async move {
        if let Err(e) = run_daemon(port, cli.bind).await {
            eprintln!("agentum-desktop: daemon exited with error: {e:#}");
            eprintln!("Check that tmux is installed and the database is writable.");
        }
    });

    // Block until the daemon is listening (or timeout).
    let bind_addr = SocketAddr::new(cli.bind, port);
    if !wait_for_listener(&rt, bind_addr, Duration::from_secs(15)) {
        anyhow::bail!(
            "Daemon did not start within 15 seconds.\n\
             Check that tmux is installed and the database directory is writable:\n  \
             data dir: {}\n  \
             Try: agentum-desktop --headless for verbose daemon output.",
            data_dir()
                .unwrap_or_else(|| PathBuf::from("unknown"))
                .display()
        );
    }

    // ── Headless mode: daemon only, no GUI ────────────────────────────
    if cli.headless {
        let url = if cli.bind == IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)) {
            // When binding to all interfaces, try to show the LAN IP
            let lan_ip = detect_lan_ip();
            format!("http://{}:{}/", lan_ip, port)
        } else {
            format!("http://{}:{}/", cli.bind, port)
        };
        eprintln!("agentum-desktop: daemon listening on {url}");
        eprintln!("agentum-desktop: dashboard → {url}");
        eprintln!("agentum-desktop: Press Ctrl+C to stop");

        // Block until Ctrl+C, then graceful shutdown.
        rt.block_on(async {
            tokio::signal::ctrl_c().await.ok();
        });
        rt.shutdown_timeout(Duration::from_secs(3));
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let _ = daemon_handle.await;
            });
        return Ok(());
    }

    // ── Windowed mode: Tauri GUI ──────────────────────────────────────
    let url = format!("http://127.0.0.1:{port}/");

    // NOTE: no updater plugin. Tauri's updater requires a `pubkey` in
    // tauri.conf.json, and we have no release-signing pipeline yet. Wiring it
    // in half-way (endpoints but no pubkey) made the plugin fail to
    // initialize, which made `build()` return Err and the window never
    // opened. Re-add it as a unit — plugin + config `pubkey` + CI signing —
    // when auto-update is ready.
    let app_result = tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .setup(move |app| {
            use tauri::{
                menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder},
                tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
                Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
            };

            // --------------- Main window ---------------------------------
            let parsed: tauri::Url = url.parse().context("parse loopback url")?;
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(parsed))
                .title("agentum")
                .inner_size(1280.0, 820.0)
                .min_inner_size(480.0, 320.0)
                .visible(true)
                .build()
                .context("create main window")?;

            // --------------- System tray ---------------------------------
            let tray_show = MenuItemBuilder::with_id("tray_show", "Show").build(app)?;
            let tray_hide = MenuItemBuilder::with_id("tray_hide", "Hide").build(app)?;
            let tray_quit = MenuItemBuilder::with_id("tray_quit", "Quit").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .item(&tray_show)
                .item(&tray_hide)
                .item(&PredefinedMenuItem::separator(app)?)
                .item(&tray_quit)
                .build()?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().unwrap())
                .tooltip("agentum")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "tray_show" => { if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.set_focus(); } }
                    "tray_hide" => { if let Some(w) = app.get_webview_window("main") { let _ = w.hide(); } }
                    "tray_quit" => { app.exit(0); }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) { let _ = w.hide(); }
                            else { let _ = w.show(); let _ = w.set_focus(); }
                        }
                    }
                })
                .build(app)?;

            // --------------- Native menu bar -----------------------------
            let file_menu = SubmenuBuilder::new(app, "File").quit().build()?;
            let view_menu = SubmenuBuilder::new(app, "View")
                .item(&MenuItemBuilder::with_id("reload", "Reload").accelerator("CmdOrCtrl+R").build(app)?)
                .item(&MenuItemBuilder::with_id("toggle_devtools", "Toggle DevTools").accelerator("CmdOrCtrl+Shift+I").build(app)?)
                .build()?;
            let help_menu = SubmenuBuilder::new(app, "Help")
                .item(&MenuItemBuilder::with_id("about", "About agentum").build(app)?)
                .item(&MenuItemBuilder::with_id("open_data_dir", "Open Data Directory").build(app)?)
                .item(&MenuItemBuilder::with_id("open_logs", "Open Logs").build(app)?)
                .build()?;
            app.set_menu(
                MenuBuilder::new(app).item(&file_menu).item(&view_menu).item(&help_menu).build()?
            )?;

            app.on_menu_event(|app, event| {
                match event.id().as_ref() {
                    "reload" => { if let Some(w) = app.get_webview_window("main") { let _ = w.eval("location.reload()"); } }
                    "toggle_devtools" => {
                        if let Some(w) = app.get_webview_window("main") {
                            if w.is_devtools_open() { w.close_devtools(); } else { w.open_devtools(); }
                        }
                    }
                    "about" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.eval("alert('agentum — self-hosted control plane for AI coding agents\\n\\nVersion: 0.10.5\\nhttps://github.com/mateocerquetella/agentum')");
                        }
                    }
                    "open_data_dir" => { if let Some(d) = data_dir() { open_path(&d); } }
                    "open_logs" => {
                        if let Some(d) = data_dir() { let p = d.join("logs"); let _ = std::fs::create_dir_all(&p); open_path(&p); }
                    }
                    _ => {}
                }
            });

            // --------------- Window close → hide to tray -----------------
            let w = window.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = w.hide();
                }
            });

            // --------------- Native notifications bridge -----------------
            // Subscribe to the daemon's event bus and raise OS notifications
            // for agent-lifecycle events (finished / awaiting input / crashed)
            // so the user is alerted even when the window is hidden to tray.
            let notif_handle = app.handle().clone();
            tauri::async_runtime::spawn(notification_bridge(notif_handle, port));

            Ok(())
        })
        .build(tauri::generate_context!());

    // Surface a build failure explicitly and exit non-zero. We deliberately
    // don't use `?` here: returning an Err would drop the multi-thread `rt` on
    // the way out of main, and that drop blocks forever waiting on the
    // host-metrics `spawn_blocking` ticker (an uncancellable `loop`) — hanging
    // the process *before* the error is ever printed. `process::exit` skips
    // the blocking drop so the failure is actually reported.
    let app = match app_result {
        Ok(a) => a,
        Err(e) => {
            eprintln!("agentum-desktop: failed to start the window: {e:#}");
            std::process::exit(1);
        }
    };

    // ---- Run event loop (blocks until the app quits) ---------------------
    app.run(|_app_handle, _event| {});

    // The app is quitting. Tear down the daemon runtime without blocking: the
    // host-metrics `spawn_blocking` ticker can't be cancelled, so a plain drop
    // (or `shutdown_timeout`) would stall on quit. `shutdown_background`
    // abandons outstanding tasks and returns immediately — fine, the process
    // is on its way out anyway.
    // We don't await the daemon task — the runtime is about to be abandoned.
    // Explicit `drop` (not `let _ =`) avoids clippy's let_underscore_future.
    drop(daemon_handle);
    rt.shutdown_background();

    Ok(())
}

// ── Daemon ───────────────────────────────────────────────────────────────

/// Open the local store and serve the API + dashboard on the given bind.
async fn run_daemon(port: u16, bind: IpAddr) -> Result<()> {
    let (store, _db_path) = agentum_store::open_default().await.context(
        "Failed to open or create the agentum database.\n\
                  Check that the data directory is writable and not on a read-only filesystem.",
    )?;

    let addr = SocketAddr::new(bind, port);
    // cert_addr unused when tls=false; bind ephemeral.
    let cert_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

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

// ── Helpers ──────────────────────────────────────────────────────────────

/// Block until the daemon is accepting connections or the deadline passes.
/// Returns true if the daemon is up.
fn wait_for_listener(rt: &tokio::runtime::Runtime, addr: SocketAddr, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    rt.block_on(async move {
        loop {
            if Instant::now() >= deadline {
                return false;
            }
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
}

/// Return the agentum data directory (platform-appropriate).
fn data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
            Some(PathBuf::from(dir).join("agentum"))
        } else {
            home_dir().map(|home| home.join(".local").join("share").join("agentum"))
        }
    }
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("agentum")
        })
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA")
            .ok()
            .map(|d| PathBuf::from(d).join("agentum"))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        home_dir().map(|h| h.join(".agentum"))
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// Subscribe to the daemon's `/api/events` WS and raise native OS
/// notifications for the agent-lifecycle events worth interrupting the user
/// for. Runs for the app's lifetime, reconnecting with a short backoff if the
/// socket drops (e.g. during the daemon's startup window). The daemon is a
/// no-auth loopback bind, so the events stream needs no token.
async fn notification_bridge(app: tauri::AppHandle, port: u16) {
    use futures_util::StreamExt;
    use tauri_plugin_notification::NotificationExt;
    use tokio_tungstenite::tungstenite::Message;

    let url = format!("ws://127.0.0.1:{port}/api/events");
    loop {
        if let Ok((mut ws, _)) = tokio_tungstenite::connect_async(&url).await {
            while let Some(Ok(msg)) = ws.next().await {
                if let Message::Text(txt) = msg
                    && let Some((title, body)) = notification_for(&txt)
                {
                    let _ = app.notification().builder().title(title).body(body).show();
                }
            }
        }
        // Socket closed or never connected — back off, then retry.
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Map an event-bus JSON frame to a `(title, body)` when it's worth a
/// notification. Returns `None` for everything else — including the `replay`
/// frames the WS sends on connect, so we don't fire a burst of stale alerts.
fn notification_for(json: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v.get("replay").and_then(|r| r.as_bool()).unwrap_or(false) {
        return None;
    }
    let kind = v.get("kind")?.as_str()?;
    let name = v
        .get("session_name")
        .and_then(|x| x.as_str())
        .unwrap_or("a session");
    let title = match kind {
        "agent.finished" => "Agent finished",
        "agent.awaiting_input" => "Awaiting input",
        "session.crashed" => "Session crashed",
        _ => return None,
    };
    Some((title.to_string(), name.to_string()))
}

/// Open a path in the system file manager.
fn open_path(path: &std::path::Path) {
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = path;
    }
}

/// Best-effort LAN IP detection for headless mode.
fn detect_lan_ip() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(ip) = std::process::Command::new("hostname").arg("-I").output() {
            let s = String::from_utf8_lossy(&ip.stdout);
            if let Some(first) = s.split_whitespace().next() {
                if first.chars().all(|c| c.is_ascii_digit() || c == '.') {
                    return first.to_string();
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        for iface in &["en0", "en1"] {
            if let Ok(ip) = std::process::Command::new("ipconfig")
                .args(["getifaddr", iface])
                .output()
            {
                let s = String::from_utf8_lossy(&ip.stdout).trim().to_string();
                if !s.is_empty() {
                    return s;
                }
            }
        }
    }
    "127.0.0.1".to_string()
}
