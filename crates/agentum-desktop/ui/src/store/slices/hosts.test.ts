import { describe, it, expect } from 'vitest'
import { createStore } from 'zustand/vanilla'
import { createHostsSlice, type HostsSlice } from './hosts'

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
