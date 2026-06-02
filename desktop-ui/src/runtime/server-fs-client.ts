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
  return getJson<FsListing>(`/api/fs/list${qs({ path, hidden: opts?.hidden })}`)
}
