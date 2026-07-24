# Handoff: PM → Architect — Spec 016 (design slice **016a**)

## 1. Summary
PM gate **PASSED** (6/6) on spec 016 (server-side two-way board↔tracker sync;
supersedes 014). Two acceptance criteria were tightened to be tester-observable;
two context notes added for you. **016a** is confirmed as the foundational,
one-screen first slice. Design **016a only** — not the whole parent.

## 2. Completed Work
- Spec 016 drafted + PM-refined; Goal/AC/scope/risks clear.
- PM gate 6/6 pass (AC count is 7 — tolerable for a parent).
- Refinements applied to `spec.md`: fails-loud AC → observable (non-2xx + **no
  board mutation**, stubbed-unreachable test); conflict-policy AC → observable
  (`conflict`/`conflicts[]` field in the sync result); 016a scope-exclusion guard;
  016c two-direction risk note.
- Redundancy check: 011 (one-way create-push), 012 (board-goals view), #58
  (one-way client mirror) — none own server-side two-way; 014 superseded. No live
  conflict.

## 3. Pending Work
- **Your task:** produce `ai/specs/016-board-server-two-way-sync/architecture.md`
  scoped to **016a**: server-side GitHub PULL + durable binding + migration, built
  on #58's data model.
- 016b (GitHub push-back) / 016c (Linear) / 016d (desktop) are future slices —
  design later, not now.

## 4. Important Decisions
- **Build on #58, don't fork it.** Reuse `board_items.external_url` /
  `external_provider` + `upsert_board_item_by_external_url`; add only the stable
  external **id** + last-synced marker that two-way needs.
- **Three collisions to design out** (these closed 014's PRs): (1) migration
  `0022` is taken (`0022_board_external_link`) → use next-free (`0023`) and
  *extend* #58's columns, not a parallel schema; (2) `POST /api/board/sync`
  `{items:[…]}` is #58's shipped contract → put server-pull on a **separate**
  route (`POST /api/board/bindings/{id}/sync`), no regression; (3) `linear.rs`
  module clash → merge into the existing module (relevant in 016c, not 016a).
- **Reference-port** from local branch `feat/014d` (`routes/board_sync.rs`, store
  external-ref helpers, the tested `reconcile_status`, `forge.rs` reuse) — port the
  logic onto current develop; do **NOT** cherry-pick the commits.

## 5. Risks
- **#58 regression** if the pull route touches the shipped `{items}` path → keep
  them separate; plan a regression test (it's an AC now).
- **Migration numbering** — `0021` and `0022` are both taken on remotes; verify
  next-free at build time, don't assume `0023`.
- **main-checkout WIP hazard** — build on a fresh branch off `develop`; never
  `git add -A`; stage only own hunks.
- 016c (later) is two-direction in one slice — heaviest child; may need a
  pull/push split.

## 6. Questions
- None blocking. Open design choice for you: the exact shape of the binding store
  + how the stable external-id / synced-marker extends #58's `external_url`-keyed
  model (reconcile by `external_url` vs. `(provider, external_id)`).

## 7. Recommended Next Step
Architect designs **016a**: name the files/modules to touch (migration, store
methods, the new bindings+sync route, reuse of `forge.rs`), state the boundary
(what changes vs. what stays — especially the untouched #58 path), document the
key tradeoff (extend #58's schema vs. parallel), and give a mitigation for the
#58-regression + migration-numbering risks. Keep mapping/reconcile as pure,
unit-testable functions. Then hand off to Developer.
