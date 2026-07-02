import type { IBufferLine, IBufferRange, IDisposable, Terminal } from '@xterm/xterm'
import { openHttpLink } from '@/lib/http-link-routing'
import { buildCandidateLogicalLinesForBufferPosition, rangeContainsBufferPosition } from './terminal-file-link-hit-testing'
import { rangeForParsedFileLink } from './wrapped-terminal-link-ranges'

type UrlLinkHitTestDeps = {
  worktreeId: string
  forceSystemBrowser?: boolean
}

type UrlLinkClickFallbackDeps = {
  worktreeId: string
}

type ParsedTerminalHttpLink = {
  url: string
  startIndex: number
  endIndex: number
}

// Mirrors @xterm/addon-web-links' strict URL matcher so fallback clicks use
// the same visible URL span as xterm's hover-time WebLinksAddon provider.
const TERMINAL_HTTP_URL_REGEX = /\bhttps?:\/\/[^\s"'!*(){}|\\^<>`]*[^\s"':,.!?{}|\\^~[\]`()<>]/gi

function extractTerminalHttpLinks(lineText: string): ParsedTerminalHttpLink[] {
  const links: ParsedTerminalHttpLink[] = []
  for (const match of lineText.matchAll(TERMINAL_HTTP_URL_REGEX)) {
    const url = match[0]
    const index = match.index ?? 0
    let parsed: URL
    try {
      parsed = new URL(url)
    } catch {
      continue
    }
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      continue
    }
    links.push({ url: parsed.toString(), startIndex: index, endIndex: index + url.length })
  }
  return links
}

function isTerminalLinkActivation(
  event: Pick<MouseEvent, 'metaKey' | 'ctrlKey'> | undefined
): boolean {
  const isMac = navigator.userAgent.includes('Mac')
  return isMac ? Boolean(event?.metaKey) : Boolean(event?.ctrlKey)
}

function getTerminalScreenElement(terminal: Terminal): HTMLElement | null {
  return terminal.element?.querySelector('.xterm-screen') ?? null
}

export function getBufferPositionForTerminalMouseEvent(
  terminal: Terminal,
  event: MouseEvent
): { x: number; y: number } | null {
  const screenElement = getTerminalScreenElement(terminal)
  if (!screenElement || terminal.cols <= 0 || terminal.rows <= 0) {
    return null
  }

  const rect = screenElement.getBoundingClientRect()
  const relativeX = event.clientX - rect.left
  const relativeY = event.clientY - rect.top
  if (relativeX < 0 || relativeY < 0 || relativeX >= rect.width || relativeY >= rect.height) {
    return null
  }

  const cellWidth = rect.width / terminal.cols
  const cellHeight = rect.height / terminal.rows
  if (cellWidth <= 0 || cellHeight <= 0) {
    return null
  }

  return {
    x: Math.floor(relativeX / cellWidth) + 1,
    y: Math.floor(relativeY / cellHeight) + terminal.buffer.active.viewportY + 1
  }
}

export function installHttpLinkClickFallback(
  terminal: Terminal,
  deps: UrlLinkClickFallbackDeps
): IDisposable {
  const handleMouseUp = (event: MouseEvent): void => {
    // Why: do NOT bail on event.defaultPrevented. In an agent terminal with mouse
    // tracking on, xterm consumes the click for its mouse report and
    // preventDefaults the mouseup, and the WebLinksAddon never fires — so the old
    // defaultPrevented guard made ⌘/Ctrl+click do nothing on URLs. We still open
    // here; double-opening (addon + this fallback for one click) is prevented by
    // openHttpLink's coalescing, not by skipping the click.
    if (event.button !== 0 || !isTerminalLinkActivation(event)) {
      return
    }

    const position = getBufferPositionForTerminalMouseEvent(terminal, event)
    if (!position) {
      return
    }

    const opened = openHttpLinkAtBufferPosition(terminal.buffer.active, position, terminal.cols, {
      worktreeId: deps.worktreeId,
      forceSystemBrowser: event.shiftKey
    })
    if (opened) {
      event.preventDefault()
      terminal.clearSelection()
    }
  }

  const terminalElement = terminal.element
  terminalElement?.addEventListener('mouseup', handleMouseUp)
  return {
    dispose: () => {
      terminalElement?.removeEventListener('mouseup', handleMouseUp)
    }
  }
}

export function openHttpLinkAtBufferPosition(
  buffer: { getLine(y: number): IBufferLine | undefined },
  position: { x: number; y: number },
  terminalColumns: number,
  deps: UrlLinkHitTestDeps
): boolean {
  const logicalLines = buildCandidateLogicalLinesForBufferPosition(buffer, position.y)
  if (logicalLines.length === 0) {
    return false
  }

  for (const logicalLine of logicalLines) {
    for (const parsed of extractTerminalHttpLinks(logicalLine.text)) {
      const range = rangeForParsedFileLink(logicalLine, parsed.startIndex, parsed.endIndex)
      if (!range || !rangeContainsBufferPosition(range, position, terminalColumns)) {
        continue
      }
      openHttpLink(parsed.url, {
        worktreeId: deps.worktreeId,
        forceSystemBrowser: deps.forceSystemBrowser
      })
      return true
    }
  }

  return false
}

