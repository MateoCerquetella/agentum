// Pure, React/DOM-free logic behind the New Workspace issue picker (spec 012
// F1). The UI package ships no jsdom, so the picker's two gradeable behaviors —
// deriving the pickable open-issue list from a fetched Project view, and shaping
// the bind payload from a chosen row — live here where they can be unit-tested
// without mounting the wizard (mirrors `create-workspace-wizard-model.ts`).
//
// Both entry paths (the wizard's step-3 Tracker picker and the Project-board
// "launch" reverse-entry) share `buildBindPayload`, so a board-launched
// workspace drives status exactly like a wizard-picked one.
import type {
  GitHubProjectOwnerType,
  GitHubProjectRow,
  GitHubProjectTable
} from '../../shared/github-project-types'
import type { LinkedWorkItemSummary } from '@/lib/new-workspace'

/** One pickable Project item — an OPEN issue only. The Project item id is kept
 *  for display/debug; the tracker write re-resolves the item from the issue's
 *  content id at write time (spec 010's idempotent `addProjectV2ItemById`), so
 *  we never persist the item id (spec 012 §6 decision). */
export type WorkItemOption = {
  /** The Project item id (`row.id`) — display/debug only, never a write coord. */
  itemId: string
  /** The issue number in its own repo. */
  number: number
  title: string
  /** The issue's canonical URL — the exact `tracker_url` a later transition
   *  parses `owner/repo` + number from (a Project can span repos, so this is
   *  authoritative, not the workspace repo's remote). */
  url: string
  /** `nameWithOwner`, e.g. `owner/repo`, when GitHub returned it. */
  repository: string | null
}

/** The bind coords a picked work item threads onto the new worktree, plus the
 *  composer summary the existing `applyLinkedWorkItem` attach seam consumes. */
export type WorkItemBind = {
  /** The composer's linked-work-item summary (drives the badge + auto-name). */
  summary: LinkedWorkItemSummary
  /** Persisted on the worktree so the session-start reactor / PR poller can
   *  drive the item without a per-event `git remote` lookup. GitHub-only in v1
   *  (the picker sources GitHub Projects issues). */
  trackerProvider: 'github'
  /** The picked issue's canonical URL — the write coord. */
  trackerUrl: string
}

/** A Project row is a pickable work item iff it is an OPEN issue with a usable
 *  number + URL. PRs, draft issues, redacted rows, and closed issues are
 *  excluded (AC 1). GitHub reports issue state as `OPEN`/`CLOSED`; a missing
 *  state defaults to open (issues always carry one — a null is treated as a
 *  transient gap, not a closed item). */
export function isPickableIssueRow(row: GitHubProjectRow): boolean {
  if (row.itemType !== 'ISSUE') return false
  const state = (row.content.state ?? 'OPEN').toUpperCase()
  if (state !== 'OPEN') return false
  return typeof row.content.number === 'number' && Boolean(row.content.url)
}

/**
 * Derive the pickable option list from a fetched Project view. Open issues only,
 * in the view's fetched order, deduped by issue URL (a Project item appears once
 * per view, but a defensive dedupe keeps the picker honest if a view repeats a
 * row). A null/empty table yields `[]` — the honest empty state (AC 3), never a
 * throw.
 */
export function deriveIssueOptions(
  table: GitHubProjectTable | null | undefined
): WorkItemOption[] {
  const rows = table?.rows
  if (!rows || rows.length === 0) return []
  const seen = new Set<string>()
  const out: WorkItemOption[] = []
  for (const row of rows) {
    if (!isPickableIssueRow(row)) continue
    const url = row.content.url as string
    if (seen.has(url)) continue
    seen.add(url)
    out.push({
      itemId: row.id,
      number: row.content.number as number,
      title: row.content.title,
      url,
      repository: row.content.repository
    })
  }
  return out
}

/**
 * Filter the open-issue picker without mutating/reordering its authoritative
 * Project order. Number, title, and repository are all searchable so a large
 * cross-repo Project remains usable; a leading `#` is accepted for the natural
 * issue-number spelling. Selection lives outside this helper, so filtering a
 * selected issue out of view can never silently unlink it.
 */
export function filterWorkItemOptions(
  options: readonly WorkItemOption[],
  query: string
): WorkItemOption[] {
  const normalized = query.trim().toLowerCase().replace(/^#/, '')
  if (!normalized) return [...options]
  return options.filter((option) => {
    const haystack = [String(option.number), option.title, option.repository ?? '']
      .join('\n')
      .toLowerCase()
    return haystack.includes(normalized)
  })
}

/** The minimal Project identity the issue picker needs to list issues: owner +
 *  ownerType + number. Both the per-repo binding and the global `activeProject`
 *  collapse to this shape (`fetchProjectViewTable` reads exactly these). */
export type PickerProjectRef = {
  owner: string
  ownerType: GitHubProjectOwnerType
  number: number
}

/** The identity fields the wizard reads off a per-repo Projects binding (spec
 *  010's `BoardBinding` / `ProjectBindingDto`). `projectOwnerType` is a free
 *  string on the wire (older writers), narrowed at resolution time. */
export type PickerBindingIdentity = {
  projectOwner: string | null
  projectOwnerType: string | null
  projectNumber: number | null
}

/**
 * Resolve which GitHub Project the New Workspace issue picker lists from (spec
 * 011 F2). Precedence, fail-closed:
 *  1. the selected repo's **per-repo binding** wins (each project configures its
 *     own tracker) — but only when it carries a complete identity (owner +
 *     number); a partial/legacy binding is ignored, never half-resolved;
 *  2. else the globally-active Project (spec 012 behavior — zero regression);
 *  3. else `null` — the picker's honest empty state.
 * Pure so it's unit-testable without mounting the wizard. `ownerType` is
 * normalized to `organization` only for an exact match (mirrors
 * `ProjectBindingEditor`'s re-discover normalization), else `user`.
 */
export function resolvePickerProject(input: {
  binding: PickerBindingIdentity | null | undefined
  activeProject: PickerProjectRef | null | undefined
}): PickerProjectRef | null {
  const b = input.binding
  if (b && b.projectOwner && b.projectNumber != null) {
    return {
      owner: b.projectOwner,
      ownerType: b.projectOwnerType === 'organization' ? 'organization' : 'user',
      number: b.projectNumber
    }
  }
  const active = input.activeProject
  if (active) {
    return { owner: active.owner, ownerType: active.ownerType, number: active.number }
  }
  return null
}

/**
 * Shape the bind payload for a chosen work item — the single seam both entry
 * paths use (AC 2). The `summary` flows through `applyLinkedWorkItem`; the
 * `trackerProvider`/`trackerUrl` are persisted on the worktree at create so the
 * lifecycle layer (F2–F4) can move the item.
 */
export function buildBindPayload(option: WorkItemOption): WorkItemBind {
  return {
    summary: {
      type: 'issue',
      number: option.number,
      title: option.title,
      url: option.url
    },
    trackerProvider: 'github',
    trackerUrl: option.url
  }
}

/**
 * Derive the persisted tracker coords from a composer/launch linked item — the
 * single seam BOTH entry paths (wizard submit + Project-board launch) call so a
 * board-launched workspace drives status exactly like a wizard-picked one
 * (architecture §3 consistency note). GitHub issues bind by URL; Linear binds
 * by identifier; a PR/MR-linked or unlinked create binds nothing (fail-closed —
 * no wrong-issue coord, AC 3). Returns `null` when there's nothing to bind.
 */
export function deriveTrackerBindCoords(
  item:
    | { type: 'issue' | 'pr' | 'mr'; url: string; linearIdentifier?: string }
    | null
    | undefined
): { trackerProvider: string; trackerUrl: string } | null {
  if (!item) return null
  if (item.linearIdentifier) {
    return { trackerProvider: 'linear', trackerUrl: item.linearIdentifier }
  }
  if (item.type === 'issue' && item.url.includes('github.com')) {
    return { trackerProvider: 'github', trackerUrl: item.url }
  }
  return null
}
