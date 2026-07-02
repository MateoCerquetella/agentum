# Handoff 03 — Developer → Tester

- **Spec:** 006-sdd-native-loop-and-rich-issues
- **Date:** 2026-07-02
- **From:** Developer (autonomous /sdd-loop, iterations 3–4, two gated slices)
- **To:** Tester
- **Artifacts:** commits `15365352` (F1+F4), `358347dc` (F2+F3) on branch
  `finish-the-loop`; `tasks.md` carries the checklist, deviations, and the
  stored-turn investigation verdict.

## Gate result (developer phase)

Both slices green, orchestrator-verified (fmt --check; drive.rs diff = the
one C1 Decompose hunk, zero lines touching run_role_gate/decide_gate/
parse_role_verdict/spawn_feature_agent):

- Slice 1 (F1+F4): 522/0 lib, clippy -D warnings green, vite, vitest 5/0.
- Slice 2 (F2+F3): **535/0 lib (5 ignored)**, clippy green, vite 2m04s.
- Pins written FIRST against pre-change code (both verified green pre-edit).

## What the tester must verify (ACs 1–9 + C1)

Independent re-runs first (cargo test lib, clippy -D warnings, vite, the two
vitest files: `issue-context-body.test.ts`, `open-created-workspace.test.ts`),
then per-AC evidence with assertion-body reads:

- AC 1–3 (F1): labels serde-default pin; `parse_label_names`; the `## Context`
  auto-fill exact strings (vitest); labels on both snapshots; the
  `/api/github/labels` 422/400 arms.
- AC 4–5 (F2): the byte-identical pin's literal really is today's output
  (compare `git show 733ff687:crates/agentum-server/src/routes/chat.rs`);
  blank-as-absent; three-section ordering; the AC 5 round-trip through
  `spec_md_from_issue` → `derive_backlog_from_spec`; C4: `problem`/`goal`
  survive preview → DraftPlan → the Confirm-side REBUILD in chat-client.ts
  (deviation 2 — read the rebuild code, this was the silent-drop seam).
- Mandatory item: the fake-gh wire test asserts `--body` non-empty with
  summary + `- [ ]` for a realistic plan; audit the stored-turn verdict
  ("not reproducible") against the code it cites.
- AC 6–8 (F3): `SDD_ROLES_ENABLED_SETTING` default-TRUE reads (OPPOSITE of
  the QA knob — both pinned); `apply_start_work_knobs` only ever SETS roles;
  the wire pin's new exact two-field string; `HarnessSettingsPatch` partial
  PUTs (old one-field body still valid); verdict-contract character pin
  passes against the NEW briefs; brief deltas match architecture §4 verbatim.
- C1: `shared_tracker_provenance` + Decompose's `plan_from_spec_with_tracker`
  arm — the label-trail regression fix.
- AC 9 (F4): author serialization pins; `authenticated_github_login` called
  only after successful create; dialog `?? 'unknown'` untouched; NO
  list-side change (C3).
- Cross-cutting: no `is_public` additions; registry untouched; exactly TWO
  env-mutating tests now exist in the touched surface (the 005 Todo test +
  the new F2 wire test), both under TEST_ENV_LOCK with the allow.

## Deviations to audit (tasks.md has full text)

1. `TaskSink::Github` create arm spawns `gh_bin()` (wire-test enabler,
   default byte-identical). 2. C4 fix landed in the Confirm-side rebuild
   (deeper than architecture said — verify the architecture's "spreads
   verbatim" claim was indeed wrong and the fix is right). 3. The sanctioned
   env-lock test. 4. Provenance tests live in harness.rs::surface_tests.

## NOT verified (qa.sh/staging items — do not fail them)

Settings SDD-role-loop toggle, composer armed-copy switch, real chat
Preview→Confirm filing an SDD body, PhaseStrip on a start-work run, the C1
live regression check (`status/*` flips at InProgress on a roles-on run),
labels/author on a real created issue.

## Expected tester artifact

`verification.md` — per-AC PASS/FAIL with evidence, independent re-run
numbers, deviations audit, Info findings, deferred list (004/005 format).
