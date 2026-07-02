# Handoff 01 — PM → Architect

- **Spec:** 006-sdd-native-loop-and-rich-issues
- **Date:** 2026-07-02
- **From:** PM (autonomous /sdd-loop iteration 1 — run INLINE by the
  orchestrator: the dispatched sdd-pm subagent died on the account's monthly
  spend limit mid-verification; all cites were re-verified inline before
  gating)
- **To:** Architect
- **Artifact:** `ai/specs/006-sdd-native-loop-and-rich-issues/spec.md`
  (PM-gated; D1–D3 locked)

## Gate result

PM gate: **PASS** after five AC edits. Load-bearing cites verified on this
worktree (develop, HEAD c82e8a75 + merge):

- `routes/github.rs:166-172` `CreateIssueBody` has NO labels field; `:237`
  hardcodes `labels: Vec::new()` — the #232 root cause.
- `task_sink.rs:24-32` `NewFeature.labels` + per-label `gh --label` already
  plumbed (spec 003); chat threads it (`chat.rs:903-906`, `:1031-1036`).
- **Material finding (AC 4 rewritten):** chat issue bodies are COMPOSED
  deterministically by `compose_issue_body` (`chat.rs:973`, called `:1035`)
  from the extraction JSON (`EXTRACT_INSTRUCTIONS` `:866`:
  `{title, summary, tasks[{title, detail, priority}]}`), NOT model-emitted.
  F2 = extend the JSON with optional `problem`/`goal` + reshape the composer,
  with an absent-fields byte-identical pin.
- Spec-013 roles machinery intact: briefs at
  `crates/agentum-server/src/harness_roles/{pm,architect,reviewer}.md` (read:
  role framing + "your job this turn" but thin on the concrete gate
  checklists — diff them against `ai/skills/validate_handoff.md` /
  `write_spec.md` and specify exact deltas, AC 7).
- `spec_md_from_issue` (`harness/types.rs:1042`) appends the control-stripped
  body verbatim — `##` sections survive into the worktree spec (AC 5 holds).
- **F4 root cause found:** `GitHubItemDialog.tsx:892/:938` renders
  `workItem.author ?? 'unknown'`; the composer's just-created snapshot
  (`useComposerState.ts:1455-1470`) carries only `{type, number, title, url}`
  — no author. D3 locks the fix shape (create response + snapshot).
- No repo-label fetch exists in the UI runtime (D2's new-seam decision).

## Decisions locked (full text in spec "Decisions (PM-locked)")

D1 roles setting defaults **ON**, scoped to start-work-planned backlogs only
(read once in the post-plan knob write; manual runs untouched; deliberate,
argued divergence from 005-D3's default-OFF). D2 label picker = new thin
`GET /api/github/labels` + static `type/*`+`priority/*` fallback; no creation.
D3 author fix = create response gains the authenticated login + snapshot
populates it; dialog fallback stays.

## What to blueprint (F1→F4)

1. **F1:** `CreateIssueBody.labels` widening (serde-default, absent =
   byte-identical wire+argv, pinned) + the labels seam
   (`GET /api/github/labels`, same slug resolution as create) + composer
   picker UI + blank-body auto-fill from typed-prompt/note under `## Context`.
2. **F2:** `EXTRACT_INSTRUCTIONS` JSON extension (optional `problem`/`goal`,
   serde-default) + `compose_issue_body` three-section rendering + the
   absent-fields byte-identical pin + an SDD-shaped-body →
   `spec_md_from_issue` round-trip fixture.
3. **F3:** `SDD_ROLES_ENABLED_SETTING` (default TRUE — mind
   `setting_get_bool(.., true)` default handling) + `roles: true` in
   start-work's existing `update_backlog_knobs` write + settings wire
   widening (`HarnessSettings` gains a field — check serde compat with the
   existing camelCase pin test) + brief-content deltas (specify exactly) +
   the verdict-contract character-identical pin. Confirm what
   `run_pre_feature_phases` does when `spec_id` names a spec file that
   start-work just wrote (it should — same workdir).
4. **F4:** `CreateIssueResponse` + author (where does `gh` expose the login
   cheaply — `gh api user --jq .login`? once per create? cache?), snapshot
   population, and the Tasks LIST payload check.

## Constraints for the developer gate (carry into handoff 02)

- Add `cargo clippy --workspace --all-targets -- -D warnings` to every slice
  gate (v0.51.0 tag lesson — it went red post-release on clippy-only errors).
- Tests mod goes at EOF of any file it's added to (items_after_test_module).
- Env-lock tests need `#[allow(clippy::await_holding_lock)]` +
  justification (board_sync precedent).

## Expected architect artifact

`ai/specs/006-sdd-native-loop-and-rich-issues/architecture.md` — boundaries,
seam signatures, corrections, per-feature build/test plan, matching 005's
shape.
