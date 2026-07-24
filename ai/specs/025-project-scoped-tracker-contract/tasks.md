# Spec 025 — Implementation tasks

## F1 — Canonical project tracker contract

**Status: IN PROGRESS (2026-07-21).** Implemented the versioned core wire
types, migration `0027_project_tracker_configs.sql`, SQLite CAS
get/put/delete + exact-GitHub-slug lookup, canonical repo-scoped
GET/PUT/PATCH/DELETE routing, exact-repo legacy provider/binding migration,
repoId-aware compatibility delegation for the existing GitHub binding route,
and canonical-row cleanup during repo deletion.

Verified:

- `cargo test -p agentum-store project_tracker --lib` — PASS, 1 passed.
- `git diff --check` — PASS.

Not yet verified/completed:

- `cargo test -p agentum-server project_trackers --lib` compiled
  `agentum-server` and emitted only a pre-existing duplicate-test-attribute
  warning, but the command was interrupted before the test result; server F1
  is therefore not claimed green.
- Route-level migration/CAS/host tests and complete ambiguous legacy-write
  coverage remain required before F1 is complete.

- Add the versioned domain types, SQLite migration, CAS store methods, and
  repo-scoped GET/PUT/PATCH/DELETE route.
- Implement server-side migration from the requested repo's explicit provider
  and exact-slug GitHub binding; preserve legacy files and unknown repo fields.
- Turn existing GitHub binding mutations into compatibility delegates when a
  project can be resolved; reject ambiguous writes.
- Covers AC 1, 2, 7, and 8.

## F2 — One configuration owner in Settings and Tasks

**Status: PENDING.**

- Add the revisioned per-repo UI slice/client with repo/host/generation guards.
- Make Project Settings, Project Hub tracker intake, and Tasks use the same
  configuration selectors/actions and surface provenance/conflicts.
- Submit matching repo-keyed legacy UI hints once through canonical PUT; never
  consume global legacy selections.
- Covers AC 2, 3, 7, 8, and 10.

## F3 — Project-scoped task consumers and preferences

**Status: PENDING.**

- Resolve GitHub/Linear listing, filtering, creation, linking, refresh, and
  provider views from canonical configuration only.
- Move task preferences behind repo-keyed canonical preference PATCHes; retain
  old global fields as read-only migration data.
- Key caches and late-response acceptance by repo + target identity.
- Covers AC 3, 4, 7, and 10.

## F4 — Exact-ticket inheritance and transition isolation

**Status: PENDING.**

- Preserve worktree/feature provider + URL coordinates and thread parent
  `Repo.id` into transition context without changing existing ticket links.
- Resolve GitHub Projects mappings from canonical config, fail closed on target
  mismatch/ambiguity, and remove sole-binding inference.
- Clean only the removed project's config/preferences/cache during deletion.
- Covers AC 5, 6, 8, and 9.

## Final gates

**Status: NOT RUN / NOT ELIGIBLE.** The Developer gate remains open because
F1 is not fully verified and F2–F4 are unimplemented. Vite and full relevant
workspace library gates were not run.

- Focused Rust and Vitest suites described in `architecture.md` are green.
- Vite production build and relevant workspace library tests are green.
- `git diff --check` is clean.
- Real-desktop QA remains a human release-environment gate and must not be
  reported as passed unless the GitHub, Linear, and SSH legs are captured.
