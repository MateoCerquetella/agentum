# Handoff — Developer to Tester

- **Spec:** 024-create-workspace-tracker-intake
- **From:** Developer (autonomous SDD loop step 3)
- **To:** Tester
- **Date:** 2026-07-21
- **Gate:** PASS

## Delivered

- Repository-scoped binding states and Project-keyed table eligibility prevent
  global fallback and stale cross-repository rows.
- Cached-first forced background revalidation, manual force refresh, retained
  stale rows, project identity, search/count, explicit states, and metadata
  Status groups/chips were added to the existing Tracker section.
- Chat's existing model preference is now shared; Create Issue adds detected
  agent plus Claude-model/default-model controls and retains choices.
- Optional agent/model now flow through the existing TypeScript client,
  composer seam, Rust request DTO, agent resolution, and model resolution.

## Acceptance-criteria evidence

- **AC 1–2:** `work-item-picker-model.ts` closed binding state plus
  `CreateWorkspaceWizard.tsx` target/Project keys and source footer.
- **AC 3–4:** shared field grouping and pure view-model tests; UI status chips,
  filter/count, accessible pressed rows, refresh/retry, and linked styling.
- **AC 5–6:** matching cache paints before a forced refresh; response writes
  compare the current Project key; background failure retains last good table.
- **AC 7:** `chat-preferences.ts`, migrated `ChatPage`, detected engine/model UI,
  and preference tests.
- **AC 8:** `DraftLlmChoice`, payload tests, `DraftBodyRequest.model`, and
  `resolve_chat_model(model, &resolved)` with chat-agent precedence tests.
- **AC 9:** tracker/AI failures remain inline and existing manual/create paths
  are untouched.

## Changed files

- `crates/agentum-desktop/ui/src/components/new-workspace/{CreateWorkspaceWizard.tsx,create-workspace-wizard-model.ts,create-workspace-wizard-model.test.ts,work-item-picker-model.ts,work-item-picker-model.test.ts}`
- `crates/agentum-desktop/ui/src/shared/github-project-group-sort.ts`
- `crates/agentum-desktop/ui/src/runtime/{chat-preferences.ts,chat-preferences.test.ts,github-issue-client.ts,github-issue-client.test.ts}`
- `crates/agentum-desktop/ui/src/components/harness/ChatPage.tsx`
- `crates/agentum-desktop/ui/src/hooks/useComposerState.ts`
- `crates/agentum-server/src/routes/{github.rs,chat.rs}`

## Verification

- `vitest run` for group-sort, picker/wizard models, Chat preferences, and
  GitHub issue client — PASS (5 files, 87 tests).
- `cargo test -p agentum-server --lib routes::github::tests` — PASS (10 tests).
- `cargo test -p agentum-server --lib routes::chat_agent::tests` — PASS (11 tests).
- `npm run build --prefix crates/agentum-desktop/ui` — PASS.
- `git diff --check` — PASS.

## Decisions and invariants

- A selected git repository is closed scope: only its resolved binding can
  produce a Project identity.
- No polling or second cache was added; the existing store owns dedupe and
  concurrency.
- Status workflow order/color comes from Project metadata, with No status last.
- Drafting only fills editable text; filing still requires the existing Create.
- Unrelated SDD-loop/scaffold edits and the legacy AutoWiki harness were
  preserved and excluded from feature verification.

## Remaining risks / next action

- Tester should inspect the scoped diff and rerun gates, then record browser QA
  as deferred when a live desktop, two bound Projects, and LLM credentials are
  unavailable. Standalone `tsc` and workspace rustfmt have documented pre-existing
  baselines; neither was weakened or rewritten for this slice.
