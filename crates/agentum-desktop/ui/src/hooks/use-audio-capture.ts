import { invoke } from '@tauri-apps/api/core'
import { useRef, useCallback } from 'react'

// Microphone capture for Voice dictation.
//
// Previously this captured audio in the webview via
// `navigator.mediaDevices.getUserMedia` + AudioContext/ScriptProcessor and fed
// Float32 chunks back over IPC. That API does NOT exist in macOS WKWebView
// (Tauri's webview) — `navigator.mediaDevices` is `undefined`, so dictation
// failed with "undefined is not an object". Capture now happens natively in Rust
// (cpal/CoreAudio); this hook just drives the native start/stop commands and the
// Rust side feeds the STT engine directly. The transcript events arrive through
// the same `speech-*` listeners as before, so DictationController is unchanged.

type StartAudioCaptureOptions = {
  // Accepted for API compatibility with the old getUserMedia path. Native
  // capture feeds the (kept-warm) engine directly, so there is no separate JS
  // buffering stage; the flag is ignored.
  bufferAudio?: boolean
  sessionId?: string
}

type StopAudioCaptureOptions = {
  preserveBufferedAudio?: boolean
}

export function useAudioCapture() {
  const isCapturingRef = useRef(false)
  // True once a capture session has started successfully. Used by
  // getCapturedChunkCount so the controller's "audio was captured but no final
  // transcript" branch still triggers. (Native capture owns the real sample
  // count; surfacing it per-chunk would mean an IPC event storm for no benefit.)
  const capturedRef = useRef(false)

  const start = useCallback(async (options: StartAudioCaptureOptions = {}) => {
    if (isCapturingRef.current) {
      return
    }
    const sessionId = options.sessionId ?? 'desktop'
    capturedRef.current = false
    // Native command opens the default input device and streams samples into the
    // engine. Rejects on a real device/permission error, which the caller surfaces.
    await invoke('speech_start_capture', { value: sessionId })
    isCapturingRef.current = true
    capturedRef.current = true
  }, [])

  const stop = useCallback((_options: StopAudioCaptureOptions = {}) => {
    isCapturingRef.current = false
    void invoke('speech_stop_capture').catch(() => undefined)
  }, [])

  // No JS-side buffer anymore — native capture feeds the engine directly. These
  // are retained as no-ops so the dictation controller's flow is unchanged.
  const flushBufferedAudio = useCallback(async () => undefined, [])
  const discardBufferedAudio = useCallback(() => undefined, [])

  const getCapturedChunkCount = useCallback(() => (capturedRef.current ? 1 : 0), [])

  return {
    start,
    stop,
    flushBufferedAudio,
    discardBufferedAudio,
    getCapturedChunkCount,
    isCapturingRef
  }
}
