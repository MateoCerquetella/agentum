//! Native application menu.
//!
//! Why this exists: Tauri's *default* menu binds ⌘W ("Close Window") as a
//! native key-equivalent. On macOS AppKit dispatches menu key-equivalents via
//! `performKeyEquivalent:` BEFORE the keystroke ever reaches the WKWebView, so
//! the webview's own ⌘W handler — which closes the active editor tab, browser
//! tab, or terminal pane (see `Terminal.tsx` / `terminal-pane/keyboard-handlers.ts`)
//! and `preventDefault()`s — never ran. The window closed out from under the
//! user even when they only meant to close the file they were on.
//!
//! This menu mirrors the platform default in every other respect, but it does
//! NOT bind ⌘W to any menu item. With no native owner, ⌘W flows into the
//! webview and the existing tab-close logic takes over — VS Code behavior
//! (⌘W = close active tab/file). An explicit "Close Window" remains under
//! ⌘⇧W (matching VS Code's `workbench.action.closeWindow`) so there is still a
//! discoverable keyboard path to close the window itself.
//!
//! The predefined `close_window` item can't be rebound — muda hardcodes its
//! ⌘W accelerator — so we replicate `tauri::menu::Menu::default` by hand and
//! swap in a custom item instead.

use tauri::menu::{
    AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID,
    WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Manager, Runtime};

/// Menu id for our custom "Close Window" item (⌘⇧W). Matched in
/// [`on_menu_event`] to close the focused window.
pub const CLOSE_WINDOW_MENU_ID: &str = "agentum:close-window";

/// Build the application menu. Installed via `Builder::menu` in `lib.rs`.
///
/// Mirrors `tauri::menu::Menu::default` with one deliberate change: the ⌘W
/// `close_window` predefined items are dropped, and a single "Close Window"
/// item is added under ⌘⇧W. Keeping the `WINDOW_SUBMENU_ID` / `HELP_SUBMENU_ID`
/// ids lets Tauri still register the native macOS Window and Help menus.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let pkg_info = app.package_info();
    let config = app.config();
    let about_metadata = AboutMetadata {
        name: Some(pkg_info.name.clone()),
        version: Some(pkg_info.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };

    // ⌘⇧W instead of the default ⌘W. Leaving ⌘W unbound at the native level is
    // the entire point: the webview owns ⌘W and closes the active tab/file.
    let close_window = MenuItem::with_id(
        app,
        CLOSE_WINDOW_MENU_ID,
        "Close Window",
        true,
        Some("CmdOrCtrl+Shift+W"),
    )?;

    let window_menu = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
        ],
    )?;

    let help_menu = Submenu::with_id_and_items(
        app,
        HELP_SUBMENU_ID,
        "Help",
        true,
        &[
            #[cfg(not(target_os = "macos"))]
            &PredefinedMenuItem::about(app, None, Some(about_metadata.clone()))?,
        ],
    )?;

    Menu::with_items(
        app,
        &[
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                pkg_info.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, Some(about_metadata))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?,
            // "File" exists on macOS and Windows in the default menu; it's the
            // conventional home for "Close Window".
            #[cfg(not(any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd"
            )))]
            &Submenu::with_items(
                app,
                "File",
                true,
                &[
                    &close_window,
                    #[cfg(not(target_os = "macos"))]
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(app, None)?,
                    &PredefinedMenuItem::redo(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::select_all(app, None)?,
                ],
            )?,
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &window_menu,
            &help_menu,
        ],
    )
}

/// Handle menu events. Only our custom "Close Window" item needs handling;
/// every other entry is a predefined item AppKit/muda actions natively.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    if event.id() == CLOSE_WINDOW_MENU_ID {
        // Single-window app: close "main" like the red traffic-light button
        // does (`ui_request_close`). `close()` fires the webview's
        // `beforeunload`, which persists scrollback before the window goes away.
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.close();
        }
    }
}
