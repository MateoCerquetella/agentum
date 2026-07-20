# Handoff 01 — PM → Architect

- **Spec:** 010-end-to-end-autonomous-flow *(renumbered from 009 at this
  handoff: `ai/specs/009-wiki-project-scoped` ships on sibling branch
  `wiki-remove-it-fomr-the-side`; the 003 pair is the wart we're not repeating)*
- **Date:** 2026-07-06
- **From:** PM (autonomous /sdd-loop iteration 1)
- **To:** Architect
- **Artifact:** `ai/specs/010-end-to-end-autonomous-flow/spec.md` (PM-gated;
  decisions D1–D8 locked; six PM edits applied)

## Gate result

PM gate (`ai/skills/validate_handoff.md`): **PASS** — all nine items green
after edits. 30+ code citations spot-verified against v0.58.3 (`388eaa66`);
two drifts found and fixed in the spec: `TaskSink::create_feature` is at
`task_sink.rs:124` (GitHub arm `:156–198`), and the transition seam has
**four direct call sites spanning six transition points** (the draft said
"five" — matched neither count). The F3 greenfield claim verified true:
zero `gh repo create` / `--template` hits in any crate.

## Decisions locked (see spec "Decisions (PM-locked, 2026-07-06)")

D1 Done closes the issue on BOUND workspaces only, via `done_closes_issue`
(wizard default ON); supersedes 004-D1 there; unbound flows byte-identical
(deliberate narrowing of the PRD's unconditional close — flagged in-spec).
D2 binding lives DAEMON-SIDE; mechanism = architect's call among
github.json+passthrough / sibling file / store table — under the hard
constraint that a Settings label save must never destroy a binding.
D3 human drags are overwritten; no Phase-1 echo machinery of any kind.
D4 default template `goempirical/empirical-sdd-ddd-starter`, configurable.
D5 board CREATE ships alongside link-existing; fallbacks always VISIBLE;
no Status-field option mutation. D6 one-slice ruling: three ordered
increments, F1+F2 self-sufficient, F3 may ship separately. D7 the
bind/mapping UI is ONE shared component with a settings/edit mount first
(wizard-independent); manual per-phase selects are the recovery path for
refused auto-resolution. D8 scaffold commit/push is consent-gated,
default-ON, plain push, no AI-attribution trailer.

## Material PM findings (load-bearing for the blueprint)

1. **github.json clobber hazard (shapes D2).** Desktop
   `github_labels.rs::update_config` (:60–71) round-trips a typed
   `GithubConfig { state_map }` — serde drops unknown keys, so a naïve
   server-written `projects` key would be **erased by the next Settings
   label save**. Its `STORE_LOCK` (:38) is module-local too — a server-side
   writer would be uncoordinated. Any file-based D2 choice needs a desktop
   passthrough field + a preserves-bindings regression test; otherwise pick
   the sibling-file or store option.
2. **Store-backed binding is zero-signature reachable.**
   `apply_tracker_transition` already takes `&Store`;
   `agentum_core::TrackerBinding` (`lib.rs:610`, keyed provider +
   `owner/repo`, board_sync 016a) is the persisted per-repo precedent. The
   real D2 choice is store-vs-file, not file-vs-in-repo.
3. **"Zero call-site edits" verified feasible.** The seam signature carries
   everything the projects arm needs (slug+number already parsed from
   `tracker_url` at :739; `addProjectV2ItemById` is idempotent AND returns
   the item id — ensure and id-fetch are one call). Four direct call sites:
   `drive.rs:388` wrapper (InProgress :129 / ReadyToTest :207 / Done :268),
   `board_goals.rs:605` (Todo), `routes/harness.rs:425` (Todo),
   `routes/mcp.rs:1201` (`agentum_report_status`).
4. **Reopen mechanics need one design call (AC 6).** The seam is stateless;
   blind `gh issue reopen` exits non-zero on an open issue → would spam
   best-effort logs on EVERY InProgress transition of a bound repo. PM
   recommends probe-then-reopen (`gh issue view --json state`); architect
   finalizes.
5. **F3's wizard seam is a typed data table.** `OPTIONAL_WORKSPACE_STEPS`
   (`ui/src/lib/workspace-goal-step.ts:114`), 008-F3 shipped props-only —
   `useComposerState` was never edited; keep that contract. Wrinkle:
   `GoalStepInputs.repoId` is required (:78) but a template-born repo
   doesn't exist at goal time — template mode must produce the repoId
   (create → clone → workdir) before/at `isGoalStepReady`; architect
   decides where that sits.
6. **Fake-gh test pattern:** production knob `gh_bin()`/`AGENTUM_GH_BIN`
   (`task_sink.rs:577`), but the established technique passes
   `program: &str` explicitly (`github_transition_with` :621) — new
   discovery/mutation runners should take `program` the same way.

## What to blueprint (F1 → F3 order)

1. **F1 — bind (foundation).** `BoardBinding` type with the
   constructor invariant (unmapped phase unrepresentable); one
   `gh api graphql` Status-field discovery query (server-side, neutral cwd,
   explicit `program`); pure fuzzy mapper + fallback table (AC 2's four
   cases pin behavior; Backlog/Building/QA/Shipped is the custom fixture);
   the D2 persistence decision FIRST (it shapes everything); bind/read/
   update routes + the `project`-scope probe with the actionable
   `gh auth refresh -s project` error; the shared mapping component with
   its settings/edit mount (D7), reusing `gh_resolve_project_ref` /
   `gh_list_accessible_projects` (desktop, read-only, real) for picking.
2. **F2 — drive (headline value).** The projects arm inside
   `apply_tracker_transition` + `apply_blocked_transition` ONLY (zero
   call-site edits); pure argv builders for `addProjectV2ItemById` (doubles
   as ensure + item-id fetch) and `updateProjectV2ItemFieldValue`; node-id
   resolution from slug+number; optional id cache (correctness must not
   depend on it; ≤ ~10 GraphQL calls per feature run is the ceiling);
   `done_closes_issue` close/reopen (finding 4); fake-gh tests per phase +
   gh-exits-nonzero non-fatal test (AC 7) + existing label tests untouched
   (AC 8).
3. **F3 — provision (born ready).** Template mode (`gh repo create
   --template`, greenfield) and adopt mode converging on ONE idempotent
   ensure: label loop (reuse `gh_label_ensure_argv` :452), project
   link-or-create (D5) + F1 bind, `scaffold_harness` + D8 consent-gated
   commit/push; run-twice test (fake gh + temp git repo: identical state,
   unchanged commit count, AC 10); wizard wiring via the
   `OPTIONAL_WORKSPACE_STEPS` extension seam without editing
   `useComposerState` (finding 5), including template-mode-produces-repoId.

## Open architect calls

- D2 mechanism: github.json `projects` + passthrough vs sibling
  `github_projects.json` vs store table (clobber constraint non-negotiable;
  the seam's `&Store` makes the store option cheap).
- Fuzzy-match internals (normalization/synonyms) — AC 2's four cases are
  the contract.
- Node-id / item-id caching — optional optimization only.
- Reopen: probe-then-reopen vs tolerated blind reopen (PM recommends probe).
- Bind route placement (`routes/github.rs` vs new `routes/github_projects.rs`).
- Wizard structure for template mode vs the goal step's required `repoId`;
  where the D8 consent step renders; provisioning as a fourth optional-step
  entry vs a mode of 'tracker'/'worktree'.
- Where `done_closes_issue`'s default materializes (binding constructor vs
  wizard layer) — default ON either way per D1.

## Expected architect artifact

`ai/specs/010-end-to-end-autonomous-flow/architecture.md` — boundaries, the
D2 persistence call, the seam-internal arm design, GraphQL argv shapes,
tradeoffs, risks, and a per-feature build/test plan (matching prior specs'
`architecture.md` shape) — then `handoffs/02-architect-to-developer.md`.
