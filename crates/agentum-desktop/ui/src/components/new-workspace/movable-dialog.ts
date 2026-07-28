export type DialogOffset = {
  x: number
  y: number
}

export type DialogBaseRect = {
  left: number
  top: number
  right: number
  bottom: number
}

const DEFAULT_VIEWPORT_GUTTER = 16

function clamp(value: number, minimum: number, maximum: number): number {
  if (minimum > maximum) {
    return (minimum + maximum) / 2
  }
  return Math.min(maximum, Math.max(minimum, value))
}

/** Keep a translated, center-positioned dialog fully inside the viewport. */
export function clampDialogOffset(input: {
  desiredOffset: DialogOffset
  baseRect: DialogBaseRect
  viewportWidth: number
  viewportHeight: number
  gutter?: number
}): DialogOffset {
  const gutter = input.gutter ?? DEFAULT_VIEWPORT_GUTTER
  return {
    x: clamp(
      input.desiredOffset.x,
      gutter - input.baseRect.left,
      input.viewportWidth - gutter - input.baseRect.right
    ),
    y: clamp(
      input.desiredOffset.y,
      gutter - input.baseRect.top,
      input.viewportHeight - gutter - input.baseRect.bottom
    )
  }
}
