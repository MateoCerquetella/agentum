---
phase: 01-goal-cards-planner-slice
plan: "02"
subsystem: planner-config
tags: [config, planner, toml, xdg, security]
dependency_graph:
  requires: []
  provides:
    - agentum_store::paths::planner_config_path
    - agentum_server::planner::PlannerConfig
    - agentum_server::planner::load_planner_config
    - crates/agentum-server/src/planner_prompt.md
  affects:
    - crates/agentum-server/src/lib.rs (mod planner registration)
    - crates/agentum-server/Cargo.toml (toml dep added)
tech_stack:
  added:
    - toml = "0.8" added to agentum-server/Cargo.toml (workspace dep, already pinned)
  patterns:
    - Cow<'static, str> zero-alloc bundled-default path (mirrors rules.rs Cow<'static, [RequiredField]>)
    - include_str! embedded static asset (new convention in this codebase)
    - XDG env isolation test pattern with static Mutex (mirrors routes/profiles.rs)
key_files:
  created:
    - crates/agentum-store/src/paths.rs (planner_config_path fn + test)
    - crates/agentum-server/src/planner.rs (PlannerConfig, load_planner_config, 7 tests)
    - crates/agentum-server/src/planner_prompt.md (bundled default prompt)
  modified:
    - crates/agentum-server/src/lib.rs (pub mod planner added)
    - crates/agentum-server/Cargo.toml (toml dep added)
    - crates/agentum-server/src/routes/board.rs (pre-existing rustfmt fix)
    - crates/agentum-store/src/lib.rs (pre-existing rustfmt fix)
decisions:
  - "planner_prompt.md lives in agentum-server (option b from PATTERNS.md): daemon owns the bundled string; CLI shims never read the file, eliminating cross-crate include_str! fragility risk"
  - "BUNDLED_PROMPT uses Cow<'static, str> so the common (no planner.toml) path allocates nothing"
  - "Path-traversal guard rejects both relative paths and absolute paths containing .. segments (T-02-01)"
  - "No in-memory cache per D-12; file read on every goal-submit via tokio::fs::read_to_string"
metrics:
  duration: "~25 minutes"
  tasks_completed: 2
  files_modified: 7
  completed_date: "2026-05-21"
---

# Phase 01 Plan 02: Planner Config Layer Summary

**One-liner:** XDG planner config loader with `prompt_file > prompt > bundled` precedence, path-traversal guard, and a bundled default prompt that teaches agents to emit cards via `agentum board add-card`.

---

## What Was Built

### Task 1: Path helper + bundled prompt asset

**`agentum_store::paths::planner_config_path()`** — added to `crates/agentum-store/src/paths.rs`. Returns `config_dir()?.join("planner.toml")`, making `$XDG_CONFIG_HOME/agentum/planner.toml` the canonical per-server planner config location per D-12. Sibling to `profiles.toml` and `credentials.toml`.

Unit test `planner_config_path_under_config_dir` uses `XDG_CONFIG_HOME` isolation (serialised by a static `Mutex`) to assert:
- Path ends with `planner.toml`
- Parent equals `config_dir()`

**`crates/agentum-server/src/planner_prompt.md`** — bundled default planner prompt per D-13. Four sections:
1. Role paragraph: you are agentum's planner, goal card already exists, you write 3-7 children
2. CLI surface: full `agentum board add-card` invocation with all flags explained
3. Worked example: 4 cards with a linear `schema → migration → types → test` dependency chain
4. Constraints + `<DONE>` terminator

Contains 5 occurrences of `agentum board add-card` (plan verification threshold: ≥5).

### Task 2: `planner.rs` config loader

**`crates/agentum-server/src/planner.rs`** — new module registered as `pub mod planner` in `lib.rs`.

Public surface:
```rust
pub struct PlannerConfig {
    pub tool: String,           // default "claude"
    pub prompt: Cow<'static, str>,  // zero-alloc on bundled-default path
}

pub async fn load_planner_config() -> Result<PlannerConfig, ApiError>
```

Private wire types: `PlannerFile { planner: Option<PlannerSection> }` and `PlannerSection { tool, prompt_file, prompt }` — all fields optional so an empty `[planner]` header is valid.

Resolution order (D-12):
1. `prompt_file` (absolute path to external file)
2. `prompt` (inline string in planner.toml)
3. Bundled default (`BUNDLED_PROMPT = include_str!("planner_prompt.md")`)

Missing `planner.toml` → bundled default with `tool = "claude"` (zero-config path).

---

## Security: Path-Traversal Guard (T-02-01)

Two checks applied to `prompt_file` before any file read:
1. `!abs.is_absolute()` → `ApiError::BadRequest("planner.prompt_file must be an absolute path: ...")`  
2. `.components().any(|c| matches!(c, ParentDir))` → `ApiError::BadRequest("planner.prompt_file must not contain `..`: ...")`

Both checks have dedicated unit tests. The error message for relative paths explicitly contains `"absolute"` — asserted in `prompt_file_relative_is_rejected`.

---

## Unit Tests (all 8 pass)

| Test | Crate | Verifies |
|------|-------|---------|
| `planner_config_path_under_config_dir` | agentum-store | Path helper returns config_dir/planner.toml |
| `missing_file_returns_bundled_default` | agentum-server | No planner.toml → tool="claude", prompt contains "agentum board add-card" |
| `inline_prompt_overrides_default` | agentum-server | `[planner] tool="codex" prompt="hello world"` → correct tool+prompt |
| `prompt_file_beats_inline_when_both_set` | agentum-server | prompt_file content wins over inline prompt |
| `prompt_file_relative_is_rejected` | agentum-server | `"../etc/passwd"` → BadRequest containing "absolute" |
| `prompt_file_parent_dir_is_rejected` | agentum-server | `"/tmp/foo/../bar"` → BadRequest |
| `prompt_file_missing_is_rejected` | agentum-server | non-existent path → BadRequest containing "does not exist" |
| `invalid_toml_returns_bad_request` | agentum-server | malformed TOML → BadRequest containing "invalid" |

---

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] PlannerConfig missing `#[derive(Debug)]`**
- **Found during:** Task 2 compilation
- **Issue:** `unwrap_err()` requires `Debug` on the `Ok` type; `PlannerConfig` had no derive
- **Fix:** Added `#[derive(Debug)]` to `PlannerConfig`
- **Files modified:** `crates/agentum-server/src/planner.rs`
- **Commit:** e4c61c6

**2. [Rule 1 - Bug] Pre-existing rustfmt failures in board.rs + agentum-store/lib.rs**
- **Found during:** `cargo fmt --all -- --check` verification step
- **Issue:** `board.rs` lines 241-248 and 826, `agentum-store/lib.rs` line 569 had pre-existing fmt issues introduced in a prior commit. Success criteria required `cargo fmt --all -- --check` to pass.
- **Fix:** Reformatted the three blocks to match `cargo fmt` output
- **Files modified:** `crates/agentum-server/src/routes/board.rs`, `crates/agentum-store/src/lib.rs`
- **Commit:** e4c61c6

### Design Decisions Made Inline

**`planner_prompt.md` location:** PATTERNS.md flagged two options for the bundled prompt location. The plan specifies option (b): `crates/agentum-server/src/planner_prompt.md`. This avoids the cross-crate `include_str!` path fragility risk (the file is adjacent to `planner.rs`, so `include_str!("planner_prompt.md")` resolves cleanly at compile time).

**`toml` dep in `agentum-server/Cargo.toml`:** The plan notes `toml` was already a workspace dep but didn't note it was missing from `agentum-server/Cargo.toml`. Added `toml = { workspace = true }` with a comment explaining the rationale.

---

## Known Stubs

None. This plan is purely a config-reader layer with no UI surface. No stub patterns were introduced.

---

## Threat Flags

No new network endpoints, auth paths, or file-access patterns beyond what the plan's `<threat_model>` documents. The `prompt_file` read is guarded by T-02-01 mitigation (absolute path + no `..` checks).

---

## Self-Check

### Files Created/Modified

- [x] `crates/agentum-store/src/paths.rs` — contains `pub fn planner_config_path`
- [x] `crates/agentum-server/src/planner_prompt.md` — non-empty, contains `agentum board add-card`
- [x] `crates/agentum-server/src/planner.rs` — contains `PlannerConfig`, `load_planner_config`, `BUNDLED_PROMPT`
- [x] `crates/agentum-server/src/lib.rs` — contains `pub mod planner`
- [x] `crates/agentum-server/Cargo.toml` — contains `toml = { workspace = true }`

### Commits

- [x] b7512e9: feat(01-02): add planner_config_path() path helper + bundled planner prompt asset
- [x] e4c61c6: feat(01-02): add planner.rs with PlannerConfig + load_planner_config()

## Self-Check: PASSED
