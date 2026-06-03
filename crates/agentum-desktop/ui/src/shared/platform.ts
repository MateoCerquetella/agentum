// Browser-safe platform detection, replacing Node's `process.platform` in
// renderer-bundled shared code. Returns Node-style ids so existing comparisons
// (=== 'win32' | 'linux' | 'darwin') keep working. Empty string when the
// platform can't be determined (non-DOM context).
export function currentPlatform(): 'darwin' | 'win32' | 'linux' | '' {
  const ua =
    typeof navigator !== 'undefined' ? `${navigator.userAgent} ${navigator.platform ?? ''}` : ''
  if (/Mac|iPhone|iPad/.test(ua)) {
    return 'darwin'
  }
  if (/Win/.test(ua)) {
    return 'win32'
  }
  if (/Linux|X11|Android/.test(ua)) {
    return 'linux'
  }
  return ''
}
