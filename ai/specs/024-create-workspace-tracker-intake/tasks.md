# Tasks — Spec 024 Create Workspace tracker intake

## Slice 1 — Correct repository identity and pure issue organization — COMPLETE

- Replace nullable binding ambiguity with keyed resolution states and prohibit
  active-Project fallback for a selected git repository.
- Expose field-specific Project grouping and add pure canonical Status,
  pickability, ordering, color, No-status, count, and filter derivation.
- Extend unified tracker display states for resolving, refreshing, stale, empty,
  unavailable, and failed cases.
- Tests: repository transitions/late results at pure seams; grouping precedence,
  ambiguity, option order, stable position, filtering, and exclusions.
- **Acceptance criteria:** AC 1, 2, 3, 4, 6, 9.
- **Gate:** picker/wizard/group-sort Vitest coverage PASS; repository-scoped
  binding states, canonical Status order/color, No status, filtering, and
  project keys are exercised.

## Slice 2 — Cached-first, race-safe TrackerSection UI — COMPLETE

- Key visible state and all async writes by binding target/Project identity.
- Paint matching cache immediately, force one background revalidation, retain
  last good rows on background error, and add force-refresh/retry.
- Render project identity, status-aware groups/labels, issue count, search,
  accessible selection, linked-row styling, and explicit operational states.
- Tests: repo A/B switch and stale response, cached/cold paths, refresh force,
  retained rows on error, re-entry, selection seam, and keyboard labels.
- **Acceptance criteria:** AC 1–6 and 9.
- **Gate:** production Vite build PASS. Table state is keyed by Project identity;
  cached rows force-revalidate, stale responses are rejected, failures retain
  matching rows, and manual refresh uses `{ force: true }`.

## Slice 3 — Shared drafting preference and contextual controls — COMPLETE

- Extract the existing Chat model local-storage preference into one runtime
  helper and migrate `ChatPage` to consume it without changing the key.
- Add supported/detected agent and model/default-model controls beside Draft,
  initialize from Chat preferences, and persist through existing owners.
- Widen the Create Issue generation seam with `DraftLlmChoice` while keeping
  edit/create behavior and failures non-blocking.
- Tests: saved defaults, detection, persistence, Claude models, agent default,
  Draft/Redraft choice, and AI failure with manual editing still enabled.
- **Acceptance criteria:** AC 7 and 9.
- **Gate:** shared preference and client Vitest coverage PASS; Chat and Create
  Workspace use the same model key while agent changes persist through settings.

## Slice 4 — Carry agent/model through the existing draft endpoint — COMPLETE

- Add optional agent/model to the TypeScript request and Rust request DTO.
- Pass request values through `useComposerState`, the GitHub route, and
  `chat::draft_issue_body`; resolve agent then request model with existing
  authoritative helpers.
- Preserve omitted-field defaults and prove draft generation never files.
- Tests: client JSON, explicit/omitted resolution, invalid values/error path,
  and regression coverage for callers that omit both fields.
- **Acceptance criteria:** AC 8 and 9.
- **Gate:** TypeScript payload tests plus Rust GitHub-route and chat-agent tests
  PASS for explicit and omitted fields and request/config/default precedence.

## Final gate — COMPLETE

- Run focused Vitest suites, the desktop UI production build, focused Agentum
  server library tests, harness `verify.sh`, and `git diff --check`.
- Record runtime-only cross-repository/browser evidence in `qa.sh`; do not turn
  missing external credentials into a unit-gate failure.
- **Acceptance criteria:** AC 1–9.
- **Results (2026-07-21):** 87 focused Vitest tests PASS; 10 GitHub route tests
  PASS; 11 chat-agent tests PASS; final Vite production build PASS;
  `git diff --check` PASS. Standalone `tsc` remains baseline-red on the repo's
  unresolved legacy shared-module paths, but filtered output has no new Spec 024
  errors. Workspace `cargo fmt --check` remains baseline-red in unrelated
  `agentum-executor/src/adapters.rs` and pre-existing formatting in large route
  files; no unrelated formatting rewrite was applied.
