// Browser-safe base64 <-> bytes/utf-8 helpers. Replaces Node's `Buffer` in
// renderer-bundled shared code (Vite does not polyfill Node globals).

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = ''
  // Chunk to stay well under the String.fromCharCode argument limit on large inputs.
  const chunk = 0x8000
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk))
  }
  return btoa(binary)
}

export function base64ToBytes(base64: string): Uint8Array {
  const binary = atob(base64)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i)
  }
  return bytes
}

export function utf8ToBase64(text: string): string {
  return bytesToBase64(new TextEncoder().encode(text))
}

export function base64ToUtf8(base64: string): string {
  return new TextDecoder().decode(base64ToBytes(base64))
}
