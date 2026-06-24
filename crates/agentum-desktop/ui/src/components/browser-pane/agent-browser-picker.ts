// Pure geometry for the agent-browser element picker overlay (W2).
//
// The screencast <canvas> is sized to the frame's DEVICE pixels, but the page's
// layout viewport — and thus every CDP / getBoundingClientRect coordinate — is in
// CSS px, reported per frame as `metadata.deviceWidth/deviceHeight`. The pane sets
// the page's viewport to the canvas's CSS box size, so the frame aspect ratio
// equals the canvas box aspect ratio (no `object-contain` letterboxing) and the
// map between client px, CSS-viewport px, and overlay px is a single linear scale.
//
// These helpers are the two directions of that map. They're pure (no DOM/canvas)
// so the coordinate math is unit-tested directly — the picker's correctness hinges
// on it, and a canvas is painful to assert against.

/** A DOMRect-like box (only the fields we use), in client/display px. */
export type CanvasBox = { left: number; top: number; width: number; height: number }

/** A point in the page's CSS-viewport ("device") coordinate space CDP expects. */
export type DevicePoint = { x: number; y: number }

/** An element rect in CSS-viewport px (from CDP `getBoundingClientRect`). */
export type ElementClip = { x: number; y: number; width: number; height: number }

/** An overlay rect in canvas-relative display px (top-left = canvas top-left). */
export type OverlayRect = { left: number; top: number; width: number; height: number }

/**
 * Map a client (mouse) point to the CSS-viewport coords the CDP page expects.
 * Mirrors `AgentBrowserScreencastPane.toDevicePoint`. Returns `null` for a
 * degenerate box so callers skip the event rather than divide by zero.
 */
export function pointToDevice(
  clientX: number,
  clientY: number,
  rect: CanvasBox,
  deviceWidth: number,
  deviceHeight: number
): DevicePoint | null {
  if (rect.width <= 0 || rect.height <= 0 || deviceWidth <= 0 || deviceHeight <= 0) {
    return null
  }
  return {
    x: Math.round(((clientX - rect.left) / rect.width) * deviceWidth),
    y: Math.round(((clientY - rect.top) / rect.height) * deviceHeight)
  }
}

/**
 * Inverse of {@link pointToDevice} for a rect: place an element's CSS-viewport
 * clip as an overlay rect in canvas-relative display px (its top-left is relative
 * to the canvas top-left, so the overlay layer is positioned over the canvas with
 * the same origin). Returns `null` for a degenerate box.
 */
export function clipToOverlayRect(
  clip: ElementClip,
  rect: CanvasBox,
  deviceWidth: number,
  deviceHeight: number
): OverlayRect | null {
  if (rect.width <= 0 || rect.height <= 0 || deviceWidth <= 0 || deviceHeight <= 0) {
    return null
  }
  const scaleX = rect.width / deviceWidth
  const scaleY = rect.height / deviceHeight
  return {
    left: clip.x * scaleX,
    top: clip.y * scaleY,
    width: clip.width * scaleX,
    height: clip.height * scaleY
  }
}
