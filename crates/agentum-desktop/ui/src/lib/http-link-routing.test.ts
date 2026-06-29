import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { FLOATING_TERMINAL_WORKTREE_ID } from '../../../shared/constants'
import {
  openHttpLink,
  registerHttpLinkStoreAccessor,
  resetRecentHttpLinkOpensForTests
} from './http-link-routing'

const openUrlMock = vi.fn()
const setActiveWorktreeMock = vi.fn()
const createBrowserTabMock = vi.fn()

const storeState = {
  settings: undefined as
    | { openLinksInApp?: boolean; activeRuntimeEnvironmentId?: string | null }
    | undefined,
  setActiveWorktree: setActiveWorktreeMock,
  createBrowserTab: createBrowserTabMock
}

beforeEach(() => {
  vi.clearAllMocks()
  resetRecentHttpLinkOpensForTests()
  storeState.settings = undefined
  registerHttpLinkStoreAccessor(() => storeState)
  vi.stubGlobal('window', {
    api: {
      shell: {
        openUrl: openUrlMock
      }
    }
  })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('openHttpLink', () => {
  it('routes into Agentum when openLinksInApp is on and a worktree is known', () => {
    storeState.settings = { openLinksInApp: true }

    openHttpLink('https://example.com/', { worktreeId: 'wt-1' })

    expect(setActiveWorktreeMock).toHaveBeenCalledWith('wt-1')
    expect(createBrowserTabMock).toHaveBeenCalledWith('wt-1', 'https://example.com/', {
      activate: true
    })
    expect(openUrlMock).not.toHaveBeenCalled()
  })

  it('defaults to Agentum routing when settings have not hydrated', () => {
    storeState.settings = undefined

    openHttpLink('https://example.com/', { worktreeId: 'wt-1' })

    expect(createBrowserTabMock).toHaveBeenCalled()
    expect(openUrlMock).not.toHaveBeenCalled()
  })

  it('routes floating workspace links into Agentum without changing the active repo worktree', () => {
    storeState.settings = { openLinksInApp: true }

    openHttpLink('https://example.com/', { worktreeId: FLOATING_TERMINAL_WORKTREE_ID })

    expect(setActiveWorktreeMock).not.toHaveBeenCalled()
    expect(createBrowserTabMock).toHaveBeenCalledWith(
      FLOATING_TERMINAL_WORKTREE_ID,
      'https://example.com/',
      { activate: true }
    )
    expect(openUrlMock).not.toHaveBeenCalled()
  })

  it('routes to the system browser when openLinksInApp is off', () => {
    storeState.settings = { openLinksInApp: false }

    openHttpLink('https://example.com/', { worktreeId: 'wt-1' })

    expect(openUrlMock).toHaveBeenCalledWith('https://example.com/')
    expect(createBrowserTabMock).not.toHaveBeenCalled()
  })

  it('routes to the system browser when a remote runtime environment is active', () => {
    storeState.settings = { openLinksInApp: true, activeRuntimeEnvironmentId: 'env-1' }

    openHttpLink('https://example.com/', { worktreeId: 'wt-1' })

    expect(openUrlMock).toHaveBeenCalledWith('https://example.com/')
    expect(createBrowserTabMock).not.toHaveBeenCalled()
    expect(setActiveWorktreeMock).not.toHaveBeenCalled()
  })

  it('routes to the system browser when no worktree id is provided', () => {
    storeState.settings = { openLinksInApp: true }

    openHttpLink('https://example.com/', { worktreeId: '' })

    expect(openUrlMock).toHaveBeenCalledWith('https://example.com/')
    expect(createBrowserTabMock).not.toHaveBeenCalled()
  })

  it('forceSystemBrowser overrides the setting even when a worktree is active', () => {
    storeState.settings = { openLinksInApp: true }

    openHttpLink('https://example.com/', { worktreeId: 'wt-1', forceSystemBrowser: true })

    expect(openUrlMock).toHaveBeenCalledWith('https://example.com/')
    expect(createBrowserTabMock).not.toHaveBeenCalled()
    expect(setActiveWorktreeMock).not.toHaveBeenCalled()
  })

  it('coalesces the same URL opened twice in quick succession (one ⌘+click, two paths)', () => {
    storeState.settings = { openLinksInApp: false }

    // The WebLinksAddon click handler and the mouseup fallback both call
    // openHttpLink for a single ⌘+click; the user should get exactly one tab.
    openHttpLink('https://example.com/', { worktreeId: 'wt-1' })
    openHttpLink('https://example.com/', { worktreeId: 'wt-1' })

    expect(openUrlMock).toHaveBeenCalledTimes(1)
  })

  it('still opens a different URL right after another', () => {
    storeState.settings = { openLinksInApp: false }

    openHttpLink('https://example.com/', { worktreeId: 'wt-1' })
    openHttpLink('https://other.test/', { worktreeId: 'wt-1' })

    expect(openUrlMock).toHaveBeenCalledTimes(2)
  })
})
