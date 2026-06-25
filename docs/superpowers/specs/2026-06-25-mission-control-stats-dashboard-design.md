# Mission Control becomes the default stats dashboard

**Date:** 2026-06-25
**Branch:** `mission-control`
**Status:** Approved design — ready for implementation plan

## Problem

Two things are wrong with the desktop app's home experience:

1. **The statistics section is dead.** The whole Stats UI is built and
   self-contained (`components/stats/*`), but every backend Tauri command that
   feeds it (`stats_get_summary`, `claude_usage_*`, `codex_usage_*`,
   `open_code_usage_*`) is **stubbed to return zeros**. The command files say so
   verbatim — e.g. `crates/agentum-desktop/src/commands/claude_usage.rs:3` —
   *"Claude usage scanning … isn't ported. Return empty scan state / zeroed
   summary / empty lists so the usage UI shows 'no data'."* So the panes render
   but always show "no data". The section is buried inside Settings → "Stats &
   Usage" (`components/settings/Settings.tsx:1088`).

2. **The app opens to the wrong thing.** The default view is `'terminal'`
   (`store/slices/ui.ts:832`), which — with no workspace selected — renders the
   bland `Landing.tsx` (logo + "select a workspace"). Meanwhile "Mission
   Control" already exists as the `activity` view, but its content is a ~1,860
   line **prototype** agent-activity feed (`components/activity/ActivityPrototypePage.tsx`)
   that the user doesn't use and considers unfinished.

## Goal

- **Phase 1:** Make Claude + Codex usage stats return **real data** (read local
  session logs and aggregate), so both Mission Control *and* the existing
  Settings → Stats & Usage section light up. OpenCode shows a deliberate
  **"Soon"** state.
- **Phase 2:** Turn **Mission Control** (the `activity` view) into the **Usage &
  Stats dashboard** and make it the **default view on every launch**. Delete the
  agent-activity feed prototype and `Landing.tsx`, folding Landing's still-useful
  bits (preflight banner + Add Project / Create Workspace) into Mission Control.
  Add a "Coming soon" section reserved for Agent Orchestration and more.

The two phases are independent and ship as **two PRs, Phase 1 first** (Phase 1
is independently valuable — it fixes the Settings stats too; Phase 2 surfaces the
now-real data on the home dashboard).

## Non-goals (YAGNI)

- **No OpenCode scanner** this round. OpenCode's pane renders a "Soon"
  placeholder; its backend stays stubbed. (User decision.)
- **No new charts / no redesign of the stat panes.** Phase 2 *reuses*
  `<StatsPane/>` unchanged. We are relocating + defaulting, not re-skinning.
- **No removal of `activity-terminal-portal.ts`** or the Terminal.tsx portal
  wiring. With the feed gone the portal simply finds no target and stays inert;
  ripping it out touches `Terminal.tsx` and is a separate cleanup.
- **No change to the `'activity'` view type, its nav entry, the "Mission
  Control" label, the unread badge, or `openActivityPage`/`closeActivityPage`.**
  We swap *only the rendered component*.
- **No change to the agent-status store.** The sidebar's working/idle dots and
  the Mission Control unread badge depend on it; it stays.
- **No server HTTP routes** for the stats. The desktop boots `agentum-server`
  in-process; the desktop commands call `agentum_server::usage::*` functions
  directly (offline, local-only).
- **No `agentum`-vs-`all` scope precision** in v1 — see Design §1.4.

## Existing surface (verified)

### Backend (Phase 1)

| Piece | Location |
| --- | --- |
| Stubbed desktop commands | `crates/agentum-desktop/src/commands/{claude_usage,codex_usage,open_code_usage,stats}.rs` |
| Existing scanner crate | `crates/agentum-server/src/usage.rs` (1065 lines) |
| `walk_jsonl` (recursive `.jsonl` finder) | `usage.rs:113` |
| `parse_iso8601_ms` (ts → ms, also yields y/m/d) | `usage.rs:151` |
| `scan_claude` (5h-window snapshot) + inlined per-record parse | `usage.rs:193`, parse at `:239–291` |
| `scan_codex` (`~/.codex/sessions`, newest-first) | `usage.rs:306`, path at `:312` |
| Per-model pricing + cost | `model_input_price_per_token` `:435`, `estimate_cost_usd` `:454` |
| Claude log paths | `~/.claude/projects`, `~/.claude/transcripts` (`usage.rs:199–200`) |
| Per-record fields read today | `message.usage.{input_tokens,output_tokens,cache_creation_input_tokens}` (`:251–262`), `timestamp` (`:272`), `message.model` (`:288`) |
| **Not read today (must add)** | `message.usage.cache_read_input_tokens`; a **project label** (derive from transcript dir) |

### Frontend contracts (Phase 1 must satisfy)

| Type | Location |
| --- | --- |
| `StatsSummary` | `ui/src/shared/types.ts:2856` |
| `ClaudeUsage*` (ScanState, Summary, DailyPoint, BreakdownRow, SessionRow) | `ui/src/shared/claude-usage-types.ts` |
| `CodexUsage*` | `ui/src/shared/codex-usage-types.ts` |
| `OpenCodeUsage*` | `ui/src/shared/opencode-usage-types.ts` |
| Store slices that call the commands | `ui/src/store/slices/{stats,claude-usage,codex-usage,opencode-usage}.ts` |

Request params: `scope: 'agentum'|'all'`, `range: '7d'|'30d'|'90d'|'all'`,
`kind: 'model'|'project'` (breakdown), `limit` (recent sessions, UI sends 10).

### Frontend (Phase 2)

| Piece | Location |
| --- | --- |
| `<StatsPane/>` — self-contained, no props, own Overview/Claude/Codex/OpenCode tabs | `ui/src/components/stats/StatsPane.tsx:63–170` |
| `<UsageOverviewPane/>` | `ui/src/components/stats/UsageOverviewPane.tsx:246–464` |
| `<OpenCodeUsagePane/>` (disabled `:106–128`, loading `:131–138`, no-data `:232–235`, data `:236–374`) | `ui/src/components/stats/OpenCodeUsagePane.tsx:81–377` |
| Settings mount (keep) | `ui/src/components/settings/Settings.tsx:1088–1095` |
| Default view initializer | `ui/src/store/slices/ui.ts:832` (`activeView: 'terminal'`) |
| View union + `previousViewBeforeActivity` | `ui.ts:439`, `:442` |
| `openActivityPage`/`closeActivityPage` (keep) | `ui.ts:489–490`, impl `:988–1001` |
| `activity` view render (swap) | `ui/src/App.tsx:1745` |
| Landing render branch (remove) | `App.tsx:1747` (`activeView === 'terminal' && !activeWorktreeId`) |
| Lazy imports | `App.tsx:216` (Landing), `:221` (ActivityPrototypePage) |
| Sidebar "Mission Control" nav entry + badge (keep) | `ui/src/components/sidebar/SidebarNav.tsx:177,181,189–201,193–194` |
| Unread badge hook — store-driven, independent of feed | `ui/src/components/activity/useActivityUnreadCount.ts:37–136` |
| Activity terminal portal (keep, inert) | `ui/src/components/activity/activity-terminal-portal.ts`; importers `Terminal.tsx:65–67,231`, `TerminalPaneOverlayLayer.tsx:8–10` |
| `DrillInHeader` home target (still valid) | `ui/src/components/nav/DrillInHeader.tsx:37–38` |
| **Delete** | `ActivityPrototypePage.tsx` (+ `ActivityPrototypePage.test.ts`), `Landing.tsx` |
| Landing bits to relocate | `getPreflightIssues()` `Landing.tsx:26–61`; preflight check `:207–259` (`api.preflight.check()`); Add Project `:299` (`openModal('add-repo')`); Create Workspace `:309` (`openModal('new-workspace-composer', { telemetrySource })`) |

The 7 pure helpers exported by `ActivityPrototypePage.tsx` (`buildActivityEvents`,
`buildAgentPaneThreads`, `getActivityThreadGroup`, `buildActivityThreadGroups`,
`groupActivityThreadsByStatus`, `activityThreadMatchesSearchQuery`,
`activityThreadResponseRenderPreview`) are imported **only** by that file and its
test — both delete together, no other consumers.

## Design

### Phase 1 — Real Claude + Codex usage

All aggregation lives in `agentum-server/src/usage.rs` (new `pub` functions +
camelCase serde structs that mirror the TS contracts). The desktop command files
become thin adapters: parse `scope`/`range`, call `agentum_server::usage::*`,
return the struct. This keeps the heavy logic in the crate that already owns the
scanner and its tests, and matches the "desktop is a thin shell over
agentum-server" architecture.

**1.1 Factor out the per-record parse.** Extract the inlined logic
(`usage.rs:239–291`) into:

```rust
struct UsageRecord { ts_ms: i64, day: String, model: String, project: String,
                     input: u64, output: u64, cache_read: u64, cache_write: u64 }
fn parse_claude_record(line: &str, project: &str) -> Option<UsageRecord>
```

It additionally reads `cache_read_input_tokens` (dropped today) and carries the
`project` label (derived from the transcript's directory under
`~/.claude/projects`). `scan_claude`'s 5h-window path is refactored to consume
`parse_claude_record` so there is one parser, not two.

**1.2 Aggregators (new, generic over a record stream).**

- `claude_usage_summary(scope, range) -> ClaudeUsageSummary` — totals (sessions,
  turns, input/output/cacheRead/cacheWrite, cacheReuseRate, topModel, topProject,
  estimatedCostUsd, hasAnyClaudeData) over the range window.
- `claude_usage_daily(scope, range) -> Vec<ClaudeUsageDailyPoint>` — per calendar
  day buckets (`day` = `YYYY-MM-DD`).
- `claude_usage_breakdown(scope, range, kind) -> Vec<ClaudeUsageBreakdownRow>` —
  grouped by model or project.
- `claude_usage_recent_sessions(scope, range, limit) -> Vec<ClaudeUsageSessionRow>`
  — newest sessions with per-session token totals + duration.
- Codex equivalents over `~/.codex/sessions`, producing the Codex-shaped fields
  (input / cachedInput / output / reasoningOutput / total, `hasInferredPricing`).

A "session" = one transcript file (its id from the filename/path); "turns"/"events"
= record count.

**1.3 Cost.** Replace the single-rate approximation with a proper rate table —
input + output + cache-write + cache-read $/Mtok per model family (opus / sonnet /
haiku; the input/output/cache-write columns are already documented at
`usage.rs:419`). Cost is summed per breakdown so it works for any range, not just
the 5h window. Unknown model ⇒ no cost contribution (and Codex sets
`hasInferredPricing` when rates are guessed). Cost is always labelled an estimate
in the UI (it already is).

**1.4 `scope`.** Ship `scope: 'all'` (all local usage) correctly. `scope:
'agentum'` is **approximated to `'all'` in v1** with a `// TODO` to later filter to
project paths under agentum-managed repos — chosen over shipping a half-built
filter that reports wrong numbers. (Open to revisit; see Decisions.)

**1.5 Enable flag + scan state.** The panes gate data behind an `enabled` toggle
(`claude_usage_set_enabled`). Persist the per-provider flag in a small JSON under
the agentum config dir (`set_enabled` writes, `get_scan_state` reads). **Default
enabled = true** for Claude + Codex so Mission Control shows data on first open;
the user can toggle off. Scanning is synchronous filesystem I/O (fast) so
`isScanning` is transient during `refresh`; `hasAnyClaudeData` reflects whether any
record was found; `lastScanCompletedAt` stamped after a scan.

**1.6 OpenCode.** `open_code_usage_*` stays stubbed; the pane shows "Soon"
(Phase 2 §2.4).

### Phase 2 — Mission Control = the dashboard, opens first

**2.1 New `MissionControlPage.tsx`** (`components/activity/` or a new
`components/mission-control/`), top to bottom:

1. **Header** — title + the relocated **preflight banner** (git/gh missing
   warnings) and **Add Project / Create Workspace** actions from `Landing.tsx`, so
   they stay reachable even with no workspace open. `getPreflightIssues()` moves to
   `lib/`.
2. **`<StatsPane/>`** — rendered unchanged (verified self-contained: own tabs,
   store-sourced, no props, no layout coupling).
3. **"Coming soon" section** — non-interactive cards with a `Soon` badge:
   **Agent Orchestration**, **Scheduled Automations**, **Cost Alerts**.

**2.2 Swap the rendered component.** `App.tsx:1745`:
`{activeView === 'activity' ? <MissionControlPage /> : null}` (was
`<ActivityPrototypePage />`). Update the lazy import at `App.tsx:221`.

**2.3 Default view + no-workspace fallback.**
- `ui.ts:832`: `activeView: 'terminal'` → `'activity'`. (Not persisted, so this
  governs every cold start.)
- `App.tsx:1747`: remove the `→ <Landing/>` branch. The `terminal &&
  !activeWorktreeId` situation falls back to Mission Control so the user is never
  stranded (Mission Control needs no workspace).

**2.4 OpenCode "Soon".** Short-circuit `OpenCodeUsagePane.tsx` to render a small
"Coming soon" card (reuse its existing disabled-card shell, replace the enable
toggle with a `Soon` badge) instead of the enable/no-data states.

**2.5 Deletions.** Remove `ActivityPrototypePage.tsx` + `ActivityPrototypePage.test.ts`
and `Landing.tsx`. Keep `useActivityUnreadCount.ts` (badge) and
`activity-terminal-portal.ts` (inert). Keep all `activity`-view store wiring.

## Data flow

```
Phase 1 (per provider):
  StatsPane tab → store slice (e.g. claude-usage.ts) → api.claudeUsage.getSummary({scope,range})
    → Tauri claude_usage_get_summary  [desktop/src/commands/claude_usage.rs — now a thin adapter]
    → agentum_server::usage::claude_usage_summary(scope, range)
        → walk_jsonl(~/.claude/{projects,transcripts})  [reused]
        → parse_claude_record(line, project) per line     [new, reads cache_read + project]
        → aggregate range/day/model/project + estimate_cost  [new + reused pricing]
    → ClaudeUsageSummary (camelCase) → pane renders real numbers

Phase 2:
  cold start → activeView defaults to 'activity' (ui.ts:832)
    → App.tsx:1745 renders <MissionControlPage/>
        → header (preflight + Add Project / Create Workspace, ex-Landing)
        → <StatsPane/> (real data from Phase 1)
        → "Coming soon" cards
```

## Error handling

- **Backend:** filesystem/parse errors are per-record skips (the scanner already
  does this — bad line ⇒ `continue`). A missing log dir ⇒ `hasAnyClaudeData:
  false` + zeroed summary (the pane's existing "no data" state), never an error
  dialog. `set_enabled` write failure surfaces via `lastScanError`.
- **Frontend:** the panes already own disabled / loading / no-data / data states;
  no new error UI. The relocated preflight banner reuses Landing's existing
  `api.preflight.check()` handling unchanged.
- **Deletion safety:** verified no importers of the deleted files outside their own
  tests; the `activity`-view wiring, unread badge (store-driven), agent-status
  store, and terminal portal are all preserved.

## Testing

**Phase 1 (the gate) — `agentum-server` unit tests with fixture JSONL:**
1. `parse_claude_record` extracts input/output/cache_read/cache_write/model/
   project/ts from a representative record; bad lines yield `None`.
2. Day bucketing: records across 3 days → 3 `ClaudeUsageDailyPoint`s with correct
   per-day sums.
3. Model + project breakdown rows sum correctly; `topModel`/`topProject` pick the
   max.
4. Range filtering: `7d` vs `all` include the right records.
5. Cost: a known token mix → expected estimate via the new rate table; unknown
   model contributes 0.
6. Codex: analogous over a `~/.codex/sessions` fixture.
7. Adapter round-trip: a desktop command returns JSON whose keys match the TS
   contract (camelCase field-name assertion).

**Phase 2 — `agentum-desktop/ui` vitest:**
1. `MissionControlPage` renders the header (Add Project / Create Workspace),
   `StatsPane`, and the three "Soon" cards.
2. Default view: a fresh store has `activeView === 'activity'`.
3. No-workspace fallback: `terminal && !activeWorktreeId` does not render Landing
   (deleted) and does not strand (renders Mission Control).
4. OpenCode pane renders the "Soon" card, not the enable toggle.
5. Regression: the sidebar Mission Control unread badge still computes from the
   store (no dependency on the deleted feed).

Baseline: the existing `agentum-server` lib tests pass; the sidebar/ui vitest
suite passes modulo the known-unrelated `@xterm/addon-ligatures` import failures
(~7 files, pre-existing, documented).

## Risks / notes

- **Cost is an estimate.** Pricing is a coarse per-family table; the UI labels it
  estimated. Acceptable.
- **`agentum` scope is approximate in v1** (== `all`). If the user relies on the
  scope toggle this will read identically for both until the path filter lands.
- **Default-enabled** reads local `~/.claude` / `~/.codex` logs without an explicit
  opt-in. This is local-only, no network; the toggle lets users turn it off. Flag
  if a privacy-default is preferred.
- **`activity-terminal-portal.ts` left inert** is mild dead weight, deliberately
  out of scope to avoid touching `Terminal.tsx`.
- **Contribution workflow:** per `CLAUDE.md`, each phase lands issue-first as a PR
  into `develop`. Phase 1 and Phase 2 are separate issues/PRs.
