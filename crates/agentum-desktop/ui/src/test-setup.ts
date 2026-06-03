import { vi } from 'vitest'

// Tests stub `window.api` (the legacy global) to drive behaviour. Production code
// now imports `api` from '@/tauri' instead. This global mock forwards the module's
// `api` to `window.api` at access time, so every existing test keeps working without
// per-file rewrites. Resolves to undefined namespaces when a test doesn't stub
// window.api — which is fine, since those tests never touch `api`.
vi.mock('@/tauri', () => ({
  api: new Proxy(
    {},
    {
      get: (_target, namespace: string) =>
        (globalThis as unknown as { window?: { api?: Record<string, unknown> } }).window?.api?.[
          namespace
        ],
    }
  ),
}))
