//! Optional companion tools the terminal dashboard can launch in a side
//! pane (currently just lazygit). Extensions are detected at runtime; if
//! missing, we render a platform-aware install hint instead of silently
//! failing.

use std::path::Path;

/// A launchable companion tool.
pub struct Extension {
    pub id: &'static str,
    pub display_name: &'static str,
    pub binary: &'static str,
    /// Args to pass when spawning. `{cwd}` is substituted with the absolute
    /// working directory.
    pub args: &'static [&'static str],
    /// Short blurb shown in help / install overlay.
    pub blurb: &'static str,
    /// Curated cheat-sheet rows (label, keys). Drawn in the help overlay.
    pub cheatsheet: &'static [(&'static str, &'static str)],
    /// Project home page — printed verbatim in the install overlay.
    pub homepage: &'static str,
}

/// The lazygit extension definition. Args use `-p <cwd>` so lazygit opens
/// scoped to the active session's workdir.
pub const LAZYGIT: Extension = Extension {
    id: "lazygit",
    display_name: "lazygit",
    binary: "lazygit",
    args: &["-p", "{cwd}"],
    blurb: "Fast, keyboard-first git UI. Stages, commits, branches, rebases, cherry-picks.",
    cheatsheet: &[
        ("switch panel",     "1 2 3 4 5"),
        ("up / down",        "k j   or ↑ ↓"),
        ("stage / unstage",  "space"),
        ("commit",           "c"),
        ("amend",            "A"),
        ("push / pull",      "P / p"),
        ("checkout branch",  "space  (in branches)"),
        ("new branch",       "n"),
        ("merge / rebase",   "M / r"),
        ("show keybindings", "?"),
        ("quit lazygit",     "q"),
    ],
    homepage: "https://github.com/jesseduffield/lazygit",
};

/// Did the extension's binary land on PATH?
pub fn is_installed(ext: &Extension) -> bool {
    which::which(ext.binary).is_ok()
}

/// Final argv with `{cwd}` substituted. Empty cwd falls back to the
/// current process directory.
pub fn resolve_args(ext: &Extension, cwd: &Path) -> Vec<String> {
    let cwd_str = cwd.to_string_lossy();
    ext.args
        .iter()
        .map(|a| a.replace("{cwd}", &cwd_str))
        .collect()
}

/// Platform-specific install commands. We list the most common package
/// managers per OS, plus a portable fallback. The strings are shown
/// verbatim — the user copy-pastes one of them.
pub fn install_hints(ext: &Extension) -> Vec<(&'static str, String)> {
    let bin = ext.binary;
    let mut hints: Vec<(&'static str, String)> = Vec::new();

    if cfg!(target_os = "macos") {
        hints.push(("Homebrew", format!("brew install {bin}")));
        hints.push(("MacPorts", format!("sudo port install {bin}")));
    } else if cfg!(target_os = "linux") {
        hints.push(("Homebrew (Linux)", format!("brew install {bin}")));
        hints.push(("Arch / pacman", format!("sudo pacman -S {bin}")));
        hints.push(("Debian / Ubuntu (recent)", format!("sudo apt install {bin}")));
        hints.push(("Fedora", format!("sudo dnf install {bin}")));
        hints.push(("Alpine", format!("sudo apk add {bin}")));
    } else if cfg!(target_os = "windows") {
        hints.push(("Scoop", format!("scoop install {bin}")));
        hints.push(("Chocolatey", format!("choco install {bin}")));
        hints.push((
            "winget",
            "winget install JesseDuffield.lazygit".to_string(),
        ));
    } else {
        hints.push(("Homebrew", format!("brew install {bin}")));
    }

    if ext.id == "lazygit" {
        hints.push(("Go", format!("go install github.com/jesseduffield/{bin}@latest")));
    }
    hints
}
