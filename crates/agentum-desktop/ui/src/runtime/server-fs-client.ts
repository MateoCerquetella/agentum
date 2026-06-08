// Filesystem client over the embedded agentum-server (`/api/fs/list`). This is
// the directory picker the daemon exposes (subdirectories only) — useful for
// "open a workspace" flows. A full file-tree listing (files too) is not yet a
// server route; that's a follow-up if the desktop file explorer moves server-side.
import { getJson, qs } from './server-http'

export type FsEntry = {
  name: string
  /** Absolute resolved path of this entry. */
  path: string
}

export type FsListing = {
  /** Resolved absolute path of the listed directory. */
  path: string
  /** Parent directory, or null at the filesystem root. */
  parent: string | null
  /** Subdirectories, sorted case-insensitively. */
  dirs: FsEntry[]
}

/**
 * `GET /api/fs/list` — list the subdirectories of `path` (defaults to `$HOME`;
 * `~` is expanded). Set `hidden` to include dotfiles.
 */
export function fsListDirs(path?: string, opts?: { hidden?: boolean }): Promise<FsListing> {
  return getJson<FsListing>(`/api/fs/list${qs({ path, show_hidden: opts?.hidden })}`)
}

export type FsFileKind = 'dir' | 'file' | 'symlink'

export type FsFileEntry = {
  name: string
  /** Absolute resolved path. */
  path: string
  kind: FsFileKind
}

export type FsEntries = {
  path: string
  parent: string | null
  /** Dirs first, then files; each case-insensitively sorted. */
  entries: FsFileEntry[]
}

/**
 * `GET /api/fs/entries` — list a directory's dirs AND files (for a server-backed
 * file explorer). Host-aware: pass `hostId` to list over SSH on a remote host
 * (the server runs `find` on the host); omit it for the local machine.
 * `show_hidden` includes dotfiles.
 */
export function fsListEntries(
  path?: string,
  opts?: { hidden?: boolean; hostId?: string }
): Promise<FsEntries> {
  return getJson<FsEntries>(
    `/api/fs/entries${qs({ path, show_hidden: opts?.hidden, host_id: opts?.hostId })}`
  )
}
