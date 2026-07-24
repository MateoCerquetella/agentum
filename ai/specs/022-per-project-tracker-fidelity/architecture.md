# Spec 022 — Architecture notes

- **Spec:** 022-per-project-tracker-fidelity
- **Phase:** Architect (autonomous SDD loop)
- **Date:** 2026-07-17
- **Baseline:** all anchors on `origin/develop` (this `cero` worktree is v0.57.0-era).

## Verdict

**Sound and minimal — proceed to Developer.** All three increments reuse existing
primitives; two are UI-only, one (B) is UI/IPC-only with the server already wired.
No new crates, no new routes, no data-model change, no backend GraphQL change.
System boundaries are respected. One hard precondition on the Developer phase: it
must run in a **fresh `origin/develop`-based worktree**, not here (see §Environment).

## Boundary check (against `ai/context/architecture_principles.md`)

- **Crate boundaries:** all edits land in `crates/agentum-desktop/ui` (React/Vite).
  The only Rust already involved (`commands/shell.rs` `shell_open_url`) is *reused,
  not modified*. `agentum-server` read/binding layer is untouched. ✅
- **One launch path / YOLO / push-streaming / per-session UUID:** not touched by any
  increment. ✅
- **MCP over skills:** N/A (pure client UI). ✅
- **No new invariant risk introduced;** one invariant is actively *repaired*: the
  "no `target='_blank'` in the Tauri webview" rule (increment C).

## Edit map (exact anchors + contracts)

### Increment A — board-card Status (UI-only)

- **Single edit point:** `ProjectBoardCard.tsx` metadata footer (~`:110-160`).
- **Data source (already present):** `row.fieldValuesByFieldId[<statusFieldId>]`,
  where the status/group field id comes from the board view
  (`ProjectBoardView.tsx:66`, `boardColumns(table)` / `board.field`). The card must
  receive the board's group-field id — thread it as a prop from `ProjectBoardView`
  (it already computes columns from that field) rather than re-deriving in the card.
- **Renderers to reuse (do not rebuild):** color dot via `SINGLE_SELECT_HEX` +
  `optionDotColor` (`ProjectBoardView.tsx:22-49`); issue-type chip colors via
  `singleSelectChipColors` (as used at `ProjectCell.tsx:316`); reference cell
  renderers `SingleSelectCell` / Type chip (`ProjectCell.tsx:88-95`, `:314-322`).
- **Extra fields (cheap, same data object):** issue-type (`row.content.issueType`),
  relative "updated X ago" (`row.updatedAt`) — reuse the app's existing relative-time
  formatter (locate the one already used elsewhere in the sidebar/cards; do not add a
  date lib).
- **Contract change:** `ProjectBoardCard` props gain the board group-field id (and,
  if not already passed, the full `fieldValuesByFieldId` is already on the row). No
  type change to `GitHubProjectRow`.

### Increment B — carry the tracker bind through wizard + local IPC

Two drops to close; the server (`routes/worktrees.rs`) already accepts, persists,
and returns `trackerProvider`/`trackerUrl` — **do not touch it**.

1. **`useComposerState.ts` `submitQuick`** (~`:2602`; `createWorktree` call
   `:2681-2703`): compute `const trackerBind = deriveTrackerBindCoords(submitLinkedWorkItem)`
   (`work-item-picker-model.ts:166-180`) and pass `trackerBind?.trackerProvider,
   trackerBind?.trackerUrl` — **mirror the `submit` path verbatim** (`:2470`,
   `:2494-2495`). The store-slice signature is positional
   (`store/slices/worktrees.ts:1014`, params `:1032-1033`) — align `submitQuick`'s
   arg list to `submit`'s order exactly to avoid slot drift.
2. **Local IPC adapter** `tauri/worktrees.ts:16-30` and
   `runtime/server-worktree-client.ts:26-38`: extend the forwarded whitelist to
   include `trackerProvider`/`trackerUrl` (serialized to the camelCase names the
   server `CreateBody` expects: `worktrees.rs:404-406`). Match the remote RPC set
   (`store/slices/worktrees.ts:1106`). While here, also forward the GitLab
   issue/MR fields for parity (they're in the same whitelist gap).
3. **Correct the false comment** at `CreateWorkspaceWizard.tsx:258` ("submitQuick
   persists the tracker bind") — it will finally be true after (1).

- **Contract change:** local adapter type widens by two optional string fields; no
  server or store-slice signature change.

### Increment C — Open-on-GitHub (UI-only)

- **Edit point:** `WorktreeCardMeta.tsx:313` (the "View on GitHub" `MetadataActionIcon`
  usage). Switch from the `href` anchor branch (`:117-163`, dead `target="_blank"`
  ~`:133`) to `onClick={() => api.shell.openUrl(issue.url!)}`.
- **Decision — call site, not the shared helper:** change only the `:313` usage.
  `MetadataActionIcon`'s `href` branch is shared; leave it intact to avoid regressing
  other `href` callers that may rely on true-anchor semantics. (If a later cleanup
  wants to kill the `href` branch entirely, audit all callers first — out of scope
  here.)
- **URL source (already present):** `issue.url` (GitHub `html_url`, required on
  `IssueInfo` at `shared/types.ts:975-981`) — correct for SSH remotes too (web URL,
  not the `git@` clone URL). Keep the existing `{issue.url && …}` guard (`:312`).
- **Canonical caller to imitate:** `GitHubItemDialog.tsx:815`.

## Build order (Developer)

Independent increments; recommended order by blast radius (smallest first):

1. **C** (one call site) — lowest risk, immediate user-visible win.
2. **A** (one component + prop threading) — UI-only, self-contained.
3. **B** (composer + IPC adapter) — most call-site sensitivity (positional args);
   do last with the arg-order guard front-of-mind.

Each increment is one gated harness feature (see spec §Harness wiring).

## Risks & mitigations

| Risk | Mitigation |
| ---- | ---------- |
| Positional-arg drift in `createWorktree` (B) silently mis-slots the bind | Copy `submit`'s exact arg list into `submitQuick`; add a unit asserting the create payload carries `trackerProvider`/`trackerUrl`. |
| Adapter field-name mismatch → `#[serde(default)]` silently drops the bind | Serialize to the exact camelCase `CreateBody` names; assert in the adapter test. |
| Reintroducing `target="_blank"` anywhere (C) | Component test asserts the markup contains no `target="_blank"` and that click calls `api.shell.openUrl` (mirror `SourceControl.hosted-review-header-link.test.tsx:35`). |
| Board card can't find the status field id (A) | Thread it from `ProjectBoardView` (authoritative), render nothing gracefully when a row has no status value (matches table behavior). |
| Stale-base implementation (v0.57.0) | Hard precondition: implement on a fresh `origin/develop` worktree (§Environment). |

## Test / gate strategy

- **Unit (`verify.sh`):** `npm run build --prefix crates/agentum-desktop/ui` (vite,
  not full tsc — `shared/*` is a vite alias); `bunx vitest` for: a `submitQuick`
  create-payload test (bind present), the adapter whitelist test, a `ProjectBoardCard`
  Status-chip render test, and a `WorktreeCardMeta` no-`target="_blank"` /
  `openUrl`-fired test; `cargo build -p agentum-desktop` (should be a no-op compile).
- **Browser QA (`qa.sh`):** the three flows in spec §Harness wiring; the ↗ assertion
  spies `api.shell.openUrl` (external browser open isn't observable in-app).

## Environment precondition (blocks Developer)

- **Fresh worktree:** `git worktree add ../agentum-022-tracker-fidelity -b
  feat/022-tracker-fidelity origin/develop` — do NOT implement in `cero` (v0.57.0;
  every anchor above would be wrong). Memory records this exact trap.
- **Issue-first:** the repo requires a tracking issue before implementation; #360 is
  closed, so a NEW issue must be filed (proposed title/labels in spec §Open
  questions). Autonomous `gh issue create` has been permission-denied before — this
  is the likely **NEEDS-HUMAN** gate before Developer.
