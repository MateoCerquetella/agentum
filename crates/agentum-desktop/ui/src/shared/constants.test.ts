import { describe, expect, it } from 'vitest'
import { getDefaultPrimarySelectionMiddleClickPaste, getDefaultSettings } from './constants'

describe('getDefaultSettings', () => {
  it('uses platform-consistent separators for the default workspace directory', () => {
    expect(getDefaultSettings('/Users/alice').workspaceDir).toBe('/Users/alice/agentum/workspaces')
    expect(getDefaultSettings('C:\\Users\\alice').workspaceDir).toBe(
      'C:\\Users\\alice\\agentum\\workspaces'
    )
  })

  it('enables gitignored file decorations by default', () => {
    expect(getDefaultSettings('/tmp').showGitIgnoredFiles).toBe(true)
  })

  it('uses list view for Source Control changes by default', () => {
    expect(getDefaultSettings('/tmp').sourceControlViewMode).toBe('list')
  })

  it('keeps first-work branch auto-renaming off by default for new settings', () => {
    expect(getDefaultSettings('/tmp').autoRenameBranchFromWork).toBe(false)
  })

  it('defaults the in-app browser to the fast native webview (screencast off)', () => {
    // The native WKWebView pane is GPU-composited, sharp, AND the exact surface the
    // `agentum_browser` MCP drives via the desktop bridge. Defaulting screencast ON
    // showed a slow headless Chromium (2x JPEG stream) the MCP did not control — the
    // "browser is super slow / the MCP can't use it" bug. Keep this off by default;
    // screencast stays an explicit opt-in for QA / SSH-host browsers.
    expect(getDefaultSettings('/tmp').agentBrowserScreencast).toBe(false)
  })

  it('enables separate light terminal theme by default', () => {
    expect(getDefaultSettings('/tmp').terminalUseSeparateLightTheme).toBe(true)
  })

  it('enables Source Control AI by default without pinning a separate agent', () => {
    expect(getDefaultSettings('/tmp').commitMessageAi).toMatchObject({
      enabled: true,
      agentId: null,
      selectedModelByAgent: {}
    })
    expect(getDefaultSettings('/tmp').sourceControlAi).toMatchObject({
      enabled: true,
      agentId: null,
      selectedModelByAgent: {},
      instructionsByOperation: {
        commitMessage: '',
        pullRequest: '',
        branchName: ''
      }
    })
  })

  it('keeps compact worktree cards experimental and disabled by default', () => {
    expect(getDefaultSettings('/tmp').experimentalCompactWorktreeCards).toBe(false)
  })
})

describe('getDefaultPrimarySelectionMiddleClickPaste', () => {
  it('enables primary selection paste on Linux by default', () => {
    expect(getDefaultPrimarySelectionMiddleClickPaste('linux')).toBe(true)
  })

  it('enables primary selection paste on macOS by default', () => {
    expect(getDefaultPrimarySelectionMiddleClickPaste('darwin')).toBe(true)
  })

  it('leaves primary selection paste opt-in on Windows', () => {
    expect(getDefaultPrimarySelectionMiddleClickPaste('win32')).toBe(false)
  })
})
