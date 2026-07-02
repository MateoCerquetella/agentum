// Spec 005 F5: pipeline → GitHub status-label overrides. Thin bindings over
// the desktop's flat-arg `github_get_state_map` / `github_set_state_map`
// commands (src/commands/github_labels.rs). The embedded server re-reads
// github.json on every tracker transition, so a save applies on the next
// transition with no restart.
import { call } from './core'

/** Effective pipeline phase → GitHub label-name map (camelCase wire shape). */
export type GithubStateMap = {
  todo: string
  inProgress: string
  readyToTest: string
  done: string
}

export function githubGetStateMap(): Promise<GithubStateMap> {
  return call('github_get_state_map', []) as Promise<GithubStateMap>
}

/**
 * Persist the overrides; a blank/omitted field clears that override (the
 * server falls back to its canonical `status/*` name). Returns the effective
 * map. Keys are the camelCase spellings Tauri derives from the command's flat
 * snake_case params — snake_case keys would silently bind as `None`.
 */
export function githubSetStateMap(map: {
  todo?: string
  inProgress?: string
  readyToTest?: string
  done?: string
}): Promise<GithubStateMap> {
  return call('github_set_state_map', [map]) as Promise<GithubStateMap>
}
