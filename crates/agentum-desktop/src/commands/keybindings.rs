use std::collections::BTreeMap;
use super::platform::platform_label;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// action id -> key chords. Mirrors KeybindingOverrides in agentum/src/shared/keybindings.ts.
type Overrides = BTreeMap<String, Vec<String>>;

// On-disk document at ~/.agentum/keybindings.json. The renderer only consumes the
// snapshot below, so this layout just needs to round-trip with itself.
#[derive(Debug, Default, Serialize, Deserialize)]
struct KeybindingDocument {
    #[serde(default)]
    common: Overrides,
    #[serde(default)]
    darwin: Overrides,
    #[serde(default)]
    linux: Overrides,
    #[serde(default)]
    win32: Overrides,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindingFileDiagnostic {
    severity: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    section: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindingFileSnapshot {
    path: String,
    platform: String,
    exists: bool,
    overrides: Overrides,
    common_overrides: Overrides,
    platform_overrides: BTreeMap<String, Overrides>,
    diagnostics: Vec<KeybindingFileDiagnostic>,
}

fn keybindings_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
    Ok(home.join(".agentum").join("keybindings.json"))
}

fn platform_section<'a>(document: &'a KeybindingDocument, platform: &str) -> &'a Overrides {
    match platform {
        "darwin" => &document.darwin,
        "win32" => &document.win32,
        _ => &document.linux,
    }
}

// Effective bindings = common overlaid with the current platform's (platform wins).
fn effective_overrides(document: &KeybindingDocument, platform: &str) -> Overrides {
    let mut merged = document.common.clone();
    for (action, bindings) in platform_section(document, platform) {
        merged.insert(action.clone(), bindings.clone());
    }
    merged
}

fn build_snapshot(
    path: &Path,
    document: &KeybindingDocument,
    exists: bool,
    diagnostics: Vec<KeybindingFileDiagnostic>,
) -> KeybindingFileSnapshot {
    let platform = platform_label();
    let mut platform_overrides = BTreeMap::new();
    platform_overrides.insert("darwin".to_string(), document.darwin.clone());
    platform_overrides.insert("linux".to_string(), document.linux.clone());
    platform_overrides.insert("win32".to_string(), document.win32.clone());

    KeybindingFileSnapshot {
        path: path.display().to_string(),
        overrides: effective_overrides(document, &platform),
        platform,
        exists,
        common_overrides: document.common.clone(),
        platform_overrides,
        diagnostics,
    }
}

// Returns (document, exists, diagnostics). A parse failure surfaces as an error
// diagnostic with a default document, mirroring the renderer's tolerant read.
fn read_document(path: &PathBuf) -> (KeybindingDocument, bool, Vec<KeybindingFileDiagnostic>) {
    if !path.exists() {
        return (KeybindingDocument::default(), false, Vec::new());
    }
    match std::fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<KeybindingDocument>(&raw) {
            Ok(document) => (document, true, Vec::new()),
            Err(error) => (
                KeybindingDocument::default(),
                true,
                vec![KeybindingFileDiagnostic {
                    severity: "error".to_string(),
                    message: format!("Failed to parse keybindings file: {error}"),
                    action_id: None,
                    section: None,
                }],
            ),
        },
        Err(error) => (
            KeybindingDocument::default(),
            true,
            vec![KeybindingFileDiagnostic {
                severity: "error".to_string(),
                message: format!("Failed to read keybindings file: {error}"),
                action_id: None,
                section: None,
            }],
        ),
    }
}

fn write_document(path: &PathBuf, document: &KeybindingDocument) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let serialized = serde_json::to_string_pretty(document).map_err(|error| error.to_string())?;
    std::fs::write(path, format!("{serialized}\n")).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn keybindings_get() -> Result<KeybindingFileSnapshot, String> {
    let path = keybindings_path()?;
    let (document, exists, diagnostics) = read_document(&path);
    Ok(build_snapshot(&path, &document, exists, diagnostics))
}

#[tauri::command]
pub fn keybindings_reload() -> Result<KeybindingFileSnapshot, String> {
    keybindings_get()
}

#[tauri::command]
pub fn keybindings_ensure_file() -> Result<KeybindingFileSnapshot, String> {
    let path = keybindings_path()?;
    if !path.exists() {
        write_document(&path, &KeybindingDocument::default())?;
    }
    let (document, exists, diagnostics) = read_document(&path);
    Ok(build_snapshot(&path, &document, exists, diagnostics))
}

#[tauri::command]
pub fn keybindings_set_action(
    action_id: String,
    bindings: Option<Vec<String>>,
) -> Result<KeybindingFileSnapshot, String> {
    let path = keybindings_path()?;
    let (mut document, _, _) = read_document(&path);
    // setAction carries no platform, so it edits the shared `common` section.
    match bindings {
        Some(values) => {
            document.common.insert(action_id, values);
        }
        None => {
            document.common.remove(&action_id);
        }
    }
    write_document(&path, &document)?;
    let diagnostics = Vec::new();
    Ok(build_snapshot(&path, &document, true, diagnostics))
}

#[tauri::command]
pub async fn keybindings_open_file() -> Result<KeybindingFileSnapshot, String> {
    let path = keybindings_path()?;
    if !path.exists() {
        write_document(&path, &KeybindingDocument::default())?;
    }
    open_in_os(&path, false).await?;
    let (document, exists, diagnostics) = read_document(&path);
    Ok(build_snapshot(&path, &document, exists, diagnostics))
}

#[tauri::command]
pub async fn keybindings_reveal_file() -> Result<KeybindingFileSnapshot, String> {
    let path = keybindings_path()?;
    if !path.exists() {
        write_document(&path, &KeybindingDocument::default())?;
    }
    open_in_os(&path, true).await?;
    let (document, exists, diagnostics) = read_document(&path);
    Ok(build_snapshot(&path, &document, exists, diagnostics))
}

// reveal=true shows the file in the OS file manager; reveal=false opens it.
async fn open_in_os(path: &Path, reveal: bool) -> Result<(), String> {
    let target = path.display().to_string();
    let mut command = if cfg!(target_os = "macos") {
        let mut command = tokio::process::Command::new("open");
        if reveal {
            command.arg("-R");
        }
        command.arg(&target);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = tokio::process::Command::new("explorer");
        if reveal {
            command.arg(format!("/select,{target}"));
        } else {
            command.arg(&target);
        }
        command
    } else {
        // Linux has no portable "reveal"; open the file or its parent directory.
        let open_target = if reveal {
            path.parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or(target)
        } else {
            target
        };
        let mut command = tokio::process::Command::new("xdg-open");
        command.arg(open_target);
        command
    };

    command
        .status()
        .await
        .map_err(|error| error.to_string())
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err("failed to open keybindings file".to_string())
            }
        })
}
