//! On-device speech-to-text (Voice dictation).
//!
//! Ported from orca's Electron engine (Node + sherpa-onnx) to Tauri/Rust using
//! the `sherpa-rs` bindings. Layers:
//!   - [`catalog`]: the fixed list of downloadable sherpa-onnx ASR models.
//!   - [`model_manager`]: download → verify → extract → state machine.
//!   - [`engine`]: the recognizer (offline safe wrappers + streaming FFI).
//!   - [`service`]: dictation lifecycle + transcript event fan-out.
//!
//! [`SpeechState`] is held in Tauri's managed state and driven by the commands
//! in `commands/speech.rs`.

pub mod catalog;
mod engine;
mod ffi_util;
pub mod model_manager;
mod service;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

pub use model_manager::ModelManager;
pub use service::SttService;

pub struct SpeechState {
    pub models: Arc<ModelManager>,
    pub service: Arc<SttService>,
}

impl SpeechState {
    pub fn new(models_dir: PathBuf) -> Result<Self> {
        Ok(Self {
            models: Arc::new(ModelManager::new(models_dir)?),
            service: Arc::new(SttService::default()),
        })
    }
}
