//! On-device speech model lifecycle for Agentum.
//!
//! Owns the models directory and a small state machine per model
//! (not-downloaded → downloading → extracting → ready | error). Downloads stream
//! to disk with progress, are SHA-256 verified against the catalog (these
//! archives feed native ONNX parsers, so a filename check is not enough), then
//! extracted from `tar.bz2`. Progress is surfaced to the renderer via the
//! `speech-download-progress` event, which the Voice settings pane uses as a cue
//! to re-fetch `speech_get_model_states`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use bzip2::read::BzDecoder;
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};

use super::catalog::{get_catalog_model, SpeechModelManifest, SPEECH_MODEL_CATALOG};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpeechModelStatus {
    NotDownloaded,
    Downloading,
    Extracting,
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechModelState {
    pub id: String,
    pub status: SpeechModelStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DownloadProgressPayload {
    model_id: String,
    progress: f32,
}

pub struct ModelManager {
    models_dir: PathBuf,
    // Cached transient states (downloading/extracting/error). Ready/not-downloaded
    // are derived from the filesystem so a restart reflects reality.
    states: Mutex<HashMap<String, SpeechModelState>>,
    // Per-download cancellation flags; the streaming loop polls these.
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    active: Mutex<HashSet<String>>,
}

impl ModelManager {
    pub fn new(models_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&models_dir)
            .with_context(|| format!("create speech models dir {}", models_dir.display()))?;
        Ok(Self {
            models_dir,
            states: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            active: Mutex::new(HashSet::new()),
        })
    }

    pub fn get_model_states(&self) -> Vec<SpeechModelState> {
        SPEECH_MODEL_CATALOG
            .iter()
            .map(|m| self.get_model_state(m.id))
            .collect()
    }

    pub fn get_model_state(&self, model_id: &str) -> SpeechModelState {
        // An in-flight transition wins over the on-disk view.
        if let Some(cached) = self.states.lock().get(model_id) {
            if matches!(
                cached.status,
                SpeechModelStatus::Downloading | SpeechModelStatus::Extracting
            ) {
                return cached.clone();
            }
        }

        let Some(manifest) = get_catalog_model(model_id) else {
            return SpeechModelState {
                id: model_id.to_string(),
                status: SpeechModelStatus::Error,
                progress: None,
                error: Some("Unknown model".into()),
            };
        };

        let model_dir = self.model_dir(model_id);
        if model_dir.exists() && validate_model_files(manifest, &model_dir) {
            return SpeechModelState {
                id: model_id.to_string(),
                status: SpeechModelStatus::Ready,
                progress: None,
                error: None,
            };
        }

        // Surface a sticky error (e.g. failed verify) instead of masking it as
        // "not downloaded", but only when no files landed.
        if let Some(cached) = self.states.lock().get(model_id) {
            if cached.status == SpeechModelStatus::Error {
                return cached.clone();
            }
        }

        SpeechModelState {
            id: model_id.to_string(),
            status: SpeechModelStatus::NotDownloaded,
            progress: None,
            error: None,
        }
    }

    pub fn model_dir(&self, model_id: &str) -> PathBuf {
        self.models_dir.join(model_id)
    }

    /// Resolve and validate a model directory for *use* by the engine. Errors if
    /// the model is unknown or its files are missing.
    pub fn ready_model_dir(&self, model_id: &str) -> Result<PathBuf> {
        let manifest =
            get_catalog_model(model_id).ok_or_else(|| anyhow!("Unknown model: {model_id}"))?;
        let dir = self.model_dir(model_id);
        if !dir.exists() || !validate_model_files(manifest, &dir) {
            bail!("Model not ready: {model_id}");
        }
        Ok(dir)
    }

    pub async fn download_model(&self, app: &AppHandle, model_id: &str) -> Result<()> {
        if self.active.lock().contains(model_id) {
            return Ok(());
        }
        let manifest =
            get_catalog_model(model_id).ok_or_else(|| anyhow!("Unknown model: {model_id}"))?;

        let model_dir = self.model_dir(model_id);
        if model_dir.exists() && validate_model_files(manifest, &model_dir) {
            self.set_state(app, model_id, SpeechModelStatus::Ready, None, None);
            return Ok(());
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.active.lock().insert(model_id.to_string());
        self.cancels
            .lock()
            .insert(model_id.to_string(), cancel.clone());
        self.set_state(
            app,
            model_id,
            SpeechModelStatus::Downloading,
            Some(0.0),
            None,
        );

        let archive_path = self.models_dir.join(format!("{model_id}.tar.bz2"));
        let result = self
            .run_download(app, manifest, &archive_path, &model_dir, &cancel)
            .await;

        // Always drop the active/cancel registration and the temp archive.
        self.active.lock().remove(model_id);
        self.cancels.lock().remove(model_id);
        let _ = fs::remove_file(&archive_path);

        if cancel.load(Ordering::SeqCst) {
            // Cancellation is quiet: reset to not-downloaded and clean partials.
            let _ = fs::remove_dir_all(&model_dir);
            self.clear_state(model_id);
            let _ = app.emit(
                "speech-download-progress",
                DownloadProgressPayload {
                    model_id: model_id.to_string(),
                    progress: -1.0,
                },
            );
            return Ok(());
        }

        match result {
            Ok(()) => {
                self.set_state(app, model_id, SpeechModelStatus::Ready, None, None);
                Ok(())
            }
            Err(err) => {
                let _ = fs::remove_dir_all(&model_dir);
                self.set_state(
                    app,
                    model_id,
                    SpeechModelStatus::Error,
                    None,
                    Some(err.to_string()),
                );
                Err(err)
            }
        }
    }

    async fn run_download(
        &self,
        app: &AppHandle,
        manifest: &SpeechModelManifest,
        archive_path: &Path,
        model_dir: &Path,
        cancel: &Arc<AtomicBool>,
    ) -> Result<()> {
        self.download_file(app, manifest, archive_path, cancel)
            .await?;
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }

        verify_sha256(archive_path, manifest.archive_sha256)?;
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }

        self.set_state(
            app,
            manifest.id,
            SpeechModelStatus::Extracting,
            Some(0.95),
            None,
        );
        extract_tar_bz2_stripped(archive_path, model_dir)?;
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Some archives nest files one directory deeper than --strip-components=1
        // handles; pull them up if the expected files aren't at the top level.
        if !validate_model_files(manifest, model_dir) {
            flatten_nested_dir(model_dir, manifest)?;
        }
        if !validate_model_files(manifest, model_dir) {
            bail!("Model files missing after extraction");
        }
        Ok(())
    }

    async fn download_file(
        &self,
        app: &AppHandle,
        manifest: &SpeechModelManifest,
        dest: &Path,
        cancel: &Arc<AtomicBool>,
    ) -> Result<()> {
        let client = reqwest::Client::builder()
            .build()
            .context("build http client")?;
        let resp = client
            .get(manifest.download_url)
            .send()
            .await
            .context("start model download")?;
        if !resp.status().is_success() {
            bail!("HTTP {}", resp.status().as_u16());
        }

        let total = resp
            .content_length()
            .filter(|n| *n > 0)
            .unwrap_or(manifest.size_bytes);
        let mut downloaded: u64 = 0;
        let mut file = fs::File::create(dest)
            .with_context(|| format!("create archive file {}", dest.display()))?;
        let mut stream = resp.bytes_stream();
        let mut last_emit = 0.0_f32;

        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
            let chunk = chunk.context("read download chunk")?;
            std::io::Write::write_all(&mut file, &chunk).context("write archive chunk")?;
            downloaded += chunk.len() as u64;
            // Cap at 0.9 — extraction owns the last slice of the progress bar.
            let progress = (downloaded as f32 / total as f32).min(0.9);
            if progress - last_emit >= 0.01 {
                last_emit = progress;
                self.set_state(
                    app,
                    manifest.id,
                    SpeechModelStatus::Downloading,
                    Some(progress),
                    None,
                );
            }
        }
        Ok(())
    }

    pub fn cancel_download(&self, model_id: &str) {
        if let Some(flag) = self.cancels.lock().get(model_id) {
            flag.store(true, Ordering::SeqCst);
        }
        // Reflect immediately; the download task finishes cleanup asynchronously.
        self.clear_state(model_id);
    }

    pub fn delete_model(&self, model_id: &str) -> Result<()> {
        if get_catalog_model(model_id).is_none() {
            bail!("Unknown model: {model_id}");
        }
        self.cancel_download(model_id);
        let model_dir = self.model_dir(model_id);
        if model_dir.exists() {
            fs::remove_dir_all(&model_dir)
                .with_context(|| format!("delete model dir {}", model_dir.display()))?;
        }
        self.clear_state(model_id);
        Ok(())
    }

    fn set_state(
        &self,
        app: &AppHandle,
        model_id: &str,
        status: SpeechModelStatus,
        progress: Option<f32>,
        error: Option<String>,
    ) {
        self.states.lock().insert(
            model_id.to_string(),
            SpeechModelState {
                id: model_id.to_string(),
                status,
                progress,
                error,
            },
        );
        // Notify on every transition (not just download ticks) so the UI updates
        // for extracting/ready/error too.
        let progress_value = progress.unwrap_or(if status == SpeechModelStatus::Extracting {
            0.95
        } else {
            -1.0
        });
        let _ = app.emit(
            "speech-download-progress",
            DownloadProgressPayload {
                model_id: model_id.to_string(),
                progress: progress_value,
            },
        );
    }

    fn clear_state(&self, model_id: &str) {
        self.states.lock().remove(model_id);
    }
}

fn validate_model_files(manifest: &SpeechModelManifest, model_dir: &Path) -> bool {
    manifest.files.iter().all(|f| model_dir.join(f).exists())
}

fn verify_sha256(archive_path: &Path, expected: &str) -> Result<()> {
    let mut file = fs::File::open(archive_path).context("open archive for hashing")?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).context("read archive for hashing")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hex_lower(&hasher.finalize());
    if actual != expected.to_lowercase() {
        bail!("Downloaded model archive failed integrity verification");
    }
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Extract a `.tar.bz2` into `model_dir`, stripping the leading path component
/// (equivalent to `tar --strip-components=1`).
fn extract_tar_bz2_stripped(archive_path: &Path, model_dir: &Path) -> Result<()> {
    fs::create_dir_all(model_dir)
        .with_context(|| format!("create model dir {}", model_dir.display()))?;
    let file = fs::File::open(archive_path).context("open archive for extraction")?;
    let decoder = BzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries().context("read tar entries")? {
        let mut entry = entry.context("read tar entry")?;
        let path = entry.path().context("tar entry path")?.into_owned();
        // Drop the first component (the archive's top-level dir).
        let stripped: PathBuf = path.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        // Guard against path traversal in archive entries.
        if stripped
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue;
        }
        let out_path = model_dir.join(&stripped);
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&out_path).ok();
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).ok();
        }
        entry
            .unpack(&out_path)
            .with_context(|| format!("unpack {}", out_path.display()))?;
    }
    Ok(())
}

/// Move model files up one level when an archive nested them inside a
/// subdirectory that `--strip-components=1` did not flatten.
fn flatten_nested_dir(model_dir: &Path, manifest: &SpeechModelManifest) -> Result<()> {
    for entry in fs::read_dir(model_dir).context("read model dir")? {
        let entry = entry.context("read model dir entry")?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let nested = entry.path();
        let nested_names: HashSet<String> = fs::read_dir(&nested)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        let has_expected = manifest.files.iter().any(|f| nested_names.contains(*f));
        if has_expected {
            for name in &nested_names {
                let _ = fs::rename(nested.join(name), model_dir.join(name));
            }
            let _ = fs::remove_dir_all(&nested);
            return Ok(());
        }
    }
    Ok(())
}
