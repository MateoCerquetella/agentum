import { describe, it, expect } from 'vitest'
import { createStore } from 'zustand/vanilla'
import { createHostsSlice, unameDetail, type HostsSlice } from './hosts'

function makeStore() {
  return createStore<HostsSlice>()((...a) => ({ ...createHostsSlice(...(a as Parameters<typeof createHostsSlice>)) }))
}

describe('hosts slice', () => {
  it('starts with an empty host-meta map', () => {
    const store = makeStore()
    expect(store.getState().hostMetaByKey).toEqual({})
  })

  it('setHostMeta inserts and overwrites by key', () => {
    const store = makeStore()
    store.getState().setHostMeta('local', { key: 'local', kind: 'local', label: 'studio', detail: 'localhost · Darwin 24.5' })
    expect(store.getState().hostMetaByKey.local.label).toBe('studio')
    store.getState().setHostMeta('local', { key: 'local', kind: 'local', label: 'studio2' })
    expect(store.getState().hostMetaByKey.local.label).toBe('studio2')
    expect(store.getState().hostMetaByKey.local.detail).toBeUndefined()
  })
})

describe('unameDetail (host OS line — spec 003)', () => {
  it('composes "<transport> · <uname>" when the readiness probe returned a uname', () => {
    expect(unameDetail('localhost', 'Darwin 24.5')).toBe('localhost · Darwin 24.5')
    expect(unameDetail('ssh forge.lan', 'Linux 6.9')).toBe('ssh forge.lan · Linux 6.9')
  })

  it('degrades to just the transport prefix when the uname is unknown', () => {
    expect(unameDetail('localhost', null)).toBe('localhost')
    expect(unameDetail('ssh', null)).toBe('ssh')
  })
})
