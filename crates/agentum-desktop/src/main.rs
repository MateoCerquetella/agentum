//! agentum desktop — a native Tauri window over an in-process agentum daemon.
//!
//! Boots `agentum-server` on a free loopback port (plain HTTP, auth disabled —
//! only this machine can reach a loopback bind), waits for it to start
//! listening, then opens a Tauri webview window on the embedded dashboard. The
//! daemon runs on a background Tokio runtime; Tauri's event loop owns the main
//! OS thread (required on macOS).
//!
//! Features: system tray icon (hide-to-tray on close), native menu bar
//! (File/View/Help), updater plugin, window state persistence, and graceful
//! shutdown with daemon teardown.

// Hide the extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::TcpListener;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_updater::UpdaterExt;

fn main() -> Result<()> {
    // Background runtime for the daemon. `enable_all` gives it the IO + timer
    // drivers axum/tokio need; the GUI stays on the main OS thread below.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;

    // Reserve a free loopback port.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").context("reserve free port")?;
        l.local_addr()?.port()
    };
    let url = format!("http://127.0.0.1:{port}/");

    // ---- Spawn the daemon ------------------------------------------------
    // The daemon runs on the background runtime. We keep the JoinHandle so we
    // can wait for graceful teardown on quit.
    let daemon_handle = rt.spawn(async move {
        if let Err(e) = run_daemon(port).await {
            eprintln!("agentum-desktop: daemon exited: {e:#}");
        }
    });

    // Block until the daemon is listening.
    wait_for_listener(&rt, port, Duration::from_secs(10));

    // ---- Build the Tauri application -------------------------------------
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(move |app| {
            // --------------- Create the main window ------------------------
            let parsed: tauri::Url = url.parse().context("parse loopback url")?;
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(parsed))
                .title("agentum")
                .inner_size(1280.0, 820.0)
                .min_inner_size(480.0, 320.0)
                .visible(true)
                .build()
                .context("create main window")?;

            // --------------- System tray icon -----------------------------
            let tray_show = MenuItemBuilder::with_id("tray_show", "Show").build(app)?;
            let tray_hide = MenuItemBuilder::with_id("tray_hide", "Hide").build(app)?;
            let tray_quit = MenuItemBuilder::with_id("tray_quit", "Quit").build(app)?;
            let tray_sep = PredefinedMenuItem::separator(app)?;

            let tray_menu = MenuBuilder::new(app)
                .item(&tray_show)
                .item(&tray_hide)
                .item(&tray_sep)
                .item(&tray_quit)
                .build()?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().unwrap())
                .tooltip("agentum")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "tray_show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "tray_hide" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                    "tray_quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            // --------------- Native menu bar ------------------------------
            let file_menu = SubmenuBuilder::new(app, "File")
                .quit()
                .build()?;

            let view_menu = SubmenuBuilder::new(app, "View")
                .item(&MenuItemBuilder::with_id("reload", "Reload")
                    .accelerator("CmdOrCtrl+R")
                    .build(app)?)
                .item(&MenuItemBuilder::with_id("toggle_devtools", "Toggle DevTools")
                    .accelerator("CmdOrCtrl+Shift+I")
                    .build(app)?)
                .build()?;

            let help_menu = SubmenuBuilder::new(app, "Help")
                .item(&MenuItemBuilder::with_id("about", "About agentum").build(app)?)
                .item(&MenuItemBuilder::with_id("open_data_dir", "Open Data Directory").build(app)?)
                .item(&MenuItemBuilder::with_id("open_logs", "Open Logs").build(app)?)
                .build()?;

            let menu = MenuBuilder::new(app)
                .item(&file_menu)
                .item(&view_menu)
                .item(&help_menu)
                .build()?;

            app.set_menu(menu)?;

            // Menu event handler (on the app, not the window, for global shortcuts)
            app.on_menu_event(move |app, event| {
                match event.id().as_ref() {
                    "reload" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.eval("location.reload()");
                        }
                    }
                    "toggle_devtools" => {
                        if let Some(w) = app.get_webview_window("main") {
                            if w.is_devtools_open() {
                                w.close_devtools();
                            } else {
                                w.open_devtools();
                            }
                        }
                    }
                    "about" => {
                        // Show a simple about dialog via a Tauri dialog or console
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.eval(
                                "alert('agentum — self-hosted control plane for AI coding agents\\n\\nVersion: 0.10.5\\nhttps://github.com/mateocerquetella/agentum')",
                            );
                        }
                    }
                    "open_data_dir" => {
                        let data_dir = dirs_next();
                        if let Some(dir) = data_dir {
                            open_path(&dir);
                        }
                    }
                    "open_logs" => {
                        let log_dir = dirs_next().map(|d| d.join("logs"));
                        if let Some(dir) = log_dir {
                            let _ = std::fs::create_dir_all(&dir);
                            open_path(&dir);
                        }
                    }
                    _ => {}
                }
            });

            // --------------- Window close → hide to tray ------------------
            let window_clone = window.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    // Hide to tray instead of closing
                    api.prevent_close();
                    let _ = window_clone.hide();
                }
            });

            // --------------- Check for updates on startup -----------------
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match handle.updater() {
                    Ok(updater) => {
                        if let Ok(Some(update)) = updater.check().await {
                            if let Some(w) = handle.get_webview_window("main") {
                                let msg = format!(
                                    "A new version of agentum is available: {} → {}.\n\nVisit the releases page to download.",
                                    update.current_version,
                                    update.version
                                );
                                let _ = w.eval(&format!("alert({:?})", msg));
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("agentum-desktop: updater check failed: {e}");
                    }
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())?;

    // ---- Run the event loop (blocking) -----------------------------------
    let mut rt = Some(rt);
    app.run(move |_app_handle, event| {
        if let RunEvent::Exit = event {
            // The app is shutting down. Drop the Tokio runtime so the daemon
            // task is cancelled. shutdown_timeout gives in-flight connections
            // a brief window to finish.
            if let Some(rt) = rt.take() {
                rt.shutdown_timeout(Duration::from_secs(3));
            }
        }
    });

    // Wait for the daemon task to actually finish after runtime shutdown.
    // Block on a short-lived current-thread runtime so we don't need the
    // multi-thread runtime anymore.
    let _ = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            let _ = daemon_handle.await;
        });

    Ok(())
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
/// deadline passes.
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

/// Return the agentum data directory (platform-appropriate).
fn dirs_next() -> Option<std::path::PathBuf> {
    // Use the directories crate pattern: XDG_DATA_HOME / platform data dir
    #[cfg(target_os = "linux")]
    {
        if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
            Some(std::path::PathBuf::from(dir).join("agentum"))
        } else if let Some(home) = dirs_next_home() {
            Some(home.join(".local").join("share").join("agentum"))
        } else {
            None
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs_next_home().map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("agentum")
        })
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(dir) = std::env::var("APPDATA") {
            Some(std::path::PathBuf::from(dir).join("agentum"))
        } else {
            None
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        dirs_next_home().map(|h| h.join(".agentum"))
    }
}

fn dirs_next_home() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(std::path::PathBuf::from)
}

/// Open a path in the system file manager using the platform default.
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
        let _ = path; // no-op on unknown platforms
    }
}
