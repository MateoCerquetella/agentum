//! Dictation lifecycle + event fan-out — port of orca's `stt-service.ts` (minus
//! the worker-thread plumbing, which Tauri's command threads replace).
//!
//! One dictation is active at a time, keyed by the renderer's `sessionId`
//! ("owner"). The renderer captures mic audio in the webview and feeds Float32
//! chunks here; we resample to 16 kHz, run the engine, and emit transcript
//! events back. `stop` always emits `speech-stopped` for the requested session —
//! even if it was never the active one — because the renderer blocks on that
//! event to sequence runs.

use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use super::catalog::get_catalog_model;
use super::engine::{Engine, EngineEvent};
use super::model_manager::ModelManager;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TranscriptPayload {
    text: String,
    session_id: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LifecyclePayload {
    session_id: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ErrorPayload {
    error: String,
    session_id: String,
}

/// The loaded recognizer is kept warm between dictations so pressing the
/// shortcut doesn't reload the model (Parakeet's encoder alone is ~650 MB —
/// reloading it every time was the multi-second "Starting…" stall). `session`
/// is set only while actively dictating.
struct Inner {
    model_id: String,
    engine: Engine,
    session: Option<String>,
}

#[derive(Default)]
pub struct SttService {
    inner: Mutex<Option<Inner>>,
}

impl SttService {
    /// Begin a dictation. Loads the model only if no warm engine for it exists;
    /// otherwise reuses it. This does heavy work (model load on cold start) and
    /// MUST be called off the UI thread (see the async command wrapper).
    pub fn start(&self, models: &ModelManager, model_id: &str, session_id: &str) -> Result<()> {
        let mut guard = self.inner.lock();

        if let Some(inner) = guard.as_ref() {
            if let Some(active) = &inner.session {
                // Re-issuing start for the in-flight session is a no-op; a
                // different session means a second dictation is barging in.
                return if active == session_id {
                    Ok(())
                } else {
                    bail!("dictation_already_active")
                };
            }
            // Warm engine, idle: reuse it if the model matches.
            if inner.model_id == model_id {
                let inner = guard.as_mut().unwrap();
                inner.engine.reset();
                inner.session = Some(session_id.to_string());
                return Ok(());
            }
        }

        // Cold start (no engine, or a different model selected): load fresh. Drop
        // the old engine first so we don't hold two big models in memory.
        *guard = None;
        let dir = models.ready_model_dir(model_id)?;
        let manifest =
            get_catalog_model(model_id).ok_or_else(|| anyhow!("Unknown model: {model_id}"))?;
        let engine = Engine::load(&dir, manifest)?;
        *guard = Some(Inner {
            model_id: model_id.to_string(),
            engine,
            session: Some(session_id.to_string()),
        });
        Ok(())
    }

    pub fn feed(&self, app: &AppHandle, samples: &[f32], sample_rate: u32, session_id: &str) {
        let mut guard = self.inner.lock();
        let Some(inner) = guard.as_mut() else {
            return;
        };
        if inner.session.as_deref() != Some(session_id) {
            return;
        }
        let resampled = resample_to_rate(samples, sample_rate, Engine::NATIVE_SAMPLE_RATE);
        let events = inner.engine.feed(&resampled);
        drop(guard);
        emit_events(app, session_id, events);
    }

    /// Stop dictation. Flushes the final transcript (offline decodes the whole
    /// buffer here — also heavy, so call off the UI thread) but keeps the engine
    /// warm for the next dictation.
    pub fn stop(&self, app: &AppHandle, session_id: &str) {
        let mut finals = Vec::new();
        {
            let mut guard = self.inner.lock();
            if let Some(inner) = guard.as_mut() {
                if inner.session.as_deref() == Some(session_id) {
                    finals = inner.engine.finish();
                    inner.session = None; // idle, but stays warm
                }
            }
        }
        emit_events(app, session_id, finals);
        // Always signal stopped: the renderer's waitForStoppedSession() blocks on
        // it to avoid mistaking an old final for the next run.
        let _ = app.emit(
            "speech-stopped",
            LifecyclePayload {
                session_id: session_id.to_string(),
            },
        );
    }

    /// Drop the warm engine entirely (e.g. app shutdown / free memory).
    #[allow(dead_code)]
    pub fn abort(&self) {
        *self.inner.lock() = None;
    }

    #[allow(dead_code)]
    pub fn emit_error(&self, app: &AppHandle, session_id: &str, error: &str) {
        let _ = app.emit(
            "speech-error",
            ErrorPayload {
                error: error.to_string(),
                session_id: session_id.to_string(),
            },
        );
    }
}

fn emit_events(app: &AppHandle, session_id: &str, events: Vec<EngineEvent>) {
    for event in events {
        match event {
            EngineEvent::Partial(text) => {
                let _ = app.emit(
                    "speech-partial-transcript",
                    TranscriptPayload {
                        text,
                        session_id: session_id.to_string(),
                    },
                );
            }
            EngineEvent::Final(text) => {
                let _ = app.emit(
                    "speech-final-transcript",
                    TranscriptPayload {
                        text,
                        session_id: session_id.to_string(),
                    },
                );
            }
        }
    }
}

/// Linear resample — direct port of orca's `resampleToRate`. sherpa aborts the
/// process if one recognizer sees mixed input rates, so normalize before feeding.
fn resample_to_rate(samples: &[f32], input_rate: u32, output_rate: u32) -> Vec<f32> {
    if samples.is_empty() || input_rate == 0 || output_rate == 0 || input_rate == output_rate {
        return samples.to_vec();
    }
    let output_len = (((samples.len() as f64) * (output_rate as f64) / (input_rate as f64)).round()
        as usize)
        .max(1);
    let ratio = input_rate as f64 / output_rate as f64;
    let mut out = vec![0.0f32; output_len];
    let last = samples.len() - 1;
    for (i, slot) in out.iter_mut().enumerate() {
        let source = i as f64 * ratio;
        let left = source.floor() as usize;
        let right = (left + 1).min(last);
        let weight = (source - left as f64) as f32;
        *slot = samples[left] * (1.0 - weight) + samples[right] * weight;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_passthrough_when_rates_match() {
        let s = vec![0.1, 0.2, 0.3];
        assert_eq!(resample_to_rate(&s, 16000, 16000), s);
    }

    #[test]
    fn resample_downsamples_length() {
        let s = vec![0.0f32; 48000];
        let out = resample_to_rate(&s, 48000, 16000);
        assert_eq!(out.len(), 16000);
    }

    #[test]
    fn resample_empty_is_empty() {
        assert!(resample_to_rate(&[], 48000, 16000).is_empty());
    }
}
