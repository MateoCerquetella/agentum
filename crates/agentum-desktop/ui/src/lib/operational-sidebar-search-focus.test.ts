import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  focusOperationalSidebarSearch,
  OPERATIONAL_SIDEBAR_SEARCH_FOCUS_EVENT,
  requestOperationalSidebarSearchFocus
} from './operational-sidebar-search-focus'

describe('operational sidebar search focus', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('focuses and selects an available input', () => {
    const input = { focus: vi.fn(), select: vi.fn() }
    expect(focusOperationalSidebarSearch(input as unknown as HTMLInputElement)).toBe(true)
    expect(input.focus).toHaveBeenCalledOnce()
    expect(input.select).toHaveBeenCalledOnce()
  })

  it('emits the shared request event', () => {
    const target = new EventTarget()
    vi.stubGlobal('window', target)
    const listener = vi.fn()
    target.addEventListener(OPERATIONAL_SIDEBAR_SEARCH_FOCUS_EVENT, listener, { once: true })
    requestOperationalSidebarSearchFocus()
    expect(listener).toHaveBeenCalledOnce()
  })
})
