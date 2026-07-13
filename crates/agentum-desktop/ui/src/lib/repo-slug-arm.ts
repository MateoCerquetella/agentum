// Pure arm-picker for the repo slug index (spec 020 F2). Deliberately
// import-free so its vitest suite never drags in `@/tauri` or the store.

export type SlugResolutionArm = 'environment-rpc' | 'server' | 'native'

/** Which resolver a repo's slug uses: an active runtime environment keeps the
 *  existing RPC arm (spec 020 non-goal — it wins even for SSH repos, since
 *  the environment owns the whole runtime surface); else an SSH repo
 *  (`connectionId`) resolves via the server's host-aware
 *  `GET /api/repos/{id}/slug` (the local-only native read can never see a
 *  remote checkout); else the local native `gh_repo_slug`. */
export function slugResolutionArm(
  environmentTarget: boolean,
  connectionId: string | null | undefined
): SlugResolutionArm {
  if (environmentTarget) {
    return 'environment-rpc'
  }
  return connectionId ? 'server' : 'native'
}
