---
description: Issue-first workflow — create a documented, labeled GitHub issue, then branch, implement, and open a PR that closes it.
argument-hint: <short description of the change>
allowed-tools: Bash(gh:*), Bash(git:*), Read, Edit, Write, Grep, Glob
---

You are running the **agentum ship workflow**. The iron rule: **no code change
without a tracked issue, and no PR that isn't linked to one.** Follow these
phases in order. Do not skip phase 1 even if the change feels trivial.

Task from the user: **$ARGUMENTS**

---

## Phase 0 — Orient

- `git status` and `git branch --show-current` to see where you are.
- If the working tree holds unrelated uncommitted changes, they may be another
  agent's WIP. **Never `git add -A` / `git checkout` / `git reset` / `git stash`.**
  Stage only the hunks you create.
- Confirm `gh auth status` is good.

## Phase 1 — Create the issue (ALWAYS, first)

1. Classify the change:
   - **type**: one of `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `chore`
   - **area**: one or more of `desktop`, `tui`, `server`, `executor`, `store`,
     `tmux`, `watchdog`, `core`, `harness`, `ci` (from the crate map in CLAUDE.md)
   - **priority**: `p0`–`p3` (default `p2` unless the user says otherwise)
2. Write a well-documented issue body covering: **Summary**, **Motivation/problem**,
   **Proposed approach** (reference concrete crates/files), and an
   **Acceptance criteria** checklist. For bugs, use Steps to reproduce +
   Expected vs actual instead of Proposed approach.
3. Create it with the right labels:
   ```sh
   gh issue create \
     --title "<type>: <concise title>" \
     --label "type/<type>" --label "area/<area>" --label "priority/<p>" \
     --body "$(cat <<'EOF'
   ## Summary
   ...
   ## Motivation
   ...
   ## Proposed approach
   ...
   ## Acceptance criteria
   - [ ] ...
   EOF
   )"
   ```
4. Capture the issue number it prints (e.g. `#123`). Everything below references it.
   **Stop and report if issue creation fails — do not start coding without it.**

## Phase 2 — Worktree (always)

**Always work in a dedicated git worktree — never `git checkout` a new branch in
the shared checkout.** This repo runs many concurrent agents; an in-place
checkout disturbs other sessions' working trees.

- Branch name: `<type>/<kebab-short-desc>` (e.g. `fix/terminal-option-arrows`).
- Create the worktree off an up-to-date base (usually `main`, or the branch
  you're targeting):
  ```sh
  git fetch origin
  git worktree add ../agentum-<kebab-desc> -b <type>/<kebab-desc> origin/main
  cd ../agentum-<kebab-desc>
  ```
  (agentum keeps worktrees as siblings of the main checkout and under
  `.claude/worktrees/` — match whatever the user already uses.)
- If you're already in a dedicated worktree for this work, reuse it.
- When done and merged, clean up: `git worktree remove <path>`.

## Phase 3 — Implement

- Do the actual work. Match surrounding style; comments explain *why*, not *what*.
- Verify per CLAUDE.md before claiming done:
  - Rust: `cargo test -p <crate> --lib` (or `cargo build` for the touched crate).
  - Desktop UI: `npm run build --prefix crates/agentum-desktop/ui`.
- Update CLAUDE.md if you changed architecture, a crate, or a non-obvious gotcha.

## Phase 4 — Commit & push

- Stage only your hunks (`git add <specific paths>`, never `-A`).
- Commit messages end with:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  ```
- `git push -u origin <branch>`.

## Phase 5 — Open the PR (linked to the issue)

```sh
gh pr create \
  --title "<type>: <concise title>" \
  --body "$(cat <<'EOF'
Closes #<issue-number>

## What changed
...

## How it was verified
...
EOF
)"
```

- The body **must** contain `Closes #<issue-number>` so merging auto-closes the issue.
- Report back: the issue URL, the branch, and the PR URL.

## Autonomous (Harness Engine) work

If the work is driven autonomously by the Harness Engine rather than
interactively, the **same issue is the live status board** — keep it updated as
features move `coding → verifying → done/blocked`: post a progress comment on
each transition, check off the matching acceptance-criteria box on a green gate,
and close the issue (or rely on the PR's `Closes #N`) when the final gate is
green. No human is watching the pane, so the issue is the only status surface.
See the "Harness Engine" section of CLAUDE.md for the exact rule.

## Guardrails

- Only commit/push/open-PR when the user asked you to ship (this command is that ask).
- If anything outward-facing is ambiguous (target repo, base branch, labels), ask first.
- One issue per PR. If the task naturally splits, create multiple issues.
