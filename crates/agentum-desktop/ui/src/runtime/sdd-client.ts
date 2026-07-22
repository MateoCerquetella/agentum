// Client for the server-owned SDD surface (issue #313): playbook registry,
// button injection, and the per-session SDD loop. Wire shapes mirror
// `crates/agentum-server/src/routes/sdd.rs` (serde snake_case).
import { getJson, postJson } from './server-http'

export type SddPlaybook = {
  /** Canonical id (`sdd-spec`, …) — also the MCP prompt name. */
  name: string
  title: string
  description: string
  body: string
}

export type SddLoopState = {
  active: boolean
  step: number
  maxSteps: number
}

export type SddInjectMode = 'bootstrap' | 'full'

type WireLoopState = { active: boolean; step: number; max_steps: number }

const mapLoop = (w: WireLoopState): SddLoopState => ({
  active: w.active,
  step: w.step,
  maxSteps: w.max_steps
})

export async function listSddPlaybooks(): Promise<SddPlaybook[]> {
  return getJson<SddPlaybook[]>('/api/sdd/playbooks')
}

/**
 * Deliver a playbook to a running session. The server picks the mode:
 * `bootstrap` (a short "fetch it via the agentum_sdd MCP tool" line) for
 * MCP-wired tools, `full` (the whole playbook typed in) otherwise.
 */
export async function injectSddPlaybook(
  sessionId: string,
  playbook: string,
  args?: string
): Promise<{ mode: SddInjectMode; ready: boolean }> {
  return postJson<{ mode: SddInjectMode; ready: boolean }>(`/api/sessions/${sessionId}/sdd/inject`, {
    playbook,
    ...(args ? { args } : {})
  })
}

export async function getSddLoop(sessionId: string): Promise<SddLoopState> {
  return mapLoop(await getJson<WireLoopState>(`/api/sessions/${sessionId}/sdd/loop`))
}

export async function setSddLoop(sessionId: string, active: boolean): Promise<SddLoopState> {
  return mapLoop(await postJson<WireLoopState>(`/api/sessions/${sessionId}/sdd/loop`, { active }))
}
