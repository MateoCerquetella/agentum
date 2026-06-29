import { isRecord } from './type-guards'

const PI_OVERLAY_HIDE_THINKING_BLOCK = true
const PI_OVERLAY_CLEAR_ON_SHRINK = true

export function mergePiOverlayUiSettings(settings: unknown): Record<string, unknown> {
  const merged = isRecord(settings) ? { ...settings } : {}
  const terminal = isRecord(merged.terminal) ? { ...merged.terminal } : {}

  terminal.clearOnShrink = PI_OVERLAY_CLEAR_ON_SHRINK
  merged.terminal = terminal
  merged.hideThinkingBlock = PI_OVERLAY_HIDE_THINKING_BLOCK

  return merged
}
