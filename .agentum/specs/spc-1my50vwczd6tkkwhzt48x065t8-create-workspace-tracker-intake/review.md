# Review — Spec 024 Create Workspace tracker intake

- **Date:** 2026-07-21
- **Verdict:** SIGN-OFF
- **Blockers:** 0

## Final disposition

The implementation matches the PM-approved spec and architected seams. The
scoped diff fixes repository/project leakage, stale cache behavior, status-aware
issue discovery, and request-scoped drafting LLM selection without adding a
tracker provider, cache, polling loop, filing path, or launch-path exception.

## Acceptance-criteria disposition

- **AC 1 — PASS:** selected git repositories resolve only through their keyed
  binding; Project-keyed render/fetch guards reject old repository data.
- **AC 2 — PASS:** complete pickable open-issue derivation and Project identity
  display are present and tested.
- **AC 3 — PASS:** shared metadata grouping provides option order/color,
  position stability, row chips, and No status last; absent Status stays plain.
- **AC 4 — PASS:** filter/count, accessible row selection, linked styling,
  operational states, retry, and refresh are present.
- **AC 5 — PASS:** matching cache is read during render, then force-revalidated
  through the existing deduplicated store; background failure retains rows.
- **AC 6 — PASS:** re-entry/manual/repository changes can refresh data and late
  binding/fetch results cannot cross identity boundaries.
- **AC 7 — PASS:** supported/detected engine controls and Claude/default-model
  behavior reuse the existing Chat preference owners; stale saved Claude models
  are normalized through `resolveChatModel`.
- **AC 8 — PASS:** optional agent/model traverse the existing client/route/chat
  backend resolution path; omission remains backward-compatible; drafting does
  not file.
- **AC 9 — PASS:** all failure states remain inline and workspace/manual filing
  paths remain available.

## Correctness, race, and error review

- Binding target and Project identity are separate, stable guards; state loaded
  for repo A cannot become eligible after repo B is selected.
- Render-time cache lookup uses the resolved identity, while the effect performs
  refresh only. Forced refresh results are accepted only for the current key.
- Status grouping delegates to the existing Project primitive; picker-specific
  code only selects the canonical field, eligibility, filter, and display model.
- Agent is resolved before model server-side. Unknown/credential/backend errors
  retain the existing actionable response and do not enter the issue-create arm.

## Invariant and security review

- No new endpoint, credential storage, arbitrary model discovery, polling,
  webhook, tracker mutation, agent spawn, YOLO change, or launch path.
- User text continues through JSON payloads and existing backend execution; the
  UI adds no HTML injection or command construction.
- Existing store concurrency/dedupe and existing settings/local-storage owners
  remain authoritative.
- Unrelated SDD-loop/scaffold changes and legacy harness files were preserved.

## Gate evidence

- 5 focused Vitest files / 87 tests — PASS after tester and reviewer repairs.
- GitHub route tests / 10 tests — PASS.
- Chat-agent tests / 11 tests — PASS.
- Final desktop UI production build after reviewer repair — PASS.
- `git diff --check` — PASS.

## Should-fixes / release gates

1. Before release, run the explicitly documented real-desktop QA with two bound
   repositories and live Claude/Codex credentials. This is not a code blocker
   and is not recorded as passed.
2. A future test-infrastructure improvement could add a DOM-capable component
   harness for direct click/render assertions; current pure, route, and build
   evidence is sufficient for this slice.

## Sign-off

**SIGN-OFF.** Spec 024 is done. Merge, release, and environment QA remain
human-gated and were not performed.
