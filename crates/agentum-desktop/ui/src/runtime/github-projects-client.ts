// Typed client for the Projects v2 board-binding routes on the embedded
// agentum-server (spec 010 F1). Mirrors `github-issue-client.ts`: same
// loopback endpoint + bearer auth. Wire shapes are faithful to
// `crates/agentum-server/src/routes/github_projects.rs`.
import type { ResolvedMappingDto } from '../lib/github-projects-binding'
import type { ProvisionReport } from '../lib/workspace-provision-step'
import { apiUrl, getServerEndpoint } from './server-endpoint'

async function authHeaders(): Promise<Record<string, string>> {
  const { token } = await getServerEndpoint()
  return token ? { Authorization: `Bearer ${token}` } : {}
}

/** Five option IDs (statusMapping) or five option names (optionNames). */
export type StatusMappingWire = {
  todo: string
  inProgress: string
  readyToTest: string
  done: string
  blocked: string
}

export type ProjectBindingDto = {
  projectId: string
  statusFieldId: string
  statusMapping: StatusMappingWire
  doneClosesIssue: boolean
  projectTitle: string | null
  projectOwner: string | null
  projectOwnerType: string | null
  projectNumber: number | null
  optionNames: StatusMappingWire | null
}

export type DiscoverProjectStatusResponse = {
  projectId: string
  title: string
  statusFieldId: string
  options: { id: string; name: string }[]
  /** `null` = the fuzzy mapper refused (an unmappable core phase) — render
   *  empty selects + `unmappedPhases`, prompt manual completion (D7). */
  resolved: ResolvedMappingDto | null
  unmappedPhases: string[]
}

/**
 * A classified binding failure. `code` carries the server's typed envelope
 * code (`scope_missing` — whose message embeds `gh auth refresh -s project` —
 * `auth_required`, `not_found`, `no_status_field`, `no_github_repo`, …) so the
 * editor can branch; plain-text errors surface with `code: undefined`.
 */
export class GithubProjectsBindingError extends Error {
  readonly code: string | undefined

  constructor(message: string, code?: string) {
    super(message)
    this.name = 'GithubProjectsBindingError'
    this.code = code
  }
}

/** Parse a non-2xx body into the typed error (envelope or plain text). */
async function throwClassified(res: Response, fallback: string): Promise<never> {
  const raw = await res.text().catch(() => '')
  try {
    const parsed = JSON.parse(raw) as { error?: { code?: string; message?: string } | string }
    if (parsed.error && typeof parsed.error === 'object') {
      throw new GithubProjectsBindingError(
        parsed.error.message || `${fallback} (${res.status})`,
        parsed.error.code
      )
    }
    if (typeof parsed.error === 'string' && parsed.error.trim()) {
      throw new GithubProjectsBindingError(parsed.error)
    }
  } catch (err) {
    if (err instanceof GithubProjectsBindingError) {
      throw err
    }
    // Not JSON — the body itself is the message (ApiError::BadRequest is text).
  }
  throw new GithubProjectsBindingError(raw.trim() || `${fallback} (${res.status})`)
}

/**
 * `POST /api/github/project-binding/discover` — one server-side `gh api
 * graphql` call resolving the project's Status field + the fuzzy phase
 * mapping. Doubles as the `project`-scope probe: a missing scope throws a
 * `scope_missing` error whose message IS the remedy.
 */
export async function discoverProjectStatus(input: {
  owner: string
  ownerType: 'user' | 'organization'
  number: number
  /** Abort budget — one bounded `gh` call server-side. */
  timeoutMs?: number
}): Promise<DiscoverProjectStatusResponse> {
  const url = await apiUrl('/api/github/project-binding/discover')
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), input.timeoutMs ?? 35000)
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...(await authHeaders()) },
      body: JSON.stringify({
        owner: input.owner,
        ownerType: input.ownerType,
        number: input.number
      }),
      signal: controller.signal
    })
    if (!res.ok) {
      await throwClassified(res, 'project discovery failed')
    }
    return (await res.json()) as DiscoverProjectStatusResponse
  } finally {
    window.clearTimeout(timeout)
  }
}

/** The binding GET/DELETE query (spec 020 F3). Pure + exported for the wire
 *  pins: `repoId` appended only when present, so a repoId-less call keeps the
 *  pre-020 query byte-identical (the server treats absent as local). */
export function bindingQuery(input: {
  workdir: string
  slug?: string
  repoId?: string
}): URLSearchParams {
  const params = new URLSearchParams({ workdir: input.workdir })
  if (input.slug) {
    params.set('slug', input.slug)
  }
  if (input.repoId) {
    params.set('repoId', input.repoId)
  }
  return params
}

/** `GET /api/github/project-binding` — the repo's stored binding (null = unbound).
 *  `repoId` (spec 020 F3) resolves the slug on the repo's own host — the leg
 *  that makes SSH repos bindable at all. */
export async function getProjectBinding(input: {
  workdir: string
  slug?: string
  repoId?: string
  timeoutMs?: number
}): Promise<{ slug: string; binding: ProjectBindingDto | null }> {
  const params = bindingQuery(input)
  const url = await apiUrl(`/api/github/project-binding?${params.toString()}`)
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), input.timeoutMs ?? 10000)
  try {
    const res = await fetch(url, {
      headers: { ...(await authHeaders()) },
      signal: controller.signal
    })
    if (!res.ok) {
      await throwClassified(res, 'could not read the project binding')
    }
    return (await res.json()) as { slug: string; binding: ProjectBindingDto | null }
  } finally {
    window.clearTimeout(timeout)
  }
}

/**
 * `PUT /api/github/project-binding` — persist the binding. The server
 * re-validates that all five phases carry a non-empty option id (400
 * otherwise); `doneClosesIssue` omitted = ON (the binding-type default).
 */
export async function putProjectBinding(input: {
  workdir: string
  slug?: string
  /** Spec 020 F3: resolve the binding's slug on this repo's own host. */
  repoId?: string
  projectId: string
  statusFieldId: string
  statusMapping: StatusMappingWire
  doneClosesIssue?: boolean
  projectTitle?: string
  projectOwner?: string
  projectOwnerType?: string
  projectNumber?: number
  optionNames?: StatusMappingWire
  timeoutMs?: number
}): Promise<{ slug: string; binding: ProjectBindingDto }> {
  const url = await apiUrl('/api/github/project-binding')
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), input.timeoutMs ?? 10000)
  try {
    const res = await fetch(url, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json', ...(await authHeaders()) },
      body: JSON.stringify({
        workdir: input.workdir,
        ...(input.slug ? { slug: input.slug } : {}),
        ...(input.repoId ? { repoId: input.repoId } : {}),
        projectId: input.projectId,
        statusFieldId: input.statusFieldId,
        statusMapping: input.statusMapping,
        ...(input.doneClosesIssue !== undefined ? { doneClosesIssue: input.doneClosesIssue } : {}),
        ...(input.projectTitle ? { projectTitle: input.projectTitle } : {}),
        ...(input.projectOwner ? { projectOwner: input.projectOwner } : {}),
        ...(input.projectOwnerType ? { projectOwnerType: input.projectOwnerType } : {}),
        ...(input.projectNumber !== undefined ? { projectNumber: input.projectNumber } : {}),
        ...(input.optionNames ? { optionNames: input.optionNames } : {})
      }),
      signal: controller.signal
    })
    if (!res.ok) {
      await throwClassified(res, 'could not save the project binding')
    }
    return (await res.json()) as { slug: string; binding: ProjectBindingDto }
  } finally {
    window.clearTimeout(timeout)
  }
}

/**
 * `POST /api/github/repo-from-template` — spec 010 F3 template mode: create a
 * repo from a template (or adopt an existing one) and clone it under
 * `directory`. Idempotent server-side; a `gh` failure (e.g. the template repo
 * isn't marked "Template repository" on GitHub) surfaces gh's stderr verbatim
 * as the thrown message — render it, never rephrase it.
 */
export async function createRepoFromTemplate(input: {
  owner: string
  name: string
  templateRepo: string
  directory: string
  visibility?: 'private' | 'public'
  /** Create + network clone ride this — wider than the API-call defaults. */
  timeoutMs?: number
}): Promise<{ slug: string; path: string; created: boolean }> {
  const url = await apiUrl('/api/github/repo-from-template')
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), input.timeoutMs ?? 180000)
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...(await authHeaders()) },
      body: JSON.stringify({
        owner: input.owner,
        name: input.name,
        templateRepo: input.templateRepo,
        directory: input.directory,
        ...(input.visibility ? { visibility: input.visibility } : {})
      }),
      signal: controller.signal
    })
    if (!res.ok) {
      await throwClassified(res, 'could not create the repo from the template')
    }
    return (await res.json()) as { slug: string; path: string; created: boolean }
  } finally {
    window.clearTimeout(timeout)
  }
}

/** Link an existing board or create one first (spec 010 F3 / D5). */
export type ProvisionProjectChoice =
  | { owner: string; ownerType: 'user' | 'organization'; number: number }
  | { create: true; owner: string; ownerType: 'user' | 'organization'; title: string }

/**
 * `POST /api/workspace/provision` — the ONE idempotent provisioning ensure
 * (labels, board link-or-create + bind, `.agentum-harness/` scaffold,
 * consent-gated commit+push). Returns the per-step `ProvisionReport`; step
 * failures live INSIDE the report and render as warnings, never blockers.
 * `commitScaffold` is the explicit D8 consent (the UI toggle defaults it ON).
 */
export async function provisionWorkspace(input: {
  workdir: string
  slug?: string
  project?: ProvisionProjectChoice
  statusMapping?: StatusMappingWire
  doneClosesIssue?: boolean
  commitScaffold: boolean
  /** Several bounded `gh`/git calls run in sequence — allow the sum. */
  timeoutMs?: number
}): Promise<ProvisionReport> {
  const url = await apiUrl('/api/workspace/provision')
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), input.timeoutMs ?? 180000)
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...(await authHeaders()) },
      body: JSON.stringify({
        workdir: input.workdir,
        ...(input.slug ? { slug: input.slug } : {}),
        ...(input.project ? { project: input.project } : {}),
        ...(input.statusMapping ? { statusMapping: input.statusMapping } : {}),
        ...(input.doneClosesIssue !== undefined ? { doneClosesIssue: input.doneClosesIssue } : {}),
        commitScaffold: input.commitScaffold
      }),
      signal: controller.signal
    })
    if (!res.ok) {
      await throwClassified(res, 'provisioning failed')
    }
    return (await res.json()) as ProvisionReport
  } finally {
    window.clearTimeout(timeout)
  }
}

/** `DELETE /api/github/project-binding` — unbind (idempotent; 204). `repoId`
 *  (spec 020 F3) resolves the slug on the repo's own host, like the GET. */
export async function deleteProjectBinding(input: {
  workdir: string
  slug?: string
  repoId?: string
  timeoutMs?: number
}): Promise<void> {
  const params = bindingQuery(input)
  const url = await apiUrl(`/api/github/project-binding?${params.toString()}`)
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), input.timeoutMs ?? 10000)
  try {
    const res = await fetch(url, {
      method: 'DELETE',
      headers: { ...(await authHeaders()) },
      signal: controller.signal
    })
    if (!res.ok) {
      await throwClassified(res, 'could not remove the project binding')
    }
  } finally {
    window.clearTimeout(timeout)
  }
}
