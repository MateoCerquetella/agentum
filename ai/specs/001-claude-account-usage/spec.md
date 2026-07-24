# Spec: Claude account usage readout (TUI)

## Goal

A power-user running multiple agents can **glance at the TUI's bottom-left
readout and see their Claude account's estimated spend, token usage, and
how close they are to their plan limit** (color-coded) — so they're warned
before a mid-task cutoff or a surprise bill.

---

## User Value

**In one line:** see Claude spend + plan-limit headroom at a glance, so
neither a bill nor a throttle arrives by surprise.

- **Who:** the power-user running several agents at once across accounts
  (primary). Solo-dev / self-hoster benefit secondarily.
- **Why now:** a surprise bill happened. On a subscription plan, the user
  needs the plan limit "on their brain" — early warning before throttling
  interrupts work.
- **Cost of doing nothing:** more surprise bills and mid-task cutoffs with
  no heads-up; the TUI counts sessions but is blind to actual consumption.

---

## Requirements

- Show a compact usage readout in the **bottom-left** of the TUI (where the
  session count lives today). **Claude only this round.**
- **Plan-limit utilization %** — fetch `GET https://api.anthropic.com/api/oauth/usage`
  with `Authorization: Bearer <claude oauth token>`; take
  **`max(five_hour.utilization, seven_day.utilization)`** (higher of the two
  windows) plus its `resets_at`.
- **Color band** on that utilization: 🟢 `<70%`, 🟡 `70–90%`, 🔴 `>90%`.
- **Estimated $ + tokens, account-wide** — computed from a local scan of
  `~/.claude` transcripts (input/output/cache tokens × public model price),
  shown alongside the band. Labeled **estimated**, not a billed amount.
- **Refresh every 60s by default; interval configurable** via a config key.
- **Fallback:** when `/api/oauth/usage` fails or no token is present, degrade
  gracefully — keep the local transcript data, set `source="scan"`, drop the
  band, and render "plan usage unavailable". *(A `claude`-CLI PTY scrape is a
  deferred `TODO(spec-001)`.)*
- **Graceful failure:** if both sources fail or no token is present, show a
  clear "usage unavailable / stale" state — never a wrong number, never a
  crash.

---

## Acceptance Criteria

- [ ] **Content** — the bottom-left readout **displays**, for the Claude account:
      estimated $ (labeled *est.*), tokens (input + output), and plan-limit
      utilization % = `max(five_hour, seven_day)` with its reset time.
- [ ] **Band** — a color indicator **renders** 🟢 `<70%`, 🟡 `70–90%`, 🔴 `>90%`
      of that utilization.
- [ ] **Refresh** — the readout **refreshes** every 60s by default; the interval
      **reads** from a configurable key with an enforced minimum (e.g. ≥30s).
- [ ] **Fallback** — when `GET /api/oauth/usage` errors or no token is present,
      the readout **degrades gracefully**: `source="scan"`, no band, "plan usage
      unavailable" — never a wrong number. *(Amended 2026-05-29: a `claude`-CLI
      PTY scrape was the original plan; deferred as `TODO(spec-001)` because a
      robust interactive scrape is high-risk. Graceful degradation satisfies the
      intent.)*
- [ ] **Graceful failure** — when both sources fail or no token is present, the
      readout **shows** an explicit "unavailable / stale" state (no wrong number,
      no crash).
- [ ] **Correctness** — the displayed utilization % **matches** what `claude`'s
      "Show plan usage limits" reports for the same account at the same time
      (manual proof).

---

## Dependencies

- No prior specs (this is the first spec).
- Claude OAuth credentials present on the agentum **host**
  (`~/.claude/.credentials.json` / OS keychain / `CLAUDE_CODE_OAUTH_TOKEN`).
- `claude` CLI installed on the host (for the PTY fallback).
- Anthropic `GET /api/oauth/usage` endpoint (undocumented; Claude Code 2.1).
- A Claude **Max/Pro** subscription — session/weekly limit windows only exist
  for subscription plans.

---

## Risks

- **Undocumented endpoint.** `/api/oauth/usage` can change or be removed by
  Anthropic without notice. Mitigation: graceful degradation to "unavailable"
  (never a wrong number); a CLI scrape is a deferred richer fallback.
- **Remote-IP signal.** The daemon runs on a remote VPS; hitting Anthropic
  from an unexpected IP can trip rate-limit signals on the account (Orca's
  own code warns about this). Needs a documented stance / proxy option.
- **Estimated ≠ billed.** The $ figure is notional on a subscription plan;
  must be labeled "estimated" so it isn't mistaken for an invoice.
- **Credential handling.** Reading another tool's OAuth token — must be
  read-only and never logged.
- **Poll cadence.** The usage endpoint is itself rate-limitable; a too-low
  configurable interval could self-throttle. Enforce a sane minimum.

---

## Notes

**Out of scope this round:** other providers (Codex, OpenCode, Gemini),
per-session cost breakdown, usage history/charts, and the tasks-panel noise
fix (tracked as a separate spec).

**Reference implementation — Orca** (`~/Developer/projects/orca`):
- `src/main/rate-limits/claude-fetcher.ts` — `OAUTH_USAGE_URL`, `five_hour` /
  `seven_day` windows, `utilization` → `usedPercent`, `resets_at` mapping.
- `src/main/rate-limits/claude-pty.ts` — CLI/PTY fallback that scrapes
  "Show plan usage limits".
- `src/shared/claude-usage-types.ts` — `ClaudeUsageSummary`
  (input/output/cache tokens, `estimatedCostUsd`) from the transcript scan.

**Future:** mirror the same readout pattern to other providers — Orca already
has `codex-fetcher.ts`, `opencode-go-usage-fetcher.ts`, `gemini-usage-fetcher.ts`
to model.

---

## PM notes

- **Priority:** user-driven (power-user pain, triggered by a real surprise
  bill). It's adjacent to the current PWA milestone — a focused TUI addition,
  not a milestone reprioritization.
- **Refinement:** acceptance criteria consolidated 8 → 6 to fit the PM gate;
  added an enforced minimum on the configurable refresh interval (poll-rate
  risk).
- **Scope:** small enough — single readout, single provider. No split needed.
