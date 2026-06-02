import { listen, emit } from '@tauri-apps/api/event'

export async function onPtyOutput(id: string, callback: (data: Uint8Array) => void) {
  return listen<{ id: string; data: number[] }>('pty-output', (event) => {
    if (event.payload.id === id) {
      callback(new Uint8Array(event.payload.data))
    }
  })
}

export async function onFsChange(path: string, callback: (event: any) => void) {
  return listen<{ path: string; kind: string }>('fs-changed', (event) => {
    if (event.payload.path.startsWith(path)) {
      callback(event.payload)
    }
  })
}

export async function onGitChange(path: string, callback: (event: any) => void) {
  return listen<{ path: string }>('git-changed', (event) => {
    if (event.payload.path === path) {
      callback(event.payload)
    }
  })
}
