import { api } from '@/tauri'
const isMac = navigator.userAgent.includes('Mac')
let currentUIZoomLevel = 0

export function getCurrentUIZoomLevel(): number {
  return currentUIZoomLevel
}

/**
 * Apply a UI zoom level change: sets webFrame zoom via the preload API,
 * updates the CSS variable used to compensate the traffic-light pad,
 * and repositions the native macOS traffic lights to stay aligned.
 */
export function applyUIZoom(level: number): void {
  currentUIZoomLevel = level
  const zoomFactor = Math.pow(1.2, level)
  void api.ui.setZoomLevel(level)
  document.documentElement.style.setProperty('--ui-zoom-factor', String(zoomFactor))
  if (isMac) {
    api.ui.syncTrafficLights(zoomFactor)
  }
}

/**
 * Sync the CSS variable with the current webFrame zoom level.
 * Call on startup after the main process has restored the zoom.
 */
export async function syncZoomCSSVar(): Promise<void> {
  try {
    applyUIZoom(Number(await api.ui.getZoomLevel()))
  } catch {
    applyUIZoom(currentUIZoomLevel)
  }
}
