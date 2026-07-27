import { constants } from 'fs'
import { lstat, open } from 'fs/promises'
import * as path from 'path'
import { isBinaryBuffer } from './binary-buffer'
import { decodeGitCQuotedPath } from './git-cquoted-path'

export type GitLineStats = { added?: number; removed?: number }

// Limits how many untracked files we read at once when counting their lines,
// so a worktree with thousands of new files cannot exhaust file descriptors.
const UNTRACKED_READ_CONCURRENCY = 8
// Keep status polling cheap: large untracked files are commonly generated
// assets, and reading them every poll can stall the source-control sidebar.
export const MAX_UNTRACKED_LINE_COUNT_BYTES = 2 * 1024 * 1024
const UNTRACKED_STATS_CACHE_MAX_ENTRIES = 2048
const NEWLINE_BYTE = 0x0a

type NoFollowOpenConstants = {
  O_RDONLY: number
  O_NOFOLLOW?: number
  O_NONBLOCK?: number
}

// Node does not expose O_NOFOLLOW/O_NONBLOCK on every supported platform. Do
// not silently degrade to a following or potentially blocking open when either
// guarantee is unavailable; line counts are optional UI metadata.
export function untrackedNoFollowReadFlags(
  fsConstants: NoFollowOpenConstants = constants
): number | null {
  if (
    typeof fsConstants.O_NOFOLLOW !== 'number' ||
    typeof fsConstants.O_NONBLOCK !== 'number'
  ) {
    return null
  }
  return fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW | fsConstants.O_NONBLOCK
}

type CachedUntrackedStats = {
  dev: number
  ino: number
  size: number
  mtimeMs: number
  ctimeMs: number
  stats: GitLineStats
}

const untrackedStatsCache = new Map<string, CachedUntrackedStats>()

type FileSnapshot = {
  dev: number
  ino: number
  size: number
  mtimeMs: number
  ctimeMs: number
  isFile(): boolean
  isSymbolicLink(): boolean
}

function isSameFile(left: FileSnapshot, right: FileSnapshot): boolean {
  return left.dev === right.dev && left.ino === right.ino
}

function isSameSnapshot(left: FileSnapshot, right: FileSnapshot): boolean {
  return (
    isSameFile(left, right) &&
    left.size === right.size &&
    left.mtimeMs === right.mtimeMs &&
    left.ctimeMs === right.ctimeMs
  )
}

function parseNumstatCount(value: string): number | undefined {
  // git reports binary files as '-' in the numstat columns.
  if (value === '-') {
    return undefined
  }
  const count = Number.parseInt(value, 10)
  return Number.isFinite(count) ? count : undefined
}

// `git diff -M` reports renames in the numstat path column as `old => new` or
// `dir/{old => new}/file`; normalize to the post-rename path so it keys to the
// porcelain status entry, which always reports the new path.
function normalizeNumstatPath(rawPath: string): string {
  const decodedPath = decodeGitCQuotedPath(rawPath)
  const braced = /^(.*)\{(.+) => (.+)\}(.*)$/.exec(decodedPath)
  if (braced) {
    return `${braced[1]}${braced[3]}${braced[4]}`
  }
  const marker = ' => '
  const markerIndex = decodedPath.lastIndexOf(marker)
  return markerIndex === -1 ? decodedPath : decodedPath.slice(markerIndex + marker.length)
}

export function parseNumstat(stdout: string): Map<string, GitLineStats> {
  if (stdout.includes('\0')) {
    return parseNulDelimitedNumstat(stdout)
  }

  const stats = new Map<string, GitLineStats>()
  for (const line of stdout.split(/\r?\n/)) {
    if (!line) {
      continue
    }
    const parts = line.split('\t')
    const rawPath = parts.slice(2).join('\t')
    if (!rawPath) {
      continue
    }
    stats.set(normalizeNumstatPath(rawPath), {
      added: parseNumstatCount(parts[0] ?? ''),
      removed: parseNumstatCount(parts[1] ?? '')
    })
  }
  return stats
}

function parseNulDelimitedNumstat(stdout: string): Map<string, GitLineStats> {
  const stats = new Map<string, GitLineStats>()
  const records = stdout.split('\0')
  for (let i = 0; i < records.length; i += 1) {
    const record = records[i]
    if (!record) {
      continue
    }
    const parts = record.split('\t')
    const rawPath = parts.slice(2).join('\t')
    let path = rawPath
    if (!path) {
      // Git -z emits rename paths as: "added<TAB>removed<TAB>\0old\0new\0".
      // The split record has an empty path in the header; the postimage is next.
      i += 2
      path = records[i] ?? ''
    }
    if (!path) {
      continue
    }
    stats.set(path, {
      added: parseNumstatCount(parts[0] ?? ''),
      removed: parseNumstatCount(parts[1] ?? '')
    })
  }
  return stats
}

async function countFileAdditions(absolutePath: string): Promise<GitLineStats> {
  const openFlags = untrackedNoFollowReadFlags()
  if (openFlags === null) {
    try {
      const pathStat = await lstat(absolutePath)
      return pathStat.isSymbolicLink()
        ? rememberUntrackedStats(absolutePath, pathStat, { added: 1 })
        : {}
    } catch {
      return {}
    }
  }

  let handle: Awaited<ReturnType<typeof open>>
  try {
    // Open the final path without following links, then bind every check and
    // read to that descriptor. Opening before lstat means a later path swap
    // cannot redirect the read; identity comparison only decides whether the
    // already-open descriptor still represents the reported path.
    handle = await open(absolutePath, openFlags)
  } catch {
    try {
      const pathStat = await lstat(absolutePath)
      return pathStat.isSymbolicLink()
        ? rememberUntrackedStats(absolutePath, pathStat, { added: 1 })
        : {}
    } catch {
      return {}
    }
  }

  try {
    const openedStat = await handle.stat()
    if (!openedStat.isFile() || openedStat.size > MAX_UNTRACKED_LINE_COUNT_BYTES) {
      return {}
    }
    const pathStat = await lstat(absolutePath)
    if (pathStat.isSymbolicLink()) {
      return rememberUntrackedStats(absolutePath, pathStat, { added: 1 })
    }
    if (!isSameFile(pathStat, openedStat)) {
      return {}
    }
    const cached = untrackedStatsCache.get(absolutePath)
    if (
      cached &&
      cached.dev === openedStat.dev &&
      cached.ino === openedStat.ino &&
      cached.size === openedStat.size &&
      cached.mtimeMs === openedStat.mtimeMs &&
      cached.ctimeMs === openedStat.ctimeMs
    ) {
      return cached.stats
    }
    const buffer = await handle.readFile()
    const completedStat = await handle.stat()
    const completedPathStat = await lstat(absolutePath)
    if (
      completedPathStat.isSymbolicLink() ||
      !isSameSnapshot(openedStat, completedStat) ||
      !isSameSnapshot(completedPathStat, completedStat) ||
      buffer.length !== completedStat.size
    ) {
      return {}
    }
    if (isBinaryBuffer(buffer)) {
      return rememberUntrackedStats(absolutePath, completedStat, {})
    }
    if (buffer.length === 0) {
      return rememberUntrackedStats(absolutePath, completedStat, { added: 0 })
    }
    let newlineCount = 0
    for (let i = 0; i < buffer.length; i += 1) {
      if (buffer[i] === NEWLINE_BYTE) {
        newlineCount += 1
      }
    }
    // A trailing newline marks the final line as complete; without one the last
    // partial line still counts as an added line (matching git's numstat).
    const endsWithNewline = buffer.at(-1) === NEWLINE_BYTE
    return rememberUntrackedStats(absolutePath, completedStat, {
      added: endsWithNewline ? newlineCount : newlineCount + 1
    })
  } catch {
    return {}
  } finally {
    await handle.close().catch(() => undefined)
  }
}

function rememberUntrackedStats(
  absolutePath: string,
  fileStat: FileSnapshot,
  stats: GitLineStats
): GitLineStats {
  untrackedStatsCache.set(absolutePath, {
    dev: fileStat.dev,
    ino: fileStat.ino,
    size: fileStat.size,
    mtimeMs: fileStat.mtimeMs,
    ctimeMs: fileStat.ctimeMs,
    stats
  })
  if (untrackedStatsCache.size > UNTRACKED_STATS_CACHE_MAX_ENTRIES) {
    const oldestKey = untrackedStatsCache.keys().next().value
    if (oldestKey) {
      untrackedStatsCache.delete(oldestKey)
    }
  }
  return stats
}

// Untracked files have no git-tracked baseline, so `git diff` ignores them.
// We count their contents directly to show an additions magnitude.
export async function collectUntrackedAdditions(
  worktreePath: string,
  untrackedPaths: readonly string[]
): Promise<Map<string, GitLineStats>> {
  const result = new Map<string, GitLineStats>()
  for (let i = 0; i < untrackedPaths.length; i += UNTRACKED_READ_CONCURRENCY) {
    const chunk = untrackedPaths.slice(i, i + UNTRACKED_READ_CONCURRENCY)
    await Promise.all(
      chunk.map(async (relativePath) => {
        result.set(relativePath, await countFileAdditions(path.join(worktreePath, relativePath)))
      })
    )
  }
  return result
}

export function applyLineStats(
  entry: { added?: number; removed?: number },
  stats: GitLineStats | undefined
): void {
  if (!stats) {
    return
  }
  if (stats.added !== undefined) {
    entry.added = stats.added
  }
  if (stats.removed !== undefined) {
    entry.removed = stats.removed
  }
}
