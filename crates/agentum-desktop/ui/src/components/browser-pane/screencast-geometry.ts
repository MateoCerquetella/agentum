/**
 * Geometry for mapping a click on the object-contain screencast <canvas> back to
 * the page's device pixels.
 *
 * Why: the canvas is styled `object-contain`, so when the frame's aspect ratio
 * differs from the canvas box (a transient letterbox on first open, before the
 * viewport re-sync lands), the painted image is inset within the box behind
 * bars. Mapping a click against the FULL box rect is then both offset and
 * mis-scaled — the click lands in the wrong place, or on a bar. These pure
 * helpers compute the actual painted content box and map into it.
 */

/** The object-contain content box (the actually-painted image region) of a frame
 *  of intrinsic size `frameW×frameH` laid out inside an element box `boxW×boxH`.
 *  object-contain scales the frame uniformly to fit and centers it, leaving equal
 *  letterbox bars on the axis with the smaller fit ratio. Returns null for a
 *  degenerate input. */
export function containContentBox(
  boxW: number,
  boxH: number,
  frameW: number,
  frameH: number
): { offsetX: number; offsetY: number; width: number; height: number } | null {
  if (boxW <= 0 || boxH <= 0 || frameW <= 0 || frameH <= 0) {
    return null
  }
  const scale = Math.min(boxW / frameW, boxH / frameH)
  const width = frameW * scale
  const height = frameH * scale
  return {
    offsetX: (boxW - width) / 2,
    offsetY: (boxH - height) / 2,
    width,
    height
  }
}

/** Map a point given RELATIVE to the element box (`clientX - rectLeft`,
 *  `clientY - rectTop`) into device/frame pixels, accounting for the
 *  object-contain letterbox. Returns null for a degenerate box OR a point that
 *  falls on a letterbox bar (outside the painted content) — so a bar click is
 *  dropped, never mis-routed to the wrong page coordinate. */
export function clientToDevicePoint(
  relX: number,
  relY: number,
  boxW: number,
  boxH: number,
  frameW: number,
  frameH: number
): { x: number; y: number } | null {
  const content = containContentBox(boxW, boxH, frameW, frameH)
  if (!content) {
    return null
  }
  const withinX = relX - content.offsetX
  const withinY = relY - content.offsetY
  if (withinX < 0 || withinY < 0 || withinX > content.width || withinY > content.height) {
    return null
  }
  return {
    x: Math.round((withinX / content.width) * frameW),
    y: Math.round((withinY / content.height) * frameH)
  }
}
