// Pure, React/DOM-free helpers that outlived the goal-first "Create New
// Workspace" step. Spec 013 F4 removed the goal / provision / details phase
// machine (the wizard is the single front door now), so the goal-step's
// seed/reveal state machine is gone. What remains is the two pure transforms
// still consumed elsewhere:
//   - `slugifyGoalName` — a worktree-safe slug from free text (used by the repo
//     provisioning model, `workspace-provision-step.ts`);
//   - `deriveGoalIssueDraft` — intent → issue title+body seed (spec 013 F2's
//     "create issue from intent" reuses it via `deriveIntentTitle`).

/**
 * Derive a concise, worktree-safe workspace name from free-text goal input.
 * Lowercased, punctuation collapsed to word boundaries, first `maxWords` joined
 * with `-`, clamped so the seeded name field stays short. Empty for a
 * blank/whitespace goal so the composer falls back to its own default name.
 */
export function slugifyGoalName(goal: string, maxWords = 6): string {
  const words = goal
    .toLowerCase()
    // Keep only slug-safe characters; everything else becomes a boundary.
    .replace(/[^a-z0-9\s-]+/g, ' ')
    .split(/[\s-]+/)
    .filter(Boolean)
    .slice(0, Math.max(0, maxWords))
  return words
    .join('-')
    .slice(0, 48)
    // A trailing '-' can survive the length clamp mid-word; drop it.
    .replace(/-+$/, '')
}

/**
 * A GitHub-issue draft seeded from a goal/intent. Title = the first line
 * (truncated); body = the whole text. Spec 013 F2 reuses this so a typed
 * description alone produces a titled draft with no retyping.
 */
export type GoalIssueDraft = { title: string; body: string }

const ISSUE_TITLE_MAX = 72

export function deriveGoalIssueDraft(goal: string): GoalIssueDraft {
  const trimmed = goal.trim()
  const firstLine = (trimmed.split(/\r?\n/, 1)[0] ?? '').trim()
  const title =
    firstLine.length > ISSUE_TITLE_MAX
      ? `${firstLine.slice(0, ISSUE_TITLE_MAX - 1).trimEnd()}…`
      : firstLine
  return { title, body: trimmed }
}
