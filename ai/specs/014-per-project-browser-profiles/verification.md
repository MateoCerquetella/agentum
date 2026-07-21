# Verification — Spec 014 (Tester phase, 2026-07-09)

- **Tester:** sdd-tester subagent (autonomous /sdd-loop iteration 6), all gates
  independently re-run — handoff numbers NOT trusted.
- **HEAD:** `18ca3376` (F1 `8dbe0d88` · F2 `a69538b5` · F3 `18ca3376`)
- **Verdict:** **PASS-WITH-DEFERRALS** — no Blockers, no Should-fix, 3 Nits.

## Gates (independently re-run)

| Command | Result |
|---|---|
| `cargo test -p agentum-server --lib` | 574 / 0 / 5 ignored |
| `cargo test -p agentum-desktop --lib` | 78 / 0 / 4 ignored |
| `cargo fmt --all -- --check` | clean |
| clippy (server + desktop libs) | zero warnings |
| vitest `browser-project` + `browser-pane` | 86 / 2 — both fails `webview-registry.test.ts`, **independently confirmed pre-existing** (file last touched `16b4d26f`, before base `d957eefd`; imports nothing the 3 commits touched) |
| `bun run build` (vite) | ✓ 1m10s |

Diff containment: `git diff --stat d957eefd..HEAD` = ONLY the claimed files
(ai/ docs + 8 crate files + 4 UI files).

## Per-AC verdicts

| AC | Verdict | Evidence (abridged — full detail in tester report) |
|---|---|---|
| 1 | PASS | token tests + launch path builds `cdp-browser/project-<repoId>` |
| 2 | PASS-DEFERRED (qa.sh: live login survival) | no-delete/attach-noop/adhoc-parity tests; guard verified held INSIDE `run()`'s future (`routes/cdp_screencast.rs:131,146-148`); remove passes `body.worktree_id` (:456), prune `{repo_id}::{path}` (:640) |
| 3 | PASS-DEFERRED (qa.sh: live routing) | miss→Adhoc-never-Shared tests; all 15 `createBrowserTab` sites re-audited |
| 4 | PASS | 3 native store-token tests incl. legacy-id disjointness |
| 5 | PASS-DEFERRED (qa.sh: end-to-end UI) | only-mine + empty-id-bail tests; route errors propagate; native degradation observable; stub honest `false`; `defineNamespace` wire confirmed |
| 6 | PASS | reap deletes nothing; sweep test (idempotent); `cdp_driver.rs` absent from diff |
| 7 | PASS | all gates green; 13+3 new tests; baseline fails proven pre-existing |

## Deviation audit

All 6 handoff deviations **ACCURATE** (deviation 3 slightly undercounts — 4
tests, not 2, touch `resolve_browser_scope`, but all use raws that resolve
before the tables are consulted; substance holds).

## Sacred surfaces

All CLEAN: `pkill_by_signature` containment (fn byte-identical; all 3 callers
under `state_dir()/cdp-browser`); `canonical_worktree_key` + contract test;
reap deletes nothing; zero polling added (grep over full diff); remote SSH
path byte-identical; `routes/mcp.rs` + `provision.rs` absent from diff; sweep
name-check precedes every deletion; **no new `is_public` entry** (clear route
is behind auth middleware); screencast stays the default surface.

## Defects (3 Nits, 0 Blockers, 0 Should-fix)

1. **Nit** `routes/cdp_screencast.rs:126-129` — attach guard also registered
   on the explicit-`cdpPort` (tunneled/SSH) branch when a `worktreeId` is also
   present; worst case a local project Chromium lingers until quit/boot reap.
   Failure direction safe (never a wrong kill; inside AC 2's tolerance).
2. **Nit** `browser_native.rs:59-68` — native/UI treat ANY bare UUID as a
   project while the server requires a registered repo; a stale-UUID context
   would mis-key the native store vs the (Adhoc) CDP profile. Unreachable via
   current surfaces; same class as the accepted D2 repo-re-add caveat.
3. **Nit** `cdp_browser.rs:262-269` — an adhoc raw key literally starting with
   `project-` (or equal to `shared`) lands in a reserved token namespace: the
   sweep spares it / the adhoc stop could delete `cdp-browser/shared`. No
   current caller produces such keys; the shared profile is ephemeral and
   deletable-by-contract via the equally-authed explicit stop route, so no
   privilege boundary is crossed. Candidate hardening for the reviewer: prefix
   adhoc tokens (e.g. `adhoc-`) or reject reserved names.

## Security-review acknowledgment (background commit scan)

A background security review flagged potential **path traversal** in
`cdp_browser.rs`. Audited: **not possible.** Every filesystem join of
caller-influenced input flows through `BrowserScope::profile_token()` →
`sanitize_worktree_token` (`cdp_browser.rs:262-269` + `:490-514`), which maps
every char outside `[A-Za-z0-9_-]` — including `/`, `\`, `.` — to `-` and
bounds length to a 48-char tail, so no `../` sequence can survive into
`state.join("cdp-browser").join(&token)` (`:726`, `:760`, `:525-529`). The
residual namespace-collision weakness (reserved names, not traversal) is Nit 3
above.

## Deferred to qa.sh / human (per spec Harness wiring)

Live login persistence across tab close + app relaunch; cross-project
isolation in a running session; the clear action end-to-end in the UI
(toast/warning surfacing); plain-workspace routing observed live.
