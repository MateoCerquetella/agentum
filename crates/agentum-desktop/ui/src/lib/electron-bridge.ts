import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

function channelToCommand(channel: string): string {
  return channel.replace(/:/g, '_').replace(/-/g, '_')
}

function segmentToSnakeCase(segment: string): string {
  return segment
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1_$2')
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/[:.-]/g, '_')
    .toLowerCase()
}

function segmentToKebabCase(segment: string): string {
  return segment
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1-$2')
    .replace(/([a-z0-9])([A-Z])/g, '$1-$2')
    .replace(/[:._]/g, '-')
    .toLowerCase()
}

function argsToPayload(args: any[]): Record<string, unknown> {
  const payload = args.length === 1 ? args[0] : args.length > 1 ? { args } : {}
  return typeof payload === 'object' && payload !== null ? payload : { value: payload }
}

function pathToCommand(path: string[]): string {
  return path.map(segmentToSnakeCase).join('_')
}

function pathToEvent(path: string[]): string {
  return path.map(segmentToKebabCase).join('-')
}

const ipcRenderer = {
  invoke: async (channel: string, ...args: any[]) => {
    return invoke(channelToCommand(channel), argsToPayload(args))
  },
  send: (channel: string, ...args: any[]) => {
    void invoke(channelToCommand(channel), argsToPayload(args)).catch(() => {})
  },
  on: (channel: string, callback: (...args: any[]) => void) => {
    const eventName = channel.replace(/:/g, '-')
    void listen(eventName, (event) => {
      callback({ sender: {} }, event.payload)
    })
    return ipcRenderer
  },
  once: (channel: string, callback: (...args: any[]) => void) => {
    const eventName = channel.replace(/:/g, '-')
    const unlisten = listen(eventName, (event) => {
      callback({ sender: {} }, event.payload)
      void unlisten.then((fn) => fn())
    })
    return ipcRenderer
  },
  removeListener: (_channel: string, _callback: any) => ipcRenderer,
  removeAllListeners: (_channel?: string) => ipcRenderer,
}

function createApiProxy(path: string[] = []): any {
  return new Proxy(() => {}, {
    get: (_target, prop: string | symbol) => {
      if (typeof prop !== 'string' || prop === 'then') {
        return undefined
      }
      return createApiProxy([...path, prop])
    },
    apply: (_target, _thisArg, argArray: any[]) => {
      const lastSegment = path[path.length - 1]
      if (lastSegment?.startsWith('on') && typeof argArray[0] === 'function') {
        const eventPath = [...path.slice(0, -1), lastSegment.slice(2)]
        let unlisten: null | (() => void) = null
        void listen(pathToEvent(eventPath), (event) => {
          argArray[0](event.payload)
        })
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

      return invoke(pathToCommand(path), argsToPayload(argArray))
    },
  })
}

;(window as any).electron = {
  ipcRenderer,
  process: {
    platform: navigator.userAgent.includes('Mac')
      ? 'darwin'
      : navigator.userAgent.includes('Win')
        ? 'win32'
        : 'linux',
    versions: {
      electron: '0.0.0-tauri',
      node: '0.0.0-tauri',
      chrome: navigator.userAgent,
    },
    env: {},
  },
  webFrame: {
    setZoomFactor: (_factor: number) => {},
    getZoomFactor: () => 1.0,
  },
  webUtils: {
    getPathForFile: (file: File) => file.name,
  },
}

// Why: the copied renderer still talks to the nested Electron preload surface,
// so this proxy turns namespace.method calls into Tauri commands/events.
;(window as any).api = createApiProxy()
