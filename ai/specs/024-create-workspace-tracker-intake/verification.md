# Verification — Spec 024 Create Workspace tracker intake

- **Date:** 2026-07-21
- **Tester verdict:** PASS for executable gates; real-desktop QA is explicitly
  environment-gated and not represented as passed.

## Acceptance-criteria matrix

### AC 1 — Selected-project fidelity

- Automated: `resolvePickerProject` tests prove `loading`, `absent`, and `failed`
  repository binding states never borrow the global active Project.
- Code/race inspection: binding completions compare `targetKey`; table rendering
  and fetch completions compare the normalized Project key, so repo A rows are
  ineligible immediately after switching to repo B.
- Negative evidence: missing binding produces no resolved Project and therefore
  the existing configure-tracker state.

### AC 2 — Current issues render

- Automated: picker tests retain every unique open issue and exclude closed
  issues, PRs, drafts, redacted rows, and invalid number/URL rows.
- Build inspection: the tracker footer renders the resolved Project title plus
  owner/project number.

### AC 3 — Status-aware organization

- Automated: configured single-select Status option order/color and No status
  last are asserted; existing group/sort tests remain green.
- Build inspection: each rendered row includes its group Status chip/color and
  groups use the shared Project metadata primitive. No Status field produces an
  unlabelled position-ordered group.

### AC 4 — Useful issue-picker UI

- Automated: title/exact-number filters, count derivation, and selection data are
  covered by pure tests.
- Build inspection: labelled search and refresh controls, `aria-pressed` linked
  rows, project/count display, resolving/loading/refreshing/stale/empty/error
  copy, retry, and visible linked styling all compile in the production bundle.

### AC 5 — Fast cached-first paint

- Tester-found repair: matching cache lookup now occurs during render after the
  binding resolves, rather than waiting for the fetch effect.
- Code/race inspection: cached data remains visible while the effect calls
  `fetchProjectViewTable(args, { force: true })`; background failure only changes
  status and retains the keyed table; manual refresh is forced.
- Store regression: the existing store remains the single cache/in-flight owner.

### AC 6 — Updates become visible

- Code/race inspection: step re-entry reruns resolution/revalidation; manual
  refresh forces a request; target and Project refs reject late completions.
- Negative evidence: a mismatched cached/state table cannot render because both
  use the full current Project identity.

### AC 7 — Drafting LLM is selectable in context

- Automated: shared Chat model key/default, round-trip persistence, and blocked
  storage behavior pass.
- Build inspection: supported/detected Claude/Codex picker initializes from
  `settings.chatAgent`; Claude exposes `CHAT_MODELS`; Codex says default model;
  agent and model changes use the existing settings/storage owners.

### AC 8 — Selected LLM reaches generation

- Automated: client payload tests cover explicit and omitted agent/model; Rust
  request tests cover both wire shapes; chat-agent tests prove request > config
  > default model precedence and invalid-agent errors.
- Code inspection: composer passes `DraftLlmChoice`; route resolves agent then
  request model; endpoint returns editable body only and never calls filing.

### AC 9 — Optional and non-blocking

- Code inspection: tracker errors only affect inline state; Draft errors continue
  through the existing editable form; workspace and explicit issue Create paths
  are unchanged.
- Negative evidence: no polling, tracker mutation, automatic draft, automatic
  issue filing, or workspace launch-path change exists in the scoped diff.

## Executed gates

- Focused Vitest — PASS: 5 files, 87 tests.
- GitHub route Rust tests — PASS: 10 tests.
- Chat-agent Rust tests — PASS: 11 tests.
- Desktop UI Vite production build after tester repair — PASS (7,238 modules).
- `git diff --check` — PASS.

## Explicit environment/baseline gates

- **Real desktop QA — NOT RUN / ENVIRONMENT-GATED.** This worktree is not the
  running packaged Agentum app, and verification requires two real repository
  Project bindings plus valid Claude/Codex credentials. Required release QA:
  switch repo A/B and capture isolated status groups; observe cached rows during
  refresh; change an issue/status and force refresh; filter/select; draft with
  each available engine/model and confirm no filing before Create.
- Standalone `tsc --noEmit` is repository-baseline red on unresolved legacy
  shared-module paths and unrelated existing type errors. Filtering the output
  after the repair showed no new Spec 024 errors beyond those same baselines.
- Workspace `cargo fmt --check` is repository-baseline red in unrelated executor
  code and pre-existing formatting in large route files; feature hunks pass
  whitespace checks and no unrelated formatter rewrite was made.
