import { afterEach, describe, expect, it, vi } from 'vitest'
import { applyPersistedHostOrder, loadHostOrder, saveHostOrder } from './sidebar-host-order'

/** Minimal in-memory `Storage` so the persistence round-trip is exercised
 *  without a real DOM (and without leaking between tests). */
function makeMemoryStorage(): Storage {
  const map = new Map<string, string>()
  return {
    get length() {
      return map.size
    },
    clear: () => map.clear(),
    getItem: (key: string) => (map.has(key) ? map.get(key)! : null),
    key: (index: number) => Array.from(map.keys())[index] ?? null,
    removeItem: (key: string) => {
      map.delete(key)
    },
    setItem: (key: string, value: string) => {
      map.set(key, value)
    }
  }
}

describe('applyPersistedHostOrder', () => {
  it('pins local first and applies the persisted SSH order', () => {
    expect(
      applyPersistedHostOrder(['local', 'ssh:a', 'ssh:b', 'ssh:c'], ['ssh:c', 'ssh:a', 'ssh:b'])
    ).toEqual(['local', 'ssh:c', 'ssh:a', 'ssh:b'])
  })

  it('keeps local first even when a stale persisted array lists it', () => {
    // A stale persisted `'local'` can never move the local host or duplicate it.
    expect(applyPersistedHostOrder(['local', 'ssh:a'], ['ssh:a', 'local'])).toEqual([
      'local',
      'ssh:a'
    ])
  })

  it('appends a host missing from the persisted order after the ordered hosts', () => {
    // ssh:new was added after the order was saved — it appears last, no error.
    expect(applyPersistedHostOrder(['local', 'ssh:a', 'ssh:b', 'ssh:new'], ['ssh:b', 'ssh:a'])).toEqual(
      ['local', 'ssh:b', 'ssh:a', 'ssh:new']
    )
  })

  it('drops a stale persisted id no longer present without throwing', () => {
    // ssh:gone was removed since the order was saved — skipped, remaining applied.
    expect(applyPersistedHostOrder(['local', 'ssh:a'], ['ssh:gone', 'ssh:a'])).toEqual([
      'local',
      'ssh:a'
    ])
  })

  it('removes duplicate persisted ids', () => {
    expect(applyPersistedHostOrder(['local', 'ssh:a', 'ssh:b'], ['ssh:b', 'ssh:b', 'ssh:a'])).toEqual(
      ['local', 'ssh:b', 'ssh:a']
    )
  })

  it('falls back to local-first, first-seen order when nothing is persisted', () => {
    expect(applyPersistedHostOrder(['local', 'ssh:a', 'ssh:b'], [])).toEqual([
      'local',
      'ssh:a',
      'ssh:b'
    ])
  })

  it('works when there is no local host present', () => {
    expect(applyPersistedHostOrder(['ssh:a', 'ssh:b'], ['ssh:b', 'ssh:a'])).toEqual([
      'ssh:b',
      'ssh:a'
    ])
  })
})

describe('saveHostOrder / loadHostOrder round-trip', () => {
  it('reads back the exact sequence that was saved', () => {
    const storage = makeMemoryStorage()
    const order = ['ssh:c', 'ssh:a', 'ssh:b']
    saveHostOrder(order, storage)
    expect(loadHostOrder(storage)).toEqual(order)
  })

  it('returns [] when nothing is persisted', () => {
    expect(loadHostOrder(makeMemoryStorage())).toEqual([])
  })

  it('returns [] on garbled or non-array persisted data', () => {
    const storage = makeMemoryStorage()
    storage.setItem('agentum.sidebar.hostOrder', '{not json')
    expect(loadHostOrder(storage)).toEqual([])
    storage.setItem('agentum.sidebar.hostOrder', '{"a":1}')
    expect(loadHostOrder(storage)).toEqual([])
  })

  it('is a no-op when storage is unavailable', () => {
    expect(() => saveHostOrder(['ssh:a'], null)).not.toThrow()
    expect(loadHostOrder(null)).toEqual([])
  })
})

describe('reorder + persist perform no network I/O', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('never calls fetch during apply / save / load', () => {
    const fetchSpy = vi.fn()
    vi.stubGlobal('fetch', fetchSpy)
    const storage = makeMemoryStorage()

    const next = applyPersistedHostOrder(['local', 'ssh:a', 'ssh:b'], ['ssh:b', 'ssh:a'])
    saveHostOrder(next, storage)
    loadHostOrder(storage)

    expect(fetchSpy).not.toHaveBeenCalled()
  })
})
