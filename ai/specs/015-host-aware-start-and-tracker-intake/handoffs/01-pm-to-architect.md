# Handoff 01 — PM → Architect

- **Spec:** 015-host-aware-start-and-tracker-intake
- **Date:** 2026-07-13
- **From:** PM (autonomous /sdd-orchestrate iteration 1)
- **To:** Architect
- **Artifact:** `ai/specs/015-host-aware-start-and-tracker-intake/spec.md`
  (PM-gated; decisions D1–D6 locked below)

## Gate result

PM gate (`ai/skills/validate_handoff.md`): **PASS** (one-slice = borderline,
ruled in D5). Keystone citations re-verified by the PM against this worktree
(= `origin/develop` `4f98453f`):

- `repos.rs:127-161` `append_repo` — dedupe is literally
  `find(|repo| repo.path == path)` (:134); doc comment says "idempotent by
  path". `AddBody` (:168-184) **already** carries `connection_id` + `host_id`
  with docs. `update()` refuses `path` edits (:215).
- `repos.rs:358-378` — `resolve_repo_host_id` → `load_host_for_repo`,
  `unwrap_or(LOCAL_HOST_ID)` at :372.
- `worktrees.rs:412-443` `create` — host from `load_host_for_repo(&state,
  &body.repo_id)` (:431); the rest of the create path is **already
  host-aware** (`host_runtime::mkdir_p` on the repo's host :440, remote POSIX
  path strings :432-436).
- `composer-host-scoping.ts:89-94` `resolveRepoIdForHost` — the
  `hostScopedRepos[0]?.id ?? ''` fallback, exactly as specced.

## Material PM findings (load-bearing)

1. **F1 is a dedupe-key fix, not an API change.** The wire format
   (`POST /api/repos {path, kind, connectionId, hostId}`) already carries the
   remote identity; only `append_repo`'s find-by-path collapses it. No client
   changes are required for correctness (the badge/scoping bugs downstream are
   all consequences of the collapsed entry).
2. **D2 confirmed by code:** worktree create is already fully host-aware once
   the repo resolves to the right host — remote parent-dir creation and path
   handling exist and are commented as deliberate. No `CreateBody.host_id`
   needed.
3. **No repair path exists by design:** `update()` skips `id`/`path`/`addedAt`,
   so collapsed entries can't be PATCHed into remoteness. Re-add is the remedy;
   the spec's no-migration non-goal stands.
4. **Identity subtlety for the dedupe key:** `Repo` carries BOTH
   `connection_id` (desktop SSH target id; `hostKeyForRepo` buckets by it as
   `ssh:<connectionId>`) and `host_id` (server host). Two desktop connections
   pointing at one server host would be two distinct "hosts" from the UI's
   view. Architect picks the exact key (D6) — UI bucketing by `connection_id`
   suggests (path, connection_id).

## Decisions locked (D1–D6)

- **D1 — F2 chooser = the wizard hop.** Start-work on a multi-host repo routes
  through the existing item→wizard path (`openComposerForItem`,
  `TaskPage.tsx:2345`) pre-seeded with the work item, landing the operator on
  the Host step. An inline board dialog is fallback-only if the hop cannot
  carry Start-work's opinionated fields (linked item, gated-run intent).
  Single-front-door direction of 013 F4.
- **D2 — no worktree-create API change.** Host derives from the picked repo
  (confirmed sound, finding 2). `unwrap_or(LOCAL_HOST_ID)` stays.
- **D3 — Linear gated run deferred.** The Tracker panel files Linear issues
  but the "Start gated run" affordance renders only for GitHub issues, with an
  honest note. No Linear fetch arm in `ensure_spec_and_plan` this spec.
- **D4 — no migration/doctor work.** Collapsed pre-fix entries are fixed by a
  one-time re-add; a `doctor.rs` check is a follow-up ticket, not 015.
- **D5 — one-slice ruling:** three ordered increments stand (008/010-D6
  precedent). F1 is independently shippable and is the root-cause keystone;
  F2 depends on F1; F3 is severable and may ship separately if needed.
- **D6 — dedupe key:** path + remote identity. Exact remote component
  (`connection_id` vs `host_id` vs both) is the architect's call under two
  constraints: local adds (both `None`) keep today's idempotency, and
  re-adding the same remote repo over the same connection returns the existing
  entry (no duplicate explosion, spec AC 2).

## What to blueprint (F1 → F3 order)

1. **F1 — remote-repo-identity.** The D6 key inside `append_repo` + Rust unit
   tests (same path × {local, remote} → two entries; same path × same remote →
   one). Audit every path-keyed lookup over `read_repos()` for
   same-assumption bugs (at minimum `resolve_repo_path`, any find-by-path in
   worktrees/sessions/git routes) — a second entry with the same path must not
   confuse them. Then the UI end-to-end asserts (badge, selection survival,
   remote landing) — expected to fall out with zero UI edits; verify, don't
   assume.
2. **F2 — start-work-asks-where.** Detection: how the board's Start-work
   matches a repo today (`ProjectViewWrapper.tsx:503-526` `matched.id`) and
   what "matches on >1 host" looks like post-F1. Mechanism per D1: pre-seeded
   wizard hop carrying the work item; direct path preserved for the
   single-match case (AC 6). No new create/spawn code (spec AC 8).
3. **F3 — tracker-intent-intake.** The Tracker-tab panel (extend
   `create-issue-intent-model.ts`, reuse draft/create clients); the thin
   Linear-create HTTP seam over `TaskSink` (route shape = architect's call —
   dedicated `POST /api/linear/issues` vs provider param on an existing create
   route; must not fork 013 F3's future wizard use — one seam for both);
   the `startGatedWork` hop with the same precondition set the wizard uses.

## Open architect calls

- D6 exact key (see finding 4).
- F2: what exactly the wizard hop pre-seeds (host preselect? repo preselect
  per host?) and where the multi-host detection lives (pure helper for
  vitest).
- F3: Linear-create seam shape + auth/error surface; where the panel mounts
  inside the Tracker tab (`ProjectHubPage.tsx:253-277` wraps
  `ProjectBindingEditor`) without bloating `ProjectBindingEditor` itself.
- Re-ground all cited UI line numbers before editing — specs 012/013 are
  in-flight on the same wizard surface.

## Expected architect artifact

`ai/specs/015-host-aware-start-and-tracker-intake/architecture.md` —
boundaries, D6 key decision, F2 hop design, F3 seam + panel design, risks,
and a per-increment build/test plan — then
`handoffs/02-architect-to-developer.md`.
