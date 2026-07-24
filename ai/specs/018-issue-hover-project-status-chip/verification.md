# Spec 018 — Verification (Tester)

- **HEAD:** working tree on `issue-hover-card-show-the-bound-github-project-s`
  (ff'd to develop `d31314b3`), 2026-07-14.
- **Verdict:** PASS-WITH-DEFERRALS (0 defects). Local gate green on the UI side;
  the Rust unit gate is CI-deferred (no webkitgtk here — architecture §7).

## Gates run

| Gate | Result |
| ---- | ------ |
| UI build — `bun run build` (vite) | **PASS** — `✓ built in 38.87s`, no import/transpile errors; the new chip, hook, model, and tauri-client method all compile into the bundle. |
| Targeted vitest — `issue-project-status.test.ts` + `WorktreeCardMeta.test.tsx` | **PASS for all new tests** — 13 passed; the 1 failure (`WorktreeCardMeta > includes branch identity…`, the pre-existing `review`/"PR #456" case) reproduces **identically on pristine develop** (stash-all → same `expected 187 to be less than -1`), so it is the known baseline, not a regression. |
| Standalone tsc — `issue-project-status.ts` `--strict` | **PASS** — exit 0. |
| `cargo fmt -p agentum-desktop --check` | **PASS** — exit 0 (also proves the new Rust parses; rustfmt rejects unparseable source). |
| `cargo check -p agentum-desktop` | **ENV-BLOCKED** — fails in `webkit2gtk-sys` / `javascriptcore-rs-sys` build scripts (native `webkit2gtk-4.1` / `javascriptcoregtk-4.1` absent), *before* `agentum-desktop`'s own source compiles. Environmental (project memory), not a code defect. The Rust mapper's 4 `#[cfg(test)]` cases are CI-gated. |

## Acceptance criteria

- **AC 1 (chip renders the bound Status):** code-verified — `IssueProjectStatusChip`
  renders in the badges row (`WorktreeCardMeta.tsx`) with a distinct indigo tone +
  `LayoutGrid` icon (vs `IssueStateBadge` green/purple and `TrackerPhaseChip`
  phase tones). Full runtime render behind a live binding = qa.sh (needs a real
  Projects v2 board + gh auth).
- **AC 2 (silent absence):** covered — pure-model tests assert unbound repo,
  off-project (`status:null`), binding-fetch error, and status-fetch error all →
  `null`; `resolveIssueProjectStatus` is try/catch-wrapped (never throws) and the
  chip returns `null` for a null status. Rust mapper tests assert not-on-project /
  missing-field / no-items / missing-hops → `None`.
- **AC 3 (lazy fetch + per-issue cache):** covered — "does not refetch on a
  second call", "reuses a cached binding across different issues", and the
  effect is `open`-gated (no fetch until the card opens).

## Notes for the reviewer

- The mapper is a pure `fn(&Value, &str, &str) -> Option<String>` — review the
  4 test cases + the two-level match (project id, then status field id).
- Injection: owner/repo/number are bound `$vars`; the query string interpolates
  nothing user-controlled.
- Caches are module-level `Map`s in `IssueProjectStatusChip.tsx` (app-session
  scope) — the pure model takes them injected so tests are deterministic.
