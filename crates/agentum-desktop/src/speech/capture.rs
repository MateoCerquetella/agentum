//! Native microphone capture for Voice dictation.
//!
//! The renderer's `navigator.mediaDevices.getUserMedia` path (ported from orca's
//! Electron/Chromium build) does not exist in macOS WKWebView — `mediaDevices`
//! is `undefined`, so dictation failed with "undefined is not an object". Instead
//! we capture the default input device natively with `cpal` and feed the samples
//! straight into [`SttService::feed`], the same entry the old IPC `feedAudio`
//! used. No audio ever crosses the webview boundary.
//!
//! `cpal::Stream` is `!Send` on CoreAudio, so a dedicated thread builds, plays,
//! and owns the stream, then parks until a stop signal drops it. The audio
//! callback is `Send` and captures only `AppHandle` (Clone+Send+Sync),
//! `Arc<SttService>`, and the session id.

use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SizedSample;
use parking_lot::Mutex;
use tauri::AppHandle;

use crate::speech_engine::SttService;

/// Owns the lifecycle of the native capture thread. Held in `SpeechState`.
#[derive(Default)]
pub struct AudioCapture {
    // Sending on (or dropping) this unblocks the capture thread, which then drops
    // the cpal stream and stops the mic. `None` when not capturing.
    stop_tx: Mutex<Option<Sender<()>>>,
}

impl AudioCapture {
    /// Begin capturing the default input device and feeding `service`. Replaces
    /// any in-flight capture. Returns once the stream is playing (or errors).
    pub fn start(
        &self,
        app: AppHandle,
        service: Arc<SttService>,
        session_id: String,
    ) -> Result<(), String> {
        // Tear down any previous capture before starting a new one.
        self.stop();

        let (stop_tx, stop_rx) = channel::<()>();
        // The thread reports whether the stream came up so callers see real
        // device/permission errors instead of a silent no-op.
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();

        std::thread::spawn(move || {
            let host = cpal::default_host();
            let Some(device) = host.default_input_device() else {
                let _ = ready_tx.send(Err("no default input device".into()));
                return;
            };
            let config = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("input config: {e}")));
                    return;
                }
            };
            let rate = config.sample_rate().0;
            let channels = config.channels() as usize;
            let sample_format = config.sample_format();
            let stream_config: cpal::StreamConfig = config.into();

            // sherpa resamples to 16 kHz internally (via SttService::feed); here
            // we only downmix to mono f32 in the device's native format.
            let stream = match sample_format {
                cpal::SampleFormat::F32 => build_input_stream::<f32>(
                    &device,
                    &stream_config,
                    channels,
                    rate,
                    app,
                    service,
                    session_id,
                    |s| s,
                ),
                cpal::SampleFormat::I16 => build_input_stream::<i16>(
                    &device,
                    &stream_config,
                    channels,
                    rate,
                    app,
                    service,
                    session_id,
                    |s| s as f32 / 32768.0,
                ),
                cpal::SampleFormat::U16 => build_input_stream::<u16>(
                    &device,
                    &stream_config,
                    channels,
                    rate,
                    app,
                    service,
                    session_id,
                    |s| (s as f32 - 32768.0) / 32768.0,
                ),
                other => Err(format!("unsupported sample format: {other:?}")),
            };
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(format!("play stream: {e}")));
                return;
            }
            let _ = ready_tx.send(Ok(()));
            // Keep the stream (and mic) alive on this thread until stop signals.
            let _ = stop_rx.recv();
            drop(stream);
        });

        *self.stop_tx.lock() = Some(stop_tx);
        ready_rx
            .recv()
            .map_err(|_| "capture thread exited before reporting readiness".to_string())?
    }

    /// Stop capturing. Idempotent.
    pub fn stop(&self) {
        if let Some(tx) = self.stop_tx.lock().take() {
            let _ = tx.send(());
        }
    }
}

/// Build a cpal input stream for sample type `T`, downmix to mono f32 via
/// `to_f32`, and feed each buffer into the STT service.
#[allow(clippy::too_many_arguments)]
fn build_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    rate: u32,
    app: AppHandle,
    service: Arc<SttService>,
    session_id: String,
    to_f32: fn(T) -> f32,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Copy + Send + 'static,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mono: Vec<f32> = if channels <= 1 {
                    data.iter().copied().map(to_f32).collect()
                } else {
                    data.chunks(channels)
                        .map(|frame| {
                            frame.iter().copied().map(to_f32).sum::<f32>() / frame.len() as f32
                        })
                        .collect()
                };
                service.feed(&app, &mono, rate, &session_id);
            },
            |e| eprintln!("speech: audio stream error: {e}"),
            None,
        )
        .map_err(|e| format!("build input stream: {e}"))
}
