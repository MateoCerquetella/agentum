# Architecture — Spec 024 Create Workspace tracker intake

## Current-state findings

1. `TrackerSection` begins with `binding === null`, then
   `resolvePickerProject({ binding, activeProject })` immediately borrows the
   global active Project. Its fetch can therefore paint the wrong repository
   before `getProjectBinding` resolves. A later repository change also leaves the
   previous `table` visible until the next effect completes.
2. The tracker fetch effect returns after `getCachedProjectViewTable`. Cached
   data is fast, but it is never revalidated. The store already owns cache,
   in-flight deduplication, and `{ force: true }`; no new data layer is needed.
3. `GitHubProjectTable` already contains selected-view fields, configured
   single-select option order/color, row field values, position, and project
   identity. The picker currently discards all but issue identity/title.
4. Chat already owns the supported agent/model catalogs. The agent preference is
   `GlobalSettings.chatAgent`; Chat's model preference is the private
   `agentum.chat.model` local-storage key. Draft-body requests already accept an
   optional agent on the Rust side, but the Create Workspace UI sends neither an
   agent nor model and `chat::draft_issue_body` always resolves a default model.

## Decisions

### Repository binding is a closed state machine

Represent binding resolution as `loading | resolved | absent | failed`, keyed by
the selected repository binding target. For a selected git repository, only a
`resolved` repository binding may produce a Project fetch identity. `loading`,
`absent`, and `failed` produce no identity and never consult
`settings.githubProjects.activeProject`. Retain the global fallback only for the
existing no-selected-git-repository path.

The Project request identity is the normalized tuple
`ownerType:ownerLogin:projectNumber`. Visible table state carries that identity.
Changing repository, binding target, or resolved Project synchronously makes a
table with any other identity ineligible to render. Each async binding/fetch
completion also compares its captured target/key with the latest key before
committing state.

### Status means one unambiguous Project field

Add a public field-specific grouping primitive beside the existing group/sort
helpers, then build a pure picker view model over it. The canonical Status field
is the first single-select field named `Status` (case-insensitive) referenced by
the selected view in this precedence: `verticalGroupByFields`, `groupByFields`,
`sortByFields`, then visible `fields`. If none of those references Status, use a
table field named Status only when exactly one unambiguous single-select match
exists. Otherwise there is no Status field.

With a canonical Status field, enumerate its configured options in metadata
order, keep only non-empty groups, and append `No status` last. Option color is
passed through to the row/group label. Apply existing `sortRows` first so the
selected view's configured sorts and GitHub `position` remain stable within a
status. Without a canonical Status field, render the pickable rows in
`sortRows` order without invented groups or labels. Text filtering (trimmed,
case-insensitive title substring or exact/`#`-prefixed issue number) runs before
empty groups/count are derived.

### One cache, cached-first revalidation

For a resolved identity, synchronously seed from
`getCachedProjectViewTable(args)`. If present, retain it while calling
`fetchProjectViewTable(args, { force: true })` once for that entry/mount. If no
cache exists, call the normal fetch and show the cold-loading state. Manual
refresh always uses `{ force: true }`. This deliberately reuses the store's
in-flight/concurrency behavior.

Success atomically replaces the matching table. A background failure retains
the last good matching table and exposes a non-blocking stale/error state; a
cold failure exposes retry. Re-entering step 3 remounts this flow and therefore
revalidates without polling.

### Share Chat preferences without a settings migration

Keep the existing ownership: `GlobalSettings.chatAgent` for agent and
`agentum.chat.model` local storage for model. Extract the model key/read/write
logic from `ChatPage` into `runtime/chat-preferences.ts`, and make both Chat and
Create Workspace consume it. This avoids a second key and a settings schema
migration. Create Workspace reads `settings.chatAgent`, uses
`useDetectedAgents` to limit the existing `CHAT_AGENTS`, and persists changes
through `useAppStore(...updateSettings)`. Claude shows `CHAT_MODELS`; agents
without a catalog show a disabled/default-model label and send no model.

Introduce a shared `DraftLlmChoice { agent: ChatAgentId; model?: string }` at
the runtime/client boundary. Widen `CreateIssueSeams.onGenerate`,
`useComposerState.handleGenerateIssueBody`, and `draftGithubIssueBody` with
optional choice fields. Add optional `model` to Rust `DraftBodyRequest`; pass
both fields to `chat::draft_issue_body`, then call
`resolve_chat_agent(request_agent)` and
`resolve_chat_model(request_model, resolved_agent)`. Omitted fields preserve
current defaults. The route continues to return draft text only; filing remains
behind the existing Create action.

## Data and control flow

```text
selected repo
  -> binding target -> binding state (keyed, late results rejected)
  -> resolved Project identity (no global fallback for git repo)
  -> matching cached table paints
  -> deduplicated forced background refresh
  -> matching response replaces table / failure retains last good table
  -> pure pickable + sorted + status-grouped + filtered view model
  -> existing onPickWorkItem

saved chat agent + shared model preference
  -> detected/supported picker -> DraftLlmChoice
  -> onGenerate -> draftGithubIssueBody JSON
  -> DraftBodyRequest -> resolve agent -> resolve request model -> backend
  -> editable body only -> existing explicit Create files issue
```

## Exact files and seams

- `crates/agentum-desktop/ui/src/components/new-workspace/work-item-picker-model.ts`
  — binding/project resolution contract and pure issue/status/filter view model.
- `crates/agentum-desktop/ui/src/components/new-workspace/create-workspace-wizard-model.ts`
  — add binding-resolution/refreshing/stale presentation states.
- `crates/agentum-desktop/ui/src/shared/github-project-group-sort.ts`
  — expose field-specific grouping while retaining canonical option ordering.
- `crates/agentum-desktop/ui/src/components/new-workspace/CreateWorkspaceWizard.tsx`
  — keyed resolver/fetch lifecycle, tracker header/list states, refresh/search,
  agent/model controls, and widened generate seam.
- `crates/agentum-desktop/ui/src/runtime/chat-preferences.ts` (new) and
  `components/harness/ChatPage.tsx` — single existing model preference owner.
- `crates/agentum-desktop/ui/src/hooks/useComposerState.ts` and
  `runtime/github-issue-client.ts` — carry optional draft choice.
- `crates/agentum-server/src/routes/github.rs` and `routes/chat.rs` — accept and
  resolve optional request model with the selected agent.

## Build order

1. Pure binding, grouping/filtering, and tracker-state models with unit tests.
2. Keyed cached-first TrackerSection UI and interaction tests.
3. Shared preference extraction plus compact agent/model controls.
4. Optional client/server request wiring and Rust resolution tests.
5. Focused suites, production UI build, Rust route tests, and harness artifacts.

## Test strategy

- Vitest pure tests: binding `loading/resolved/absent/failed`; no repo-scoped
  global fallback; canonical Status precedence/ambiguity; option order/color;
  No status last; position stability; pickability; number/title filters.
- React tests: repo A -> repo B immediately hides A; late A binding/table is
  ignored; cached rows remain during forced refresh; cold/background errors,
  retry, count, linked selection, and accessible controls; refresh calls force.
- Preference/UI tests: saved agent/model initialization, detected agents,
  Claude model choice, default-model agent, persistence, and generated choice.
- Client/Rust tests: JSON with explicit and omitted fields; request model reaches
  `resolve_chat_model`; invalid agent/model follows existing error response;
  drafting does not invoke issue creation.
- Gates: focused Vitest, `npm run build --prefix crates/agentum-desktop/ui`,
  relevant `cargo test -p agentum-server --lib`, and `git diff --check`.

## Invariants and bounded risks

- No table renders unless its identity matches the latest resolved repository
  Project. No polling, second cache, tracker mutation, or launch-path changes.
- Store dedupe/concurrency remains authoritative; the component only selects
  normal versus forced fetch behavior.
- Status workflow order comes only from Project metadata, never name heuristics.
- UI catalogs are advisory; server validation remains authoritative and draft
  failure never blocks manual editing or workspace creation.
