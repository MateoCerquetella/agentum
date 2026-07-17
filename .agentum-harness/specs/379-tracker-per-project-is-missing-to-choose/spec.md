# Spec 379 — Per-project tracker choice (GitHub / Linear) + one shared tracker section

> From GitHub issue https://github.com/MateoCerquetella/agentum/issues/379, refined by the
> PM gate. **Amended 2026-07-17** after Mateo's live ask: the tracker *section* in the New
> Issue surface needs a UX/UI overhaul and must be shared with Chat's DraftReview (he
> prefers New Issue's style); the choice is "between github and linear" — the earlier
> "None" option is dropped from this slice. Canonical long-form spec:
> `ai/specs/021-per-project-tracker-choice/spec.md`. Anchors verified on
> `origin/develop` @ v0.78.0 (`bb25a97d`).

## Problem

A project has no say in which tracker its issues live in. The provider is decided three
inconsistent ways: Chat's DraftReview session-local toggle resets to GitHub
(`ChatPage.tsx:136`), the New Issue surface implies the provider from the active Tasks tab
(`TaskPage.tsx`, two bespoke dialogs), and the server's availability heuristic always
prefers GitHub (`task_sink.rs::pick_provider`) while the issue-driven plan path hardcodes
`"github"` (`routes/harness.rs`). Issues and harness status transitions routinely aim at
the wrong tracker; the tracker section is also duplicated (differently) between Chat and
New Issue with zero shared code.

## Persona

Mateo, running client work tracked in Linear and personal OSS tracked in GitHub through
one agentum install. A harness run goes green on the Linear-tracked project but the Linear
ticket sits in "In Progress" (planned against GitHub); filing from Chat lands on GitHub
because the toggle reset — he discovers the stale board at the client stand-up.

## Goal

A project remembers its tracker: a per-repo GitHub/Linear/Auto choice persisted on the
`Repo`, surfaced through ONE shared tracker section (New Issue + Chat DraftReview), and
honored by the server when filing issues, planning goals, and stamping harness features.

## Acceptance criteria

- [ ] `Repo` carries optional `tracker: 'auto' | 'github' | 'linear'` (absent = auto) — `shared/types.ts` + `RepoUpdate` whitelist (`store/slices/repos.ts:77`); persists to `repos.json` via the existing serde-flatten `update()` (zero server schema change, `issueSourcePreference` precedent) and renders again after relaunch.
- [ ] A shared `TrackerSection` component renders in BOTH the New Issue surface (`TaskPage.tsx`, replacing the tab-implied GitHub/Linear dialog split) and Chat's DraftReview filing strip (`ChatPage.tsx:1025-1033`, replacing the ad-hoc `SegButtons` toggle); switching provider swaps provider-specific fields while entered title/body persist.
- [ ] The section initializes from the selected repo's stored `tracker`; a dialog-local override never writes the store; only the explicit "Remember for this project" affordance persists via `updateRepo`.
- [ ] Server pinning: `chat.rs::resolve_provider` (`chat.rs:1861`) and goal planning (`board_goals.rs::create_feature_for_goal` / `TaskSink::select`) honor the pin — a `linear`-pinned project files to Linear even when GitHub is available; `AGENTUM_TASK_SINK` still overrides; pinned-but-unconnected returns the existing typed 422, never a silent fallback.
- [ ] Harness features planned for a pinned project carry the pinned `tracker_provider` (`harness/types.rs:85`) so `task_sink::apply_tracker_transition` dispatches to the matching arm — asserted by a unit test at that seam (no live credentials); the literal `"github"` in the spec-from-issue scaffold (`routes/harness.rs:421`/`438`) is correct (GitHub issue source) and stays.
- [ ] `tracker` unset/`auto` behaves exactly as today (GitHub-first heuristic, existing tests stay green); `npm run build --prefix crates/agentum-desktop/ui` and `cargo test --workspace --lib` pass.

## Out of scope (non-goals)

- Merging Chat's DraftReview and the New Issue dialog into one surface — follow-up spec;
  this slice unifies only the tracker *section*.
- A "None"/Board option — dropped per Mateo's amended ask ("choose between github and
  linear"); Chat issues stay GitHub/Linear only. A harness skip-all pin is a trivial
  follow-up if wanted.
- Per-project issue/team URL fields — `tracker_url` is stamped per-feature from the linked
  issue at plan time; a project-level URL would contradict that flow.
- Global credential/auth configuration stays as-is (`IntegrationsPane.tsx`, the
  `gh`/`linear` Tauri commands); no new providers (Jira, …); no retroactive
  re-transitioning of already-planned runs; no `LinearStateMap` changes.
