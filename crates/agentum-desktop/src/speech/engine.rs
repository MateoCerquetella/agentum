//! The on-device recognizer — port of orca's `stt-worker.ts`.
//!
//! sherpa-rs's *safe* API only exposes offline recognizers, so:
//!   - Offline models (parakeet transducer, whisper) use the safe wrappers and
//!     decode all buffered audio in one shot on stop.
//!   - Streaming models (streaming zipformer transducer, streaming paraformer)
//!     use the re-exported `sherpa_rs_sys` FFI online recognizer to emit live
//!     partials plus endpoint-triggered finals.
//!
//! All audio crossing into the recognizer is already resampled to the model's
//! native rate (16 kHz) by the caller — sherpa aborts the process if one
//! recognizer sees mixed input rates.

use std::ffi::CStr;
use std::fs;
use std::path::Path;
use std::ptr;

use anyhow::{anyhow, bail, Result};
use sherpa_rs::sherpa_rs_sys as sys;
use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};
use sherpa_rs::whisper::{WhisperConfig, WhisperRecognizer};

use super::catalog::{SpeechModelManifest, SpeechModelType};
use super::ffi_util::CStringHolder;

/// What a single feed/finish produced. The service forwards these to the
/// renderer as `speech-partial-transcript` / `speech-final-transcript`.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    Partial(String),
    Final(String),
}

pub enum Engine {
    OfflineTransducer {
        recognizer: TransducerRecognizer,
        buffer: Vec<f32>,
    },
    OfflineWhisper {
        recognizer: WhisperRecognizer,
        buffer: Vec<f32>,
    },
    Online(OnlineEngine),
}

// The engine is owned by the service behind a Mutex and only ever touched by one
// task at a time; the raw sherpa pointers inside OnlineEngine are not otherwise
// shared.
unsafe impl Send for Engine {}

impl Engine {
    /// The native rate every recognizer here expects (sherpa "models provided by
    /// us" are 16 kHz). The caller resamples to this before feeding.
    pub const NATIVE_SAMPLE_RATE: u32 = 16_000;

    pub fn load(model_dir: &Path, manifest: &SpeechModelManifest) -> Result<Self> {
        let tokens = resolve_tokens(model_dir, manifest.files)?;
        if manifest.streaming {
            return Ok(Engine::Online(OnlineEngine::load(
                model_dir, manifest, &tokens,
            )?));
        }
        match manifest.model_type {
            SpeechModelType::Whisper => {
                let encoder = resolve_file(model_dir, manifest.files, "encoder")?;
                let decoder = resolve_file(model_dir, manifest.files, "decoder")?;
                let recognizer = WhisperRecognizer::new(WhisperConfig {
                    encoder,
                    decoder,
                    tokens,
                    // Empty language lets whisper auto-detect across its 90+ langs.
                    language: String::new(),
                    num_threads: Some(2),
                    ..Default::default()
                })
                .map_err(|e| anyhow!("whisper init failed: {e}"))?;
                Ok(Engine::OfflineWhisper {
                    recognizer,
                    buffer: Vec::new(),
                })
            }
            // Offline transducer (parakeet).
            SpeechModelType::Transducer => {
                let encoder = resolve_file(model_dir, manifest.files, "encoder")?;
                let decoder = resolve_file(model_dir, manifest.files, "decoder")?;
                let joiner = resolve_file(model_dir, manifest.files, "joiner")?;
                let recognizer = TransducerRecognizer::new(TransducerConfig {
                    encoder,
                    decoder,
                    joiner,
                    tokens,
                    num_threads: 2,
                    sample_rate: Self::NATIVE_SAMPLE_RATE as i32,
                    feature_dim: 80,
                    decoding_method: "greedy_search".into(),
                    // EMPTY model_type — let sherpa auto-detect the family from the
                    // ONNX metadata. The wrapper's default is "transducer", which
                    // forces the *conventional* decoder and makes NeMo models like
                    // Parakeet TDT abort ("'vocab_size' does not exist in the
                    // metadata"); an empty string routes EncDecRNNTBPEModel encoders
                    // to the NeMo impl. Matches orca's config exactly.
                    model_type: String::new(),
                    ..Default::default()
                })
                .map_err(|e| anyhow!("transducer init failed: {e}"))?;
                Ok(Engine::OfflineTransducer {
                    recognizer,
                    buffer: Vec::new(),
                })
            }
            // A non-streaming paraformer isn't in the catalog; reject clearly.
            SpeechModelType::Paraformer => {
                bail!("offline paraformer is not supported")
            }
        }
    }

    /// Feed one (already 16 kHz) chunk. Offline recognizers buffer; streaming
    /// recognizers decode immediately and may return partial/final events.
    pub fn feed(&mut self, samples: &[f32]) -> Vec<EngineEvent> {
        match self {
            Engine::OfflineTransducer { buffer, .. } | Engine::OfflineWhisper { buffer, .. } => {
                buffer.extend_from_slice(samples);
                Vec::new()
            }
            Engine::Online(engine) => engine.feed(samples),
        }
    }

    /// Clear per-dictation state so a warm engine can be reused for a new
    /// session without reloading the model (offline: drop buffered audio;
    /// streaming: reset the recognizer stream).
    pub fn reset(&mut self) {
        match self {
            Engine::OfflineTransducer { buffer, .. } | Engine::OfflineWhisper { buffer, .. } => {
                buffer.clear();
            }
            Engine::Online(engine) => engine.reset(),
        }
    }

    /// Dictation stopped: flush whatever is left and return the final text (if
    /// any). Offline decodes the whole buffer here.
    pub fn finish(&mut self) -> Vec<EngineEvent> {
        match self {
            Engine::OfflineTransducer { recognizer, buffer } => {
                if buffer.is_empty() {
                    return Vec::new();
                }
                let text = recognizer
                    .transcribe(Self::NATIVE_SAMPLE_RATE, buffer)
                    .trim()
                    .to_string();
                buffer.clear();
                final_event(text)
            }
            Engine::OfflineWhisper { recognizer, buffer } => {
                if buffer.is_empty() {
                    return Vec::new();
                }
                let text = recognizer
                    .transcribe(Self::NATIVE_SAMPLE_RATE, buffer)
                    .text
                    .trim()
                    .to_string();
                buffer.clear();
                final_event(text)
            }
            Engine::Online(engine) => engine.finish(),
        }
    }
}

fn final_event(text: String) -> Vec<EngineEvent> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![EngineEvent::Final(text)]
    }
}

/// Streaming recognizer over the raw sherpa-onnx C API. Holds the recognizer and
/// its current stream; the stream is reset after each endpoint so one dictation
/// can span multiple utterances.
pub struct OnlineEngine {
    recognizer: *const sys::SherpaOnnxOnlineRecognizer,
    stream: *const sys::SherpaOnnxOnlineStream,
    // Keep the CString-backed config strings alive for the recognizer's lifetime;
    // sherpa stores the pointers, not copies, for some fields.
    _holder: CStringHolder,
}

impl OnlineEngine {
    fn load(model_dir: &Path, manifest: &SpeechModelManifest, tokens: &str) -> Result<Self> {
        let mut holder = CStringHolder::default();
        let tokens_ptr = holder.push(tokens);
        let provider_ptr = holder.push("cpu");
        let empty = holder.push("");

        let model_config = unsafe {
            let mut mc: sys::SherpaOnnxOnlineModelConfig = std::mem::zeroed();
            mc.tokens = tokens_ptr;
            mc.num_threads = 1;
            mc.provider = provider_ptr;
            mc.debug = 0;
            mc.model_type = empty;
            mc.modeling_unit = empty;
            mc.bpe_vocab = empty;
            match manifest.model_type {
                SpeechModelType::Transducer => {
                    let encoder = resolve_file(model_dir, manifest.files, "encoder")?;
                    let decoder = resolve_file(model_dir, manifest.files, "decoder")?;
                    let joiner = resolve_file(model_dir, manifest.files, "joiner")?;
                    mc.transducer = sys::SherpaOnnxOnlineTransducerModelConfig {
                        encoder: holder.push(&encoder),
                        decoder: holder.push(&decoder),
                        joiner: holder.push(&joiner),
                    };
                }
                SpeechModelType::Paraformer => {
                    let encoder = resolve_file(model_dir, manifest.files, "encoder")?;
                    let decoder = resolve_file(model_dir, manifest.files, "decoder")?;
                    mc.paraformer = sys::SherpaOnnxOnlineParaformerModelConfig {
                        encoder: holder.push(&encoder),
                        decoder: holder.push(&decoder),
                    };
                }
                SpeechModelType::Whisper => bail!("whisper has no streaming recognizer"),
            }
            mc
        };

        let decoding_method = holder.push("greedy_search");
        let config = unsafe {
            let mut cfg: sys::SherpaOnnxOnlineRecognizerConfig = std::mem::zeroed();
            cfg.feat_config = sys::SherpaOnnxFeatureConfig {
                sample_rate: Engine::NATIVE_SAMPLE_RATE as i32,
                feature_dim: 80,
            };
            cfg.model_config = model_config;
            cfg.decoding_method = decoding_method;
            cfg.hotwords_file = empty;
            cfg.rule_fsts = empty;
            cfg.rule_fars = empty;
            // Endpointing thresholds mirror orca's worker so a pause flushes a
            // final and the stream resets for the next utterance.
            cfg.enable_endpoint = 1;
            cfg.rule1_min_trailing_silence = 2.4;
            cfg.rule2_min_trailing_silence = 1.2;
            cfg.rule3_min_utterance_length = 20.0;
            cfg
        };

        let recognizer = unsafe { sys::SherpaOnnxCreateOnlineRecognizer(&config) };
        if recognizer.is_null() {
            bail!("SherpaOnnxCreateOnlineRecognizer failed");
        }
        let stream = unsafe { sys::SherpaOnnxCreateOnlineStream(recognizer) };
        if stream.is_null() {
            unsafe { sys::SherpaOnnxDestroyOnlineRecognizer(recognizer) };
            bail!("SherpaOnnxCreateOnlineStream failed");
        }

        Ok(Self {
            recognizer,
            stream,
            _holder: holder,
        })
    }

    fn feed(&mut self, samples: &[f32]) -> Vec<EngineEvent> {
        if samples.is_empty() {
            return Vec::new();
        }
        let mut events = Vec::new();
        unsafe {
            sys::SherpaOnnxOnlineStreamAcceptWaveform(
                self.stream,
                Engine::NATIVE_SAMPLE_RATE as i32,
                samples.as_ptr(),
                samples.len() as i32,
            );
            while sys::SherpaOnnxIsOnlineStreamReady(self.recognizer, self.stream) != 0 {
                sys::SherpaOnnxDecodeOnlineStream(self.recognizer, self.stream);
            }
            let text = self.current_text();
            if !text.is_empty() {
                events.push(EngineEvent::Partial(text.clone()));
            }
            if sys::SherpaOnnxOnlineStreamIsEndpoint(self.recognizer, self.stream) != 0 {
                if !text.is_empty() {
                    events.push(EngineEvent::Final(text));
                }
                sys::SherpaOnnxOnlineStreamReset(self.recognizer, self.stream);
            }
        }
        events
    }

    fn reset(&mut self) {
        unsafe {
            sys::SherpaOnnxOnlineStreamReset(self.recognizer, self.stream);
        }
    }

    fn finish(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        unsafe {
            sys::SherpaOnnxOnlineStreamInputFinished(self.stream);
            while sys::SherpaOnnxIsOnlineStreamReady(self.recognizer, self.stream) != 0 {
                sys::SherpaOnnxDecodeOnlineStream(self.recognizer, self.stream);
            }
            let text = self.current_text();
            if !text.is_empty() {
                events.push(EngineEvent::Final(text));
            }
            sys::SherpaOnnxOnlineStreamReset(self.recognizer, self.stream);
        }
        events
    }

    /// Read the recognizer's current hypothesis text from its JSON result.
    unsafe fn current_text(&self) -> String {
        let json_ptr = sys::SherpaOnnxGetOnlineStreamResultAsJson(self.recognizer, self.stream);
        if json_ptr.is_null() {
            return String::new();
        }
        let json = CStr::from_ptr(json_ptr).to_string_lossy().into_owned();
        sys::SherpaOnnxDestroyOnlineStreamResultJson(json_ptr);
        serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(str::to_string))
            .map(|t| t.trim().to_string())
            .unwrap_or_default()
    }
}

impl Drop for OnlineEngine {
    fn drop(&mut self) {
        unsafe {
            if !self.stream.is_null() {
                sys::SherpaOnnxDestroyOnlineStream(self.stream);
                self.stream = ptr::null();
            }
            if !self.recognizer.is_null() {
                sys::SherpaOnnxDestroyOnlineRecognizer(self.recognizer);
                self.recognizer = ptr::null();
            }
        }
    }
}

/// Find a model file whose name contains `role` and ends with `.onnx`, mirroring
/// orca's `resolveFile` (models name their ONNX files inconsistently, e.g.
/// `encoder.int8.onnx` vs `tiny-encoder.onnx` vs `encoder-epoch-99-avg-1.onnx`).
fn resolve_file(model_dir: &Path, files: &[&str], role: &str) -> Result<String> {
    let name = files
        .iter()
        .find(|f| f.contains(role) && f.ends_with(".onnx"))
        .ok_or_else(|| anyhow!("No *{role}*.onnx found in model files"))?;
    let path = model_dir.join(name);
    ensure_exists(&path)?;
    Ok(path.to_string_lossy().into_owned())
}

fn resolve_tokens(model_dir: &Path, files: &[&str]) -> Result<String> {
    let name = files
        .iter()
        .find(|f| f.ends_with("tokens.txt"))
        .ok_or_else(|| anyhow!("No *tokens.txt found in model files"))?;
    let path = model_dir.join(name);
    ensure_exists(&path)?;
    Ok(path.to_string_lossy().into_owned())
}

fn ensure_exists(path: &Path) -> Result<()> {
    if !fs::metadata(path).map(|m| m.is_file()).unwrap_or(false) {
        bail!("model file missing: {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech_engine::catalog::get_catalog_model;

    // End-to-end smoke test for the hand-written streaming FFI: downloads the
    // smallest streaming model, builds the online recognizer, and feeds audio.
    // A botched FFI struct would abort() the process here. Ignored by default
    // (network + ~74 MB); run with:
    //   cargo test -p agentum-desktop --lib streaming_engine_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn streaming_engine_smoke() {
        let manifest = get_catalog_model("zipformer-streaming-zh-14m").unwrap();
        let dir = std::env::temp_dir()
            .join("agentum-speech-test")
            .join(manifest.id);
        download_and_extract(manifest, &dir);

        let mut engine = Engine::load(&dir, manifest).expect("load streaming engine");
        assert!(matches!(engine, Engine::Online(_)));

        // 1s of silence at 16 kHz — must not crash; produces no transcript.
        let silence = vec![0.0f32; 16_000];
        let _ = engine.feed(&silence);
        let finals = engine.finish();
        eprintln!("streaming finish events: {finals:?}");
    }

    // Loads the Parakeet TDT v3 model from the real app-data dir (must already be
    // downloaded). Proves the offline NeMo-transducer path doesn't abort with the
    // model_type fix. Run with:
    //   cargo test -p agentum-desktop --lib parakeet_offline_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn parakeet_offline_smoke() {
        let manifest = get_catalog_model("parakeet-tdt-0.6b-v3-int8").unwrap();
        let dir = dirs::data_dir()
            .unwrap()
            .join("dev.agentum.app")
            .join("speech-models")
            .join(manifest.id);
        assert!(
            dir.join("tokens.txt").exists(),
            "download Parakeet v3 first: {}",
            dir.display()
        );
        let mut engine = Engine::load(&dir, manifest).expect("load parakeet engine");
        assert!(matches!(engine, Engine::OfflineTransducer { .. }));
        let silence = vec![0.0f32; 16_000];
        let _ = engine.feed(&silence);
        let finals = engine.finish();
        eprintln!("parakeet finish events: {finals:?}");
    }

    // Transcribes the real English sample shipped with Parakeet and asserts
    // actual words come out — the definitive end-to-end proof of the offline path.
    //   cargo test -p agentum-desktop --lib parakeet_transcribes_real_audio -- --ignored --nocapture
    #[test]
    #[ignore]
    fn parakeet_transcribes_real_audio() {
        let manifest = get_catalog_model("parakeet-tdt-0.6b-v3-int8").unwrap();
        let dir = dirs::data_dir()
            .unwrap()
            .join("dev.agentum.app")
            .join("speech-models")
            .join(manifest.id);
        let wav = dir.join("test_wavs").join("en.wav");
        assert!(wav.exists(), "missing {}", wav.display());

        let (samples, rate) = read_wav_mono_i16(&wav);
        let samples_16k = linear_resample(&samples, rate, Engine::NATIVE_SAMPLE_RATE);

        let mut engine = Engine::load(&dir, manifest).expect("load parakeet engine");
        let _ = engine.feed(&samples_16k);
        let finals = engine.finish();
        eprintln!("parakeet transcript: {finals:?}");
        let text = finals
            .iter()
            .find_map(|e| match e {
                EngineEvent::Final(t) => Some(t.clone()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(!text.trim().is_empty(), "expected non-empty transcript");
    }

    // Warm reuse: one loaded engine, transcribed twice with a reset between —
    // proves a warm engine produces correct output on the second dictation
    // without reloading the model.
    //   cargo test -p agentum-desktop --lib parakeet_warm_reuse -- --ignored --nocapture
    #[test]
    #[ignore]
    fn parakeet_warm_reuse() {
        let manifest = get_catalog_model("parakeet-tdt-0.6b-v3-int8").unwrap();
        let dir = dirs::data_dir()
            .unwrap()
            .join("dev.agentum.app")
            .join("speech-models")
            .join(manifest.id);
        let (samples, rate) = read_wav_mono_i16(&dir.join("test_wavs").join("en.wav"));
        let audio = linear_resample(&samples, rate, Engine::NATIVE_SAMPLE_RATE);

        let mut engine = Engine::load(&dir, manifest).expect("load");
        for run in 0..2 {
            engine.reset();
            let _ = engine.feed(&audio);
            let finals = engine.finish();
            let text = finals
                .iter()
                .find_map(|e| match e {
                    EngineEvent::Final(t) => Some(t.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            eprintln!("run {run}: {text:?}");
            assert!(!text.trim().is_empty(), "run {run} empty");
        }
    }

    // Minimal RIFF/WAVE reader for 16-bit PCM mono (enough for the test_wavs).
    fn read_wav_mono_i16(path: &Path) -> (Vec<f32>, u32) {
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let mut rate = 16_000u32;
        let mut samples = Vec::new();
        let mut i = 12usize;
        while i + 8 <= bytes.len() {
            let id = &bytes[i..i + 4];
            let size = u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]])
                as usize;
            let body = i + 8;
            if id == b"fmt " {
                rate = u32::from_le_bytes([
                    bytes[body + 4],
                    bytes[body + 5],
                    bytes[body + 6],
                    bytes[body + 7],
                ]);
            } else if id == b"data" {
                let end = (body + size).min(bytes.len());
                let mut j = body;
                while j + 1 < end {
                    let s = i16::from_le_bytes([bytes[j], bytes[j + 1]]);
                    samples.push(s as f32 / 32768.0);
                    j += 2;
                }
            }
            i = body + size + (size & 1); // chunks are word-aligned
        }
        (samples, rate)
    }

    fn linear_resample(samples: &[f32], input: u32, output: u32) -> Vec<f32> {
        if input == output || samples.is_empty() {
            return samples.to_vec();
        }
        let out_len =
            ((samples.len() as f64 * output as f64 / input as f64).round() as usize).max(1);
        let ratio = input as f64 / output as f64;
        let last = samples.len() - 1;
        (0..out_len)
            .map(|k| {
                let src = k as f64 * ratio;
                let l = src.floor() as usize;
                let r = (l + 1).min(last);
                let w = (src - l as f64) as f32;
                samples[l] * (1.0 - w) + samples[r] * w
            })
            .collect()
    }

    fn download_and_extract(manifest: &SpeechModelManifest, dir: &Path) {
        if dir.join("tokens.txt").exists() {
            return;
        }
        fs::create_dir_all(dir).unwrap();
        let archive = dir.with_extension("tar.bz2");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let bytes = rt.block_on(async {
            reqwest::get(manifest.download_url)
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
        });
        fs::write(&archive, &bytes).unwrap();
        let decoder = bzip2::read::BzDecoder::new(fs::File::open(&archive).unwrap());
        let mut tar = tar::Archive::new(decoder);
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            let stripped: std::path::PathBuf = path.components().skip(1).collect();
            if stripped.as_os_str().is_empty() {
                continue;
            }
            let out = dir.join(&stripped);
            if entry.header().entry_type().is_dir() {
                fs::create_dir_all(&out).ok();
                continue;
            }
            if let Some(p) = out.parent() {
                fs::create_dir_all(p).ok();
            }
            entry.unpack(&out).unwrap();
        }
    }
}
