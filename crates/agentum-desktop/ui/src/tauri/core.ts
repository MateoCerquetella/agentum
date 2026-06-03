// AUTO-GENERATED base by /tmp/gen-tauri-client.mjs, then hand-extended with
// name-derivation + defineNamespace. Replaces lib/electron-bridge.ts proxy.
// Command/event names match the old wire contract.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

// Mirrors the old proxy's argsToPayload: single object arg passes through;
// multiple args wrap as { args }; zero args -> {}; non-object single -> { value }.
function argsToPayload(args: any[]): Record<string, unknown> {
  const payload = args.length === 1 ? args[0] : args.length > 1 ? { args } : {}
  return typeof payload === 'object' && payload !== null ? payload : { value: payload }
}

export function call(command: string, args: any[]): Promise<any> {
  return invoke(command, argsToPayload(args))
}

// Subscribe to a Tauri event; returns an unsubscribe fn (matches old proxy on* semantics).
export function subscribe(event: string, callback: (payload: any) => void): () => void {
  let unlisten: null | (() => void) = null
  void listen(event, (e) => callback(e.payload as any))
    .then((dispose) => {
      unlisten = dispose
    })
    .catch(() => {
      unlisten = null
    })
  return () => {
    unlisten?.()
  }
}

// Name derivation, identical to the rules the old proxy used (electron-bridge.ts).
function snake(segment: string): string {
  return segment
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1_$2')
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/[:.-]/g, '_')
    .toLowerCase()
}
function kebab(segment: string): string {
  return segment
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1-$2')
    .replace(/([a-z0-9])([A-Z])/g, '$1-$2')
    .replace(/[:._]/g, '-')
    .toLowerCase()
}

// Wrap a namespace's EXPLICIT methods (the typed, greppable surface) with a thin
// fallback: any method/event NOT explicitly listed is synthesized into the same
// Tauri command/event the old dynamic bridge produced. This covers methods reached
// via aliases / dynamic access that a static rewrite cannot enumerate, so the app
// has exact parity with the previous behaviour — without a global `window.api`
// string-munging proxy. Known methods stay explicit (no Proxy overhead for them).
export function defineNamespace<T extends object>(nsName: string, explicit: T): T {
  return new Proxy(explicit, {
    get(target, key) {
      if (typeof key !== 'string' || key === 'then' || key in target) {
        return (target as Record<string | symbol, unknown>)[key as string]
      }
      if (/^on[A-Z]/.test(key)) {
        const event = `${kebab(nsName)}-${kebab(key.slice(2))}`
        return (callback: (payload: any) => void) => subscribe(event, callback)
      }
      const command = `${snake(nsName)}_${snake(key)}`
      return (...args: any[]) => call(command, args)
    },
  }) as T
}
