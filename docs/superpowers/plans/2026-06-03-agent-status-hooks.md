# Agent Status Hooks (all agents) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the sidebar "working" spinner (and agent-row dot) light up for every agent — not just Claude — by reviving agentum's per-agent status-hook injection that was lost in the Electron→Tauri port.

**Architecture:** On session launch the server already exports `AGENTUM_HOOK_URL` + `AGENTUM_HOOK_TOKEN` into the pane env and POSTs land on `/api/sessions/{id}/hook`. We (1) fix the hook URL to use the server's *actual* bound port (not hardcoded `:8822`), (2) ship one managed hook script that normalizes each agent's native hook payload into `{kind: working|done|permission}` and curls the endpoint, (3) give each `ToolAdapter` a `hook_install()` spec describing how to register that script for its CLI, (4) extend the `/hook` endpoint + renderer to map those kinds into `agentStatusByPaneKey` so `resolveWorktreeStatus` returns `working`. Claude keeps its title-based spinner; every other agent gains a hook-based one.

**Tech Stack:** Rust (axum server `agentum-server`, executor `agentum-executor`), TypeScript/React renderer (`crates/agentum-desktop/ui`), bash (managed hook script), tmux.

**Why phased:** Each agent's hook *contract* differs (event field name, payload via argv vs stdin, config file location). Phase 1 builds the whole pipeline end-to-end and wires the two agents whose contracts are verified (Claude — already partly wired; Codex — verified against `~/.superset/hooks/notify.sh` + `~/.codex/hooks.json`). Phase 2 adds one verified agent at a time using a fixed recipe. Phase 1 is independently shippable and fixes the reported Codex regression.

---

## Background (root cause — confirmed with live evidence)

- The desktop derives `working` from two signals: the agent's OSC terminal **title** (`detectAgentStatusFromTitle`) and explicit **hook** entries in `agentStatusByPaneKey` (`resolveWorktreeStatus` → `hasLiveWorking`).
- Claude Code emits rich status titles (`✳` idle, `. `/braille working) → spinner works on titles alone. Live-captured Codex pane title is just `testi` (the folder name) — **no status signal**.
- `CodexAdapter::launch()` (and every non-Claude adapter) injects **no** hook. `sessions.rs` only special-cases `if session.tool == "claude"`.
- `AGENT_HOOK_TARGETS` (in `shared/agent-hook-types.ts`) lists 12 agents — the framework was designed for all of them, but its installer/receiver lived in the upstream Electron `src/main/agent-hooks/server.ts`, which was never imported (only `ui/` came over). Confirmed: no such path in this repo's git history.
- Bonus bug: `crates/agentum-server/src/routes/sessions.rs:469` hardcodes `http://127.0.0.1:8822`. The embedded desktop server binds an ephemeral port (`serve_embedded_loopback`), so desktop hooks currently can't reach the right server at all.

Reference contract (from the user's working `~/.superset/hooks/notify.sh`):
- **Codex**: payload is JSON in `argv[1]`; event field `"type"`; `task_started`→working, `agent-turn-complete`|`task_complete`→done, `exec_approval_request`|`apply_patch_approval_request`|`request_user_input`→permission.
- **Claude / Droid**: payload via **stdin**; event field `"hook_event_name"`; `UserPromptSubmit`→working, `Stop`→done, `Notification`→permission.
- Session id from `"session_id"` or `"resource_id"`.

---

## File Structure

| File | Responsibility | New/Modify |
| --- | --- | --- |
| `crates/agentum-server/src/lib.rs` | Store the bound loopback `SocketAddr` in `AppState` so handlers can build a correct hook URL. | Modify |
| `crates/agentum-server/src/routes/sessions.rs` | Build hook URL from the bound addr; call `adapter.hook_install()`; apply argv/env/config-dir injection; extend `/hook` to accept `kind` ∈ {working,done,permission,tool_done}. | Modify |
| `crates/agentum-executor/src/adapters.rs` | Add `hook_install()` to `ToolAdapter`; implement for Claude + Codex. | Modify |
| `crates/agentum-executor/src/lib.rs` | Re-export `AgentHookInstall` + the managed-script constant. | Modify |
| `crates/agentum-executor/src/hook_script.rs` | The managed hook script body (the agentum `notify.sh`) + a helper to materialize it to a per-session temp dir. | Create |
| `crates/agentum-desktop/ui/src/hooks/useIpcEvents.ts` (or the WS event handler that owns `agent.hook`) | Map incoming `agent.hook` `{kind}` → `setAgentStatus(paneKey, {state})`. | Modify |
| `crates/agentum-desktop/ui/src/lib/agent-hook-status-map.ts` | Pure `kind → AgentStatusState` mapper (unit-tested). | Create |
| `crates/agentum-desktop/ui/src/lib/agent-hook-status-map.test.ts` | Tests for the mapper. | Create |

> Note: the working tree currently also contains an **unrelated concurrent agent's** changes (status-bar/memory refactor). Stage only the files this plan touches.

---

## Phase 1 — Core pipeline + Claude + Codex

### Task 1: Thread the bound server address into `AppState`

**Files:**
- Modify: `crates/agentum-server/src/lib.rs` (AppState construction + `serve_embedded_loopback` / `serve`)
- Modify: `crates/agentum-server/src/routes/sessions.rs:469`

- [ ] **Step 1: Add a `hook_base` field to `AppState`**

In `crates/agentum-server/src/lib.rs`, add to the `AppState` struct (near `pub addr: SocketAddr` usage):

```rust
/// The loopback base URL agent hooks should POST to, e.g.
/// "http://127.0.0.1:58176". Set once the listener binds so hooks reach
/// THIS server instance (the standalone daemon AND the embedded desktop
/// server share this code; the embedded one binds an ephemeral port).
pub hook_base: std::sync::Arc<std::sync::RwLock<String>>,
```

Initialize it as `Arc::new(RwLock::new("http://127.0.0.1:8822".into()))` wherever `AppState` is built (keep 8822 as the daemon default), and after `let addr = listener.local_addr()?;` in both `serve` and `serve_embedded_loopback`, set:

```rust
*state.hook_base.write().unwrap() = format!("http://127.0.0.1:{}", addr.port());
```

- [ ] **Step 2: Use it in the start handler**

In `crates/agentum-server/src/routes/sessions.rs`, replace line 469:

```rust
let hook_base = state.hook_base.read().unwrap().clone();
let hook_url = format!("{}/api/sessions/{}/hook", hook_base, session.id);
```

- [ ] **Step 3: Build**

Run: `cargo build -p agentum-server`
Expected: compiles clean.

- [ ] **Step 4: Commit**

```bash
git add crates/agentum-server/src/lib.rs crates/agentum-server/src/routes/sessions.rs
git commit -m "fix(server): point agent hook URL at the actual bound port, not hardcoded :8822"
```

---

### Task 2: The managed hook script

**Files:**
- Create: `crates/agentum-executor/src/hook_script.rs`
- Modify: `crates/agentum-executor/src/lib.rs` (add `mod hook_script; pub use hook_script::*;`)
- Test: `crates/agentum-executor/src/hook_script.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

In `crates/agentum-executor/src/hook_script.rs`:

```rust
//! The managed agent status hook: a single POSIX-sh script, installed per
//! agent, that normalizes each CLI's native hook payload into
//! `{kind: working|done|permission}` and POSTs it to $AGENTUM_HOOK_URL.
//! Modeled on the user's proven ~/.superset/hooks/notify.sh contract.

/// The script body. `$1` carries Codex's argv JSON; Claude/Droid pipe stdin.
/// Event field is `"type"` (Codex) or `"hook_event_name"` (Claude family).
pub const HOOK_SCRIPT: &str = include_str!("hook_script.sh");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_maps_codex_and_claude_events() {
        // working
        assert!(HOOK_SCRIPT.contains("task_started"));
        assert!(HOOK_SCRIPT.contains("UserPromptSubmit"));
        // done
        assert!(HOOK_SCRIPT.contains("agent-turn-complete"));
        assert!(HOOK_SCRIPT.contains("\"Stop\""));
        // posts kind + token to the env-provided URL
        assert!(HOOK_SCRIPT.contains("$AGENTUM_HOOK_URL"));
        assert!(HOOK_SCRIPT.contains("X-Agentum-Hook-Token"));
        assert!(HOOK_SCRIPT.contains("\\\"kind\\\":") || HOOK_SCRIPT.contains("\"kind\":"));
    }
}
```

- [ ] **Step 2: Run it (fails — no `hook_script.sh`)**

Run: `cargo test -p agentum-executor hook_script`
Expected: FAIL (compile error: `hook_script.sh` not found by `include_str!`).

- [ ] **Step 3: Create the script `crates/agentum-executor/src/hook_script.sh`**

```sh
#!/bin/sh
# agentum managed agent-status hook. Normalizes per-agent hook payloads into
# {kind: working|done|permission} and POSTs to $AGENTUM_HOOK_URL. Codex passes
# JSON as argv[1]; Claude/Droid pipe via stdin. Never default to done on a parse
# miss (a false "done" would clear a live spinner).
if [ -n "$1" ]; then INPUT="$1"; else INPUT="$(cat)"; fi

EV="$(printf '%s' "$INPUT" | grep -oE '"hook_event_name"[[:space:]]*:[[:space:]]*"[^"]*"' | grep -oE '"[^"]*"$' | tr -d '"')"
if [ -z "$EV" ]; then
  EV="$(printf '%s' "$INPUT" | grep -oE '"type"[[:space:]]*:[[:space:]]*"[^"]*"' | grep -oE '"[^"]*"$' | tr -d '"')"
fi

KIND=""
case "$EV" in
  UserPromptSubmit|task_started|turn_started) KIND="working" ;;
  Stop|SubagentStop|agent-turn-complete|task_complete) KIND="done" ;;
  Notification|exec_approval_request|apply_patch_approval_request|request_user_input) KIND="permission" ;;
esac
[ -z "$KIND" ] && exit 0
[ -z "$AGENTUM_HOOK_URL" ] && exit 0

curl -s -X POST "$AGENTUM_HOOK_URL" \
  -H "X-Agentum-Hook-Token: $AGENTUM_HOOK_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"kind\":\"$KIND\",\"payload\":{\"event\":\"$EV\"}}" \
  --connect-timeout 2 --max-time 5 >/dev/null 2>&1 || true
exit 0
```

- [ ] **Step 4: Register the module**

In `crates/agentum-executor/src/lib.rs` add near the other `mod` lines:

```rust
mod hook_script;
pub use hook_script::HOOK_SCRIPT;
```

- [ ] **Step 5: Run tests (pass)**

Run: `cargo test -p agentum-executor hook_script`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/agentum-executor/src/hook_script.rs crates/agentum-executor/src/hook_script.sh crates/agentum-executor/src/lib.rs
git commit -m "feat(executor): add managed agent-status hook script (working/done/permission)"
```

---

### Task 3: `hook_install()` on `ToolAdapter` + Claude + Codex specs

**Files:**
- Modify: `crates/agentum-executor/src/adapters.rs` (trait default + ClaudeAdapter + CodexAdapter)
- Modify: `crates/agentum-executor/src/lib.rs` (export `AgentHookInstall`)
- Test: `crates/agentum-executor/src/adapters.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Define the install spec + trait method (failing test first)**

Add to `adapters.rs`:

```rust
/// How to register the managed status hook for an agent's CLI. The script is
/// written to `<dir>/agentum-hook.sh` by the server before launch; the spec
/// says how the CLI is told to run it.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentHookInstall {
    /// Extra argv that registers the hook (e.g. Claude `--settings <json>`).
    /// `{HOOK}` placeholders are substituted with the absolute script path.
    Argv(Vec<String>),
    /// Relocate the agent's config home via `env_var` to a managed dir and
    /// write `files` (relative path, contents with `{HOOK}` substituted) there.
    ConfigHome { env_var: &'static str, files: Vec<(&'static str, String)> },
}
```

Add to the `ToolAdapter` trait (default `None`):

```rust
/// Status-hook registration for this agent, or None if unsupported.
/// `hook_script_path` is the absolute path the server materialized the
/// managed script to for this session.
fn hook_install(&self, _hook_script_path: &str) -> Option<AgentHookInstall> {
    None
}
```

Test (append to the existing `#[cfg(test)] mod tests`):

```rust
#[test]
fn codex_hook_install_relocates_codex_home() {
    let spec = CodexAdapter.hook_install("/tmp/x/agentum-hook.sh").unwrap();
    match spec {
        AgentHookInstall::ConfigHome { env_var, files } => {
            assert_eq!(env_var, "CODEX_HOME");
            assert!(files.iter().any(|(p, c)| *p == "hooks.json"
                && c.contains("/tmp/x/agentum-hook.sh")
                && c.contains("UserPromptSubmit")));
        }
        _ => panic!("expected ConfigHome"),
    }
}

#[test]
fn claude_hook_install_uses_settings_argv() {
    let spec = ClaudeAdapter.hook_install("/tmp/x/agentum-hook.sh").unwrap();
    match spec {
        AgentHookInstall::Argv(a) => {
            assert!(a.iter().any(|s| s == "--settings"));
            assert!(a.iter().any(|s| s.contains("UserPromptSubmit")));
        }
        _ => panic!("expected Argv"),
    }
}
```

- [ ] **Step 2: Run (fails — method undefined)**

Run: `cargo test -p agentum-executor hook_install`
Expected: FAIL (no `hook_install`, no `AgentHookInstall`).

- [ ] **Step 3: Implement CodexAdapter::hook_install**

Codex honors `CODEX_HOME` for its config dir. Write a managed `hooks.json` that points SessionStart/UserPromptSubmit/Stop at the script. (Codex passes the event JSON as argv to the command, so the script's `$1` branch handles it.)

```rust
impl ToolAdapter for CodexAdapter {
    // ...existing methods...
    fn hook_install(&self, hook_script_path: &str) -> Option<AgentHookInstall> {
        let hooks_json = serde_json::json!({
            "hooks": {
                "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": hook_script_path }] }],
                "Stop":            [{ "hooks": [{ "type": "command", "command": hook_script_path }] }],
                "Notification":    [{ "hooks": [{ "type": "command", "command": hook_script_path }] }]
            }
        });
        Some(AgentHookInstall::ConfigHome {
            env_var: "CODEX_HOME",
            files: vec![("hooks.json", hooks_json.to_string())],
        })
    }
}
```

> NOTE: a relocated `CODEX_HOME` loses the user's `auth.json`/`config.toml`. The server task (Task 4) must seed the managed dir by symlinking the user's `~/.codex/{auth.json,config.toml,...}` into it, then overwrite only `hooks.json`. This is captured in Task 4 Step 2.

- [ ] **Step 4: Implement ClaudeAdapter::hook_install (replaces the inline block from sessions.rs)**

```rust
impl ToolAdapter for ClaudeAdapter {
    // ...existing methods...
    fn hook_install(&self, hook_script_path: &str) -> Option<AgentHookInstall> {
        let settings = serde_json::json!({
            "hooks": {
                "UserPromptSubmit": [{ "matcher": "*", "hooks": [{ "type": "command", "command": hook_script_path }] }],
                "Stop":             [{ "matcher": "*", "hooks": [{ "type": "command", "command": hook_script_path }] }],
                "Notification":     [{ "matcher": "*", "hooks": [{ "type": "command", "command": hook_script_path }] }]
            }
        });
        Some(AgentHookInstall::Argv(vec!["--settings".into(), settings.to_string()]))
    }
}
```

- [ ] **Step 5: Export the type**

In `crates/agentum-executor/src/lib.rs`:

```rust
pub use adapters::AgentHookInstall;
```

- [ ] **Step 6: Run tests (pass)**

Run: `cargo test -p agentum-executor hook_install`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/agentum-executor/src/adapters.rs crates/agentum-executor/src/lib.rs
git commit -m "feat(executor): per-adapter hook_install spec; Claude + Codex specs"
```

---

### Task 4: Apply `hook_install()` in the launch path

**Files:**
- Modify: `crates/agentum-server/src/routes/sessions.rs` (the `start` handler, lines ~462-507)

- [ ] **Step 1: Materialize the script + apply the spec**

Replace the `if session.tool == "claude" { ... }` block (lines 477-503) with a generic application. After setting `AGENTUM_HOOK_URL`/`AGENTUM_HOOK_TOKEN`:

```rust
// Materialize the managed hook script to a per-session dir.
let hook_dir = paths::session_runtime_dir(&session.id.to_string())
    .map_err(|e| ApiError::Internal(e.to_string()))?;
std::fs::create_dir_all(&hook_dir).ok();
let hook_script = hook_dir.join("agentum-hook.sh");
std::fs::write(&hook_script, agentum_executor::HOOK_SCRIPT)
    .map_err(|e| ApiError::Internal(e.to_string()))?;
#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&hook_script, std::fs::Permissions::from_mode(0o755));
}
let hook_path = hook_script.to_string_lossy().to_string();

if let Some(install) = adapter.hook_install(&hook_path) {
    match install {
        agentum_executor::AgentHookInstall::Argv(args) => {
            launch.argv.extend(args);
        }
        agentum_executor::AgentHookInstall::ConfigHome { env_var, files } => {
            let cfg_dir = hook_dir.join("agent-home");
            std::fs::create_dir_all(&cfg_dir).ok();
            // Seed the managed home from the user's real config so auth/config
            // survive the relocation; we only override the hooks file.
            if env_var == "CODEX_HOME" {
                if let Some(home) = dirs::home_dir() {
                    let real = home.join(".codex");
                    for name in ["auth.json", "config.toml"] {
                        let src = real.join(name);
                        if src.exists() {
                            let _ = std::fs::copy(&src, cfg_dir.join(name));
                        }
                    }
                }
            }
            for (rel, contents) in files {
                let _ = std::fs::write(cfg_dir.join(rel), contents);
            }
            launch.env.push((env_var.into(), cfg_dir.to_string_lossy().to_string()));
        }
    }
}
```

> If `paths::session_runtime_dir` does not exist, add it next to `paths::pane_log` in the `paths` module (a `~/.agentum/run/<id>/` dir). Reuse the existing pane-log parent dir if simpler.

- [ ] **Step 2: Build**

Run: `cargo build -p agentum-server`
Expected: compiles. (Add `dirs` to `agentum-server/Cargo.toml` if not already a dep.)

- [ ] **Step 3: Commit**

```bash
git add crates/agentum-server/src/routes/sessions.rs crates/agentum-server/Cargo.toml
git commit -m "feat(server): apply per-adapter hook_install on session launch"
```

---

### Task 5: `/hook` endpoint accepts working/done/permission

**Files:**
- Modify: `crates/agentum-server/src/routes/sessions.rs` (`hook` handler ~1277-1320, and the `agent.hook` event payload ~1309)

- [ ] **Step 1: Pass `kind` through verbatim (already does) — verify the event includes it**

The handler already emits `Event::new("agent.hook")` with `"kind": body.kind`. Confirm `body.kind` accepts arbitrary strings (it's `String`). No server-side mapping needed — the renderer maps kinds → status (Task 6). Add a test asserting a `working` hook emits `agent.hook` with `kind=working`:

```rust
#[tokio::test]
async fn hook_working_emits_event_with_kind() {
    // mirror hook_good_token_returns_204_and_emits_event, body.kind = "working"
    // assert ev.payload["kind"] == "working"
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p agentum-server hook_`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/agentum-server/src/routes/sessions.rs
git commit -m "test(server): cover working-kind agent.hook relay"
```

---

### Task 6: Renderer maps hook `kind` → agent status

**Files:**
- Create: `crates/agentum-desktop/ui/src/lib/agent-hook-status-map.ts`
- Create: `crates/agentum-desktop/ui/src/lib/agent-hook-status-map.test.ts`
- Modify: the WS handler that receives `agent.hook` (search `'agent.hook'` in `hooks/useIpcEvents.ts`; wire it to `setAgentStatus`)

- [ ] **Step 1: Failing test for the mapper**

`agent-hook-status-map.test.ts`:

```ts
import { describe, expect, it } from 'vitest'
import { hookKindToAgentState } from './agent-hook-status-map'

describe('hookKindToAgentState', () => {
  it('maps working/done/permission', () => {
    expect(hookKindToAgentState('working')).toBe('working')
    expect(hookKindToAgentState('done')).toBe('done')
    expect(hookKindToAgentState('permission')).toBe('waiting')
  })
  it('ignores unknown/tool_done', () => {
    expect(hookKindToAgentState('tool_done')).toBeNull()
    expect(hookKindToAgentState('nope')).toBeNull()
  })
})
```

- [ ] **Step 2: Run (fails)**

Run: `npx vitest run src/lib/agent-hook-status-map.test.ts`
Expected: FAIL (module missing).

- [ ] **Step 3: Implement the mapper**

`agent-hook-status-map.ts`:

```ts
import type { AgentStatusState } from '../../../shared/agent-status-types'

/** Map a managed-hook `kind` to an explicit agent state, or null to ignore. */
export function hookKindToAgentState(kind: string): AgentStatusState | null {
  switch (kind) {
    case 'working':
      return 'working'
    case 'done':
      return 'done'
    case 'permission':
      return 'waiting'
    default:
      return null
  }
}
```

- [ ] **Step 4: Run (pass)**

Run: `npx vitest run src/lib/agent-hook-status-map.test.ts`
Expected: PASS.

- [ ] **Step 5: Wire it into the `agent.hook` WS handler**

In the handler that processes `agent.hook` events (resolve the session → paneKey the same way the existing `tool_done` path does), call:

```ts
const state = hookKindToAgentState(evt.payload.kind)
if (state) {
  useAppStore.getState().setAgentStatus(paneKey, { state, agentType, prompt: undefined }, terminalTitle, { updatedAt: Date.now() })
}
```

(Reuse the existing paneKey resolution from the `agent.hook`/`tool_done` handling; do not invent a new attribution path.)

- [ ] **Step 6: Build the UI**

Run: `npm run build --prefix crates/agentum-desktop/ui`
Expected: builds clean.

- [ ] **Step 7: Commit**

```bash
git add crates/agentum-desktop/ui/src/lib/agent-hook-status-map.ts crates/agentum-desktop/ui/src/lib/agent-hook-status-map.test.ts crates/agentum-desktop/ui/src/hooks/useIpcEvents.ts
git commit -m "feat(ui): drive agent status from managed hook kinds (working/done/permission)"
```

---

### Task 7: Live end-to-end verification (Codex)

**Files:** none (manual verification — the spinner can't be unit-tested through a real PTY).

- [ ] **Step 1: Rebuild + relaunch**

```bash
cargo build -p agentum-desktop
npm run build --prefix crates/agentum-desktop/ui
pkill -f 'target/debug/agentum-desktop'; target/debug/agentum-desktop &
```

- [ ] **Step 2: Start a Codex session and send a prompt.** Confirm in the sidebar that the worktree shows the **yellow spinning** `StatusIndicator` while Codex works, the compact agent row dot shows `working`, and both settle to `done` after the turn.

- [ ] **Step 3: Verify the env injection** (debug aid):

```bash
tmux -L default show-environment -t agentum-<codex-session> | grep -E 'CODEX_HOME|AGENTUM_HOOK'
cat "$CODEX_HOME/hooks.json"   # should be the managed file pointing at agentum-hook.sh
```

- [ ] **Step 4: Commit any fixes; tag Phase 1 done.**

---

## Phase 2 — Remaining agents (one task each, same recipe)

For each agent in `AGENT_HOOK_TARGETS` not yet wired — `openclaude, gemini, antigravity, amp, cursor, droid, command-code, grok, copilot, hermes` — repeat this recipe. **Do not write the spec until the contract is verified** (no guessing — a broken hook is worse than none):

1. **Verify the contract:** `<agent> --help` for a config-dir/hook flag or env var; check the agent's docs for hook event names + payload delivery (argv vs stdin) + the event field name. Record findings in this file.
2. **Extend the script** (`hook_script.sh`) only if the agent uses new event names — add them to the existing `case` arms (keep one script for all).
3. **Add `hook_install()`** to that adapter in `adapters.rs` (Argv for `--settings`-style CLIs, ConfigHome for `*_HOME`-style CLIs), with a unit test mirroring Task 3.
4. **Live-verify** per Task 7.
5. **Commit** per agent.

Known starting points (from `~/.superset/hooks/notify.sh`, to be re-verified per CLI version):
- `droid`: stdin, `"hook_event_name"`, same events as Claude.
- `cursor`: native title is the bare string `Cursor Agent` (no status) — needs hook injection; verify cursor-agent's hook mechanism.
- `gemini`: already emits status **symbols** in its title (`✦`/`⏲`/`◇`/`✋`) handled by `detectAgentStatusFromTitle` — may need **no hook** at all; verify before adding one.

---

## Self-Review

- **Spec coverage:** root-cause fix = Tasks 1–6; reported Codex symptom = Tasks 2–4 + 6–7; "all agents" = Phase 2 recipe. Hook-URL port bug = Task 1. ✓
- **Type consistency:** `AgentHookInstall` (Argv / ConfigHome) defined Task 3, used Task 4. `HOOK_SCRIPT` defined Task 2, used Task 4. `hookKindToAgentState` defined + used Task 6. `setAgentStatus` matches the existing store signature (`paneKey, payload, terminalTitle, timing`) verified in `store/slices/agent-status.ts`. ✓
- **Placeholders:** Phase 2 is intentionally a recipe, not fabricated per-agent code — verifying each CLI's contract is a required step, and inventing it would ship broken hooks. This is a deliberate decomposition, not a placeholder. ✓
- **Open item to confirm during Task 4:** existence/signature of `paths::session_runtime_dir`; if absent, add it beside `paths::pane_log`.
