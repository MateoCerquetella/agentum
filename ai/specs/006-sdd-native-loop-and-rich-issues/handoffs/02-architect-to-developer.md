# Handoff 02 — Architect → Developer

- **Spec:** 006-sdd-native-loop-and-rich-issues
- **Date:** 2026-07-02
- **From:** Architect (autonomous /sdd-loop iteration 2)
- **To:** Developer
- **Artifact:** `ai/specs/006-sdd-native-loop-and-rich-issues/architecture.md`
  (gate PASSED 5/5; C1's two load-bearing claims independently spot-verified
  by the orchestrator: `drive.rs:846` tracker-less `plan_from_spec`,
  `transition_tracker`'s silent `None`-provider return)

## Gate result

Architect gate: **PASS** — concrete seam signatures per feature; tradeoffs
with rejections (no login cache, GET-full/PUT-patch split, no problem/goal
editor); invariants + the developer-gate constraints carried; named test
plans with pins-written-first; contradictions surfaced as C1–C4. **C1 is
material:** flipping `roles: true` (F3/D1) would silently kill the
spec-004/005 status-label trail because Decompose re-plans tracker-less —
the `shared_tracker_provenance` fix is in F3's scope and its regression check
is in qa.sh.

## NEW mandatory item from Mateo (arrived during this phase)

Mateo reports **chat-created issues also landing with an empty description on
GitHub** ("the title alone is too simple"). Code reading says the path is
coherent (`plan_to_features` → `compose_issue_body` → `gh --body`), so the
developer must, as part of F2:

1. Add a **fake-gh wire test** through `create_github_issues` (or the
   `TaskSink::Github` arm with a composed plan): assert the `--body` argv
   value is non-empty and contains the summary AND at least one `- [ ]` line
   for a realistic plan. This pins the whole chain, not just the composer fn.
2. **Check the stored-turn/draft-review restore path** (PR #219's
   `StoredTurn.filed` + DraftReview flow, `ChatPage.tsx`): verify a plan
   restored from a stored turn still carries `summary`/`tasks` when Confirm
   posts it — a dropped-field restore would produce exactly the empty-body
   symptom Mateo saw. If a bug is found, fix it in the F2 slice and log it in
   tasks.md; if not reproducible, say so explicitly with evidence.

## Build order (F1→F4; F2/F4 independently shippable)

Per architecture.md §7. Write the pins FIRST:
`compose_issue_body_without_problem_goal_is_byte_identical` (F2) and
`role_prompt_verdict_contract_is_character_identical` (F3) — both against the
current code before any edit.

Per-slice gate (v0.51.0 tag lesson — clippy is now part of the gate):
- `cargo fmt --all`
- `cargo test -p agentum-server --lib`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `npm run build --prefix crates/agentum-desktop/ui`
- Tests mod at EOF of any file it's added to; NO env-mutating tests in this
  plan (settings tests use tempdir Stores) — if you reach for env mutation,
  redesign the test.

## Key decisions to not re-litigate (architecture.md §8)

Present-but-blank `problem`/`goal` = absent; the SDD heading is
`## Acceptance criteria` with the `- [ ]` line format as shared code; PUT is
a patch, GET is full (C2); `roles` only ever SET, never written false;
Decompose's provenance fix takes the first stamped feature's pair; login
fetched AFTER the create; two different `setting_get_bool` defaults (QA knob
false, roles knob TRUE).

## Expected developer artifact

Code + tests per architecture.md §2–§5, `tasks.md` per-feature checklist with
deviations + the chat-empty-body investigation result, all gates green,
committed per slice.
