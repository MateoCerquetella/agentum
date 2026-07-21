// Spec 010 F3: the PURE logic behind the wizard's provision step ("a workspace
// is born ready") and the goal step's template mode. React/DOM/IPC-free so the
// gradeable behaviors are vitest'able without a DOM (the UI package ships no
// jsdom — the workspace-goal-step precedent):
//   - the exact file list the consent-gated commit stages (D8's "lists exactly
//     what will be committed"),
//   - the goal → repo-name seed and template-mode readiness gating (the goal
//     step's Continue in "New repo from template" mode),
//   - the per-step ProvisionReport summary the step renders inline (failures
//     are warnings, never blockers).

import { slugifyGoalName } from './workspace-goal-step'

/**
 * D4: the default template repo — a UI constant, editable in the wizard,
 * never hardcoded deeper down (the server takes whatever the wire says).
 */
export const DEFAULT_TEMPLATE_REPO = 'goempirical/empirical-sdd-ddd-starter'

/**
 * The exact five CONTRACT paths the consent-gated commit stages — the pure
 * twin of the server's `COMMIT_PATHS` (crates/agentum-server/src/provision.rs;
 * keep the two in sync). Branch-agnostic: the commit lands on the workdir's
 * CURRENT branch, reported authoritatively in `ProvisionCommitReport.branch`.
 * Engine-written state (feature_list.json, handoff.md, qa/) is deliberately
 * absent — it stays gitignored and is never committed (spec 010 §6.8).
 */
export function provisionCommitFileList(): readonly string[] {
  return [
    '.agentum-harness/.gitignore',
    '.agentum-harness/AGENTS.md',
    '.agentum-harness/init.sh',
    '.agentum-harness/verify.sh',
    '.agentum-harness/qa.sh'
  ]
}

/** Seed a repo name from the goal (template mode) — the goal step's slugifier. */
export function deriveTemplateRepoName(goal: string): string {
  return slugifyGoalName(goal)
}

/** What template-mode Continue needs before it can create the repo. */
export type TemplateModeInputs = {
  goal: string
  owner: string
  name: string
  templateRepo: string
  directory: string
}

// One path segment on disk (`directory/<name>`) — traversal unrepresentable.
const REPO_NAME_RE = /^[A-Za-z0-9._-]+$/
const TEMPLATE_RE = /^[^\s/]+\/[^\s/]+$/

/**
 * The first unmet template-mode input as a user-facing message, or null when
 * ready — never silent (the goal-step blocker discipline), goal checked first
 * (goal-first ordering holds in both modes).
 */
export function firstTemplateModeBlocker({
  goal,
  owner,
  name,
  templateRepo,
  directory
}: TemplateModeInputs): string | null {
  if (goal.trim().length === 0) return 'Describe your goal to continue.'
  if (owner.trim().length === 0) return 'Pick the GitHub owner for the new repo.'
  const trimmedName = name.trim()
  if (trimmedName.length === 0) return 'Name the new repository.'
  if (!REPO_NAME_RE.test(trimmedName) || trimmedName === '.' || trimmedName === '..') {
    return 'Repository names may only contain letters, digits, ".", "-" and "_".'
  }
  if (!TEMPLATE_RE.test(templateRepo.trim())) return 'Template must look like owner/repo.'
  if (directory.trim().length === 0) return 'Choose the local folder to clone into.'
  return null
}

/** True iff template-mode Continue may fire (the AC-9 template-mode gate). */
export function isTemplateModeReady(inputs: TemplateModeInputs): boolean {
  return firstTemplateModeBlocker(inputs) === null
}

// ─── The ProvisionReport wire shapes + inline summary ───────────────────────
// Field names mirror crates/agentum-server/src/provision.rs (single-word
// fields, so camelCase == snake_case on the wire).

export type ProvisionStepReport = { ok: boolean; changed: boolean; detail: string }

export type ProvisionCommitReport = {
  committed: boolean
  pushed: boolean
  /** The workdir's current branch; empty when the commit step was skipped. */
  branch: string
  error: string | null
}

export type ProvisionReport = {
  labels: ProvisionStepReport
  project: ProvisionStepReport
  binding: ProvisionStepReport
  scaffold: ProvisionStepReport
  commit: ProvisionCommitReport
}

export type ProvisionSummaryLine = {
  id: 'labels' | 'project' | 'binding' | 'scaffold' | 'commit'
  label: string
  /** false renders as a WARNING — provision failures never block creation. */
  ok: boolean
  text: string
}

/**
 * Flatten a ProvisionReport into the five per-step lines the provision step
 * renders inline. The commit line derives its own text: pushed names the
 * branch; a red push keeps `ok: false` + the surfaced error and tells the
 * user to push manually (D8: non-fatal, workspace stays usable).
 */
export function summarizeProvisionReport(report: ProvisionReport): ProvisionSummaryLine[] {
  const step = (
    id: Exclude<ProvisionSummaryLine['id'], 'commit'>,
    label: string,
    r: ProvisionStepReport
  ): ProvisionSummaryLine => ({ id, label, ok: r.ok, text: r.detail })

  const commit = report.commit
  const branch = commit.branch.trim().length > 0 ? commit.branch : 'the current branch'
  let commitOk = true
  let commitText: string
  if (commit.committed && commit.pushed) {
    commitText = `committed and pushed to ${branch}`
  } else if (commit.committed) {
    commitOk = false
    commitText = `committed on ${branch}, but the push failed: ${
      commit.error ?? 'unknown error'
    } — push manually`
  } else if (commit.error) {
    commitOk = false
    commitText = `commit failed: ${commit.error}`
  } else {
    // Consent off OR nothing new to commit — either way, no new commit.
    commitText = 'no new commit'
  }

  return [
    step('labels', 'Status labels', report.labels),
    step('project', 'Project board', report.project),
    step('binding', 'Board binding', report.binding),
    step('scaffold', 'Harness scaffold', report.scaffold),
    { id: 'commit', label: 'Scaffold commit', ok: commitOk, text: commitText }
  ]
}
