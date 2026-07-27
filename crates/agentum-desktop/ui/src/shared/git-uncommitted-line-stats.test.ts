import { beforeEach, describe, expect, it, vi } from 'vitest'
import { constants } from 'fs'

const { lstatMock, openMock } = vi.hoisted(() => ({
  lstatMock: vi.fn(),
  openMock: vi.fn()
}))

vi.mock('fs/promises', () => ({ lstat: lstatMock, open: openMock }))

import {
  applyLineStats,
  collectUntrackedAdditions,
  MAX_UNTRACKED_LINE_COUNT_BYTES,
  parseNumstat,
  untrackedNoFollowReadFlags
} from './git-uncommitted-line-stats'

function mockFileStat(size: number, mtimeMs = 1, ino = 1) {
  return {
    dev: 7,
    ino,
    size,
    mtimeMs,
    ctimeMs: mtimeMs,
    isFile: () => true,
    isSymbolicLink: () => false
  }
}

function mockFileHandle(
  contents: Buffer,
  openedStat = mockFileStat(contents.length),
  completedStat = openedStat
) {
  const stat = vi.fn()
  if (completedStat === openedStat) {
    stat.mockResolvedValue(openedStat)
  } else {
    stat.mockResolvedValueOnce(openedStat).mockResolvedValueOnce(completedStat)
  }
  return {
    stat,
    readFile: vi.fn().mockResolvedValue(contents),
    close: vi.fn().mockResolvedValue(undefined)
  }
}

function mockRegularFile(contents: Buffer, fileStat = mockFileStat(contents.length)) {
  const handle = mockFileHandle(contents, fileStat)
  lstatMock.mockResolvedValue(fileStat)
  openMock.mockResolvedValue(handle)
  return handle
}

describe('parseNumstat', () => {
  it('parses added/removed counts keyed by path', () => {
    const stats = parseNumstat('3\t4\tsrc/app.ts\n10\t0\tsrc/new.ts\n')
    expect(stats.get('src/app.ts')).toEqual({ added: 3, removed: 4 })
    expect(stats.get('src/new.ts')).toEqual({ added: 10, removed: 0 })
  })

  it('treats binary "-" columns as undefined counts', () => {
    expect(parseNumstat('-\t-\tassets/logo.png\n').get('assets/logo.png')).toEqual({
      added: undefined,
      removed: undefined
    })
  })

  it('keys renames to the post-rename path', () => {
    const braced = parseNumstat('2\t1\tsrc/{old => new}/file.ts\n')
    expect(braced.get('src/new/file.ts')).toEqual({ added: 2, removed: 1 })
    const plain = parseNumstat('2\t1\told.ts => new.ts\n')
    expect(plain.get('new.ts')).toEqual({ added: 2, removed: 1 })
  })

  it('keeps literal rename-marker filenames when parsing NUL-delimited numstat', () => {
    const stats = parseNumstat('1\t0\tdocs/a => b.txt\0')

    expect(stats.get('docs/a => b.txt')).toEqual({ added: 1, removed: 0 })
  })

  it('keys NUL-delimited renames to the post-rename path', () => {
    const stats = parseNumstat('2\t1\t\0old.ts\0new.ts\0')

    expect(stats.get('new.ts')).toEqual({ added: 2, removed: 1 })
  })

  it('decodes Git C-quoted paths before keying stats', () => {
    expect(parseNumstat('1\t1\t"tab\\tfile.txt"\n').get('tab\tfile.txt')).toEqual({
      added: 1,
      removed: 1
    })
  })

  it('ignores blank lines', () => {
    expect(parseNumstat('').size).toBe(0)
  })
})

describe('collectUntrackedAdditions', () => {
  beforeEach(() => {
    lstatMock.mockReset()
    openMock.mockReset()
  })

  it('counts file lines as additions, with or without a trailing newline', async () => {
    lstatMock.mockImplementation((target: string) => {
      const contents = String(target).endsWith('trailing.ts')
        ? Buffer.from('a\nb\nc\n')
        : Buffer.from('a\nb\nc')
      return Promise.resolve(mockFileStat(contents.length, 1, contents.length))
    })
    openMock.mockImplementation(async (target: string) => {
      const contents = String(target).endsWith('trailing.ts')
        ? Buffer.from('a\nb\nc\n')
        : Buffer.from('a\nb\nc')
      return mockFileHandle(contents, mockFileStat(contents.length, 1, contents.length))
    })
    const stats = await collectUntrackedAdditions('/repo', ['trailing.ts', 'no-trailing.ts'])
    expect(stats.get('trailing.ts')).toEqual({ added: 3 })
    expect(stats.get('no-trailing.ts')).toEqual({ added: 3 })
  })

  it('reports an empty file as zero additions', async () => {
    mockRegularFile(Buffer.from(''))
    expect((await collectUntrackedAdditions('/repo', ['empty.ts'])).get('empty.ts')).toEqual({
      added: 0
    })
  })

  it('omits counts for binary files', async () => {
    mockRegularFile(Buffer.from([0x00, 0x01, 0x02]))
    expect((await collectUntrackedAdditions('/repo', ['bin.dat'])).get('bin.dat')).toEqual({})
  })

  it('counts untracked symbolic links without following the target', async () => {
    openMock.mockRejectedValue(Object.assign(new Error('symbolic link'), { code: 'ELOOP' }))
    lstatMock.mockResolvedValue({
      dev: 7,
      ino: 11,
      size: 4,
      mtimeMs: 2,
      ctimeMs: 2,
      isFile: () => false,
      isSymbolicLink: () => true
    })

    expect((await collectUntrackedAdditions('/repo', ['link.txt'])).get('link.txt')).toEqual({
      added: 1
    })
    expect(openMock).toHaveBeenCalledWith(
      '/repo/link.txt',
      constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK
    )
  })

  it('skips oversized untracked files instead of reading them during status polling', async () => {
    const oversizedStat = mockFileStat(MAX_UNTRACKED_LINE_COUNT_BYTES + 1, 3)
    const handle = mockFileHandle(Buffer.alloc(0), oversizedStat)
    openMock.mockResolvedValue(handle)

    expect((await collectUntrackedAdditions('/repo', ['large.log'])).get('large.log')).toEqual({})
    expect(lstatMock).not.toHaveBeenCalled()
    expect(handle.readFile).not.toHaveBeenCalled()
    expect(handle.close).toHaveBeenCalledTimes(1)
  })

  it('reuses cached counts while size and mtime are unchanged', async () => {
    const handle = mockRegularFile(Buffer.from('a\nb\nc'), mockFileStat(5, 4))

    await collectUntrackedAdditions('/repo', ['cached.ts'])
    const stats = await collectUntrackedAdditions('/repo', ['cached.ts'])

    expect(stats.get('cached.ts')).toEqual({ added: 3 })
    expect(handle.readFile).toHaveBeenCalledTimes(1)
  })

  it('reads through a no-follow descriptor bound to the checked file identity', async () => {
    const handle = mockRegularFile(Buffer.from('one\ntwo\n'), mockFileStat(8, 6, 21))

    const stats = await collectUntrackedAdditions('/repo', ['stable.ts'])

    expect(stats.get('stable.ts')).toEqual({ added: 2 })
    expect(openMock).toHaveBeenCalledWith(
      '/repo/stable.ts',
      constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK
    )
    expect(handle.readFile).toHaveBeenCalledTimes(1)
    expect(handle.close).toHaveBeenCalledTimes(1)
  })

  it('fails closed when the path no longer identifies the opened descriptor', async () => {
    const openedStat = mockFileStat(4, 7, 31)
    const replacementStat = mockFileStat(4, 7, 32)
    const handle = mockFileHandle(Buffer.from('data'), openedStat)
    lstatMock.mockResolvedValue(replacementStat)
    openMock.mockResolvedValue(handle)

    expect((await collectUntrackedAdditions('/repo', ['replaced.ts'])).get('replaced.ts')).toEqual(
      {}
    )
    expect(handle.readFile).not.toHaveBeenCalled()
    expect(handle.close).toHaveBeenCalledTimes(1)
  })

  it('does not cache a file that changes while its descriptor is being read', async () => {
    const openedStat = mockFileStat(4, 8, 41)
    const completedStat = { ...openedStat, ctimeMs: 9 }
    const handle = mockFileHandle(Buffer.from('data'), openedStat, completedStat)
    lstatMock.mockResolvedValue(openedStat)
    openMock.mockResolvedValue(handle)

    expect((await collectUntrackedAdditions('/repo', ['changing.ts'])).get('changing.ts')).toEqual(
      {}
    )
    expect(handle.readFile).toHaveBeenCalledTimes(1)
    expect(handle.close).toHaveBeenCalledTimes(1)
  })

  it('fails closed when the reported path is replaced during the descriptor read', async () => {
    const openedStat = mockFileStat(4, 9, 51)
    const replacementStat = mockFileStat(4, 9, 52)
    const handle = mockFileHandle(Buffer.from('data'), openedStat)
    lstatMock.mockResolvedValueOnce(openedStat).mockResolvedValueOnce(replacementStat)
    openMock.mockResolvedValue(handle)

    expect((await collectUntrackedAdditions('/repo', ['swapped.ts'])).get('swapped.ts')).toEqual({})
    expect(handle.readFile).toHaveBeenCalledTimes(1)
    expect(handle.close).toHaveBeenCalledTimes(1)
  })
})

describe('untrackedNoFollowReadFlags', () => {
  it('fails closed when the platform cannot guarantee a no-follow nonblocking open', () => {
    expect(untrackedNoFollowReadFlags({ O_RDONLY: 0 })).toBeNull()
    expect(untrackedNoFollowReadFlags({ O_RDONLY: 0, O_NOFOLLOW: 1 })).toBeNull()
  })

  it('combines all required flags when the platform supports them', () => {
    expect(untrackedNoFollowReadFlags({ O_RDONLY: 1, O_NOFOLLOW: 2, O_NONBLOCK: 4 })).toBe(7)
  })
})

describe('applyLineStats', () => {
  it('copies defined counts onto the entry', () => {
    const entry: { added?: number; removed?: number } = {}
    applyLineStats(entry, { added: 5, removed: 2 })
    expect(entry).toEqual({ added: 5, removed: 2 })
  })

  it('leaves the entry untouched for undefined counts or missing stats', () => {
    const entry: { added?: number; removed?: number } = {}
    applyLineStats(entry, { added: undefined, removed: undefined })
    applyLineStats(entry, undefined)
    expect(entry).toEqual({})
  })
})
