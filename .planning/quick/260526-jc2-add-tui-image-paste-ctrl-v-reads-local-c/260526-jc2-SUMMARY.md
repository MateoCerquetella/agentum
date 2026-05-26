---
phase: quick-260526-jc2
plan: 01
subsystem: tui-image-paste
tags:
  - tui
  - clipboard
  - daemon-route
  - arboard
  - upload
requires:
  - "agentum-tmux::send_keys (existing)"
  - "agentum-server auth middleware chain (existing)"
provides:
  - "POST /api/sessions/{id}/uploads route"
  - "TUI Ctrl-V → local clipboard → daemon write → tmux send-keys flow"
  - "Client::upload_image method"
affects:
  - "agentum-server router surface"
  - "TUI key-handler hot path"
tech-stack:
  added:
    - "arboard 3 (image-data + wayland-data-control features only)"
    - "image 0.25 (png feature only — encode RGBA→PNG)"
  patterns:
    - "Daemon-controlled filename → safe send-keys (no shell metachars, no Enter)"
    - "spawn_blocking for sync OS clipboard reads off the tokio main task"
    - "mpsc result channel pattern (mirrors agent_tasks_tx)"
    - "Per-profile client lookup for peer-owned sessions (client_for_session)"
key-files:
  created:
    - crates/agentum-server/src/routes/uploads.rs
    - .planning/quick/260526-jc2-add-tui-image-paste-ctrl-v-reads-local-c/260526-jc2-SUMMARY.md
  modified:
    - crates/agentum-server/src/routes/mod.rs
    - crates/agentum-server/src/lib.rs
    - crates/agentum/Cargo.toml
    - crates/agentum/src/commands/terminal/api.rs
    - crates/agentum/src/commands/terminal/app.rs
    - Cargo.lock
decisions:
  - "Ctrl-V (not Ctrl-Shift-V) — Ctrl-Shift-V is reserved for terminal emulators' own host-clipboard text paste"
  - "Filename is daemon-controlled (timestamp + 5-byte random hex + sanitized extension) — never derived from user-supplied Content-Type, so send-keys can never inject shell metacharacters into the pane"
  - "25 MiB inline cap + DefaultBodyLimit route override; matches Claude Code's attachment surface"
  - "No trailing Enter from send-keys — the user commits the prompt themselves so they can add context after the path"
  - "Use mpsc UploadOutcome channel (option (b) from plan), not the existing toast channel, so the spawn_blocking task can post results without borrowing &mut App"
  - "Keep Ctrl-K I (paste_from_system_clipboard) — Ctrl-V is the local-clipboard-over-SSH path; Ctrl-K I is the host-side helper path. Complementary, not replacement."
metrics:
  duration_minutes: ~50
  commits: 3
  completed_at: 2026-05-26
---

# Quick 260526-jc2 Plan 01: Add TUI Image Paste (Ctrl-V) Summary

**One-liner:** Ctrl-V in the TUI reads the LOCAL OS clipboard via arboard, PNG-encodes the pixels off the main task, POSTs the bytes to the new bearer-auth-gated `POST /api/sessions/{id}/uploads` route — which writes the file under `<workdir>/.agentum-uploads/` and `tmux send-keys`s the relative path (no Enter) into the agent pane.

## What Shipped

### 1. New daemon route — `POST /api/sessions/{id}/uploads`

`crates/agentum-server/src/routes/uploads.rs` is the new module. It exposes a single `pub fn router() -> Router<AppState>` that mounts `POST /api/sessions/{id}/uploads` with a per-route `DefaultBodyLimit::max(25 MiB)` so the route can accept image bodies without raising the global axum default for every other endpoint.

The handler signature mirrors `routes::sessions::send`:

```rust
async fn upload(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<UploadResponse>), ApiError>
```

Pipeline:

1. **Body gates:** reject empty (`400`), reject `> 25 MiB` (`400`).
2. **Session lookup:** `parse_uuid` → `store.get_session_by_id` (`404` on miss).
3. **Running gate:** require `tmux_target`; `has_session` must return true (else `400 — "session is not running" / "tmux session not active for this session"`).
4. **Filename construction (security-load-bearing):** `Content-Type` → `mime_to_ext` (strips `; charset=…` params, lowercase, known image set or `"bin"`) → `sanitize_ext` (clamp to known set ∪ `"bin"`, blocks any `/`, `\`, `.`, or oversize extension). Filename body is `YYYYMMDD-HHMMSS-XXXX.<ext>` where `XXXX` is 10 lowercase hex chars from a fresh UUIDv4. No user-supplied component ever reaches the filesystem path or the `send-keys` payload.
5. **Workdir expansion:** reuses `super::util::expand_workdir` so `~`/`$HOME` paths in `session.workdir` resolve identically to every other workdir-aware route.
6. **Disk write:** `tokio::fs::create_dir_all(.agentum-uploads)` + `tokio::fs::write(abs_path, body)`.
7. **tmux send-keys:** `agentum_tmux::send_keys(target, &format!("{relative_path} "), false)` — trailing space, no Enter. The user commits the prompt themselves.
8. **Event broadcast:** `Event::new("session.upload").with_session(id, name).with_payload({path, size_bytes})` on `state.bus`.
9. **201 Created** + JSON `{ path, relative_path, size_bytes }`.

**Auth gating:** the route is merged in `lib.rs::router()` between `routes::sessions::router()` and `routes::agents::router()`, ahead of `auth::require_token`. It inherits bearer-token enforcement automatically; `auth::is_public` is NOT modified. A `curl -X POST … /api/sessions/$ID/uploads` with no `Authorization` header returns `401`.

### 2. TUI HTTP client — `Client::upload_image`

`crates/agentum/src/commands/terminal/api.rs`:

- New `UploadResponse` mirror struct (`path`, `relative_path`, `size_bytes`) — `#[allow(dead_code)]` on the absolute `path` field for forward-compat debugging surfaces.
- `Client::upload_image(id, bytes, mime) -> Result<UploadResponse>` — bearer-auth POST with caller-supplied `Content-Type`, bails with `{status} — {body}` on non-2xx (same pattern as `reset_agent_tasks`).
- `build_upload_url(base, id)` pure helper so the route shape is pinned by a unit test (`upload_url_is_session_scoped`) without spinning up a real `Client`.

### 3. TUI Ctrl-V key handler

`crates/agentum/src/commands/terminal/app.rs`:

- New global Ctrl-V branch in `handle_key`, placed between Ctrl-T (right-panel toggle) and Ctrl-Shift-Left/Right (split-resize). The match is `KeyCode::Char('v') | KeyCode::Char('V')` with `CONTROL` modifier set and `SHIFT` NOT set — `Ctrl-Shift-V` deliberately falls through so the user's terminal emulator (kitty, alacritty, gnome-terminal, …) still handles its host-clipboard text-paste binding.
- `spawn_ctrl_v_image_paste(app, client)`:
  - Focus gate: `Focus::Term | Focus::TermRight` only.
  - Session gate: requires `app.selected = Some(_)`.
  - Per-profile client lookup: `app.client_for_session(id)` falls back to the active `client` — a peer-owned session uploads to that peer's daemon (the file must land next to the agent's workdir).
  - Posts status hint `"Ctrl-V: reading clipboard…"` synchronously, then spawns a detached task.
  - Inside the task: `tokio::task::spawn_blocking` wraps `arboard::Clipboard::new()` + `get_image()` + `encode_rgba_as_png` so a slow X11 selection-owner negotiation can't stall the ratatui render loop.
  - Result is posted through the new `app.upload_outcome_tx` mpsc channel.
- New `App::upload_outcome_tx: Option<mpsc::UnboundedSender<UploadOutcome>>` field + `UploadOutcome { ok: bool, message: String }` type.
- Run-loop `select!` gained a sibling arm to `agent_tasks_rx.recv()` that drains `upload_outcome_rx` and routes success → `app.status_msg`, failure → `app.push_error`.
- Helper `encode_rgba_as_png(width, height, rgba) -> Result<Vec<u8>, String>` uses `image::codecs::png::PngEncoder` with `ExtendedColorType::Rgba8`. Pure-sync so the test can pin the PNG magic-byte prefix.
- Helper `clipboard_error_message(arboard::Error)` maps `ContentNotAvailable` → the stable "no image in clipboard…" hint (greppable in issue reports); other variants get readable wrappings.
- Updated the Ctrl-K chord hint string to advertise `· Ctrl-V: paste clipboard image`.

### 4. Cargo deps

`crates/agentum/Cargo.toml`:

```toml
arboard = { version = "3", default-features = false, features = ["image-data", "wayland-data-control"] }
image   = { version = "0.25", default-features = false, features = ["png"] }
```

- `arboard` defaults off + explicit `image-data` (needed for `Clipboard::get_image()`) keeps the intent visible at the call site. `wayland-data-control` opts the daemon into the modern Wayland protocol (omarchy is the primary dev host per project MEMORY).
- `image` 0.25 matches what arboard 3 already pulls transitively — no dep-graph duplication.
- `Cargo.lock` updated and committed.

## How to Verify

### Automated (CI-ready)

```bash
cargo build -p agentum-server -p agentum
cargo test -p agentum-server --lib uploads          # 5 tests, all green
cargo test -p agentum --lib ctrl_v_tests            # 4 tests, all green
cargo test -p agentum --lib commands::terminal::api # 5 tests incl. upload_url, all green
cargo clippy -p agentum-server -p agentum --all-targets -- -D warnings   # clean
cargo fmt -p agentum-server -p agentum -- --check   # clean
```

### Manual (after `cargo build --release && agentum serve`)

1. Start a session targeting Claude or any agent with a chat surface.
2. `agentum terminal`; navigate to the new session; focus its terminal pane (Ctrl-Right or click).
3. Copy any image to the OS clipboard (screenshot, `wl-copy < screenshot.png`, browser image right-click → copy image, …).
4. Press **Ctrl-V**. Expect:
   - Bottom-right status flips to `Ctrl-V: reading clipboard…` then `uploaded .agentum-uploads/<file>.png (N bytes)`.
   - `ls <session-workdir>/.agentum-uploads/` shows the PNG with a sensible `YYYYMMDD-HHMMSS-XXXXXXXXXX.png` filename.
   - The agent pane shows the relative path typed at its prompt **with no Enter pressed**. Cursor sits at the end of `path + space`.
   - Pressing Enter at this point sends the prompt to Claude; Claude reads the PNG and acknowledges the attachment.

### Manual negative paths

- No image in clipboard → toast `no image in clipboard — copy an image first (Ctrl-V is for images only — use bracketed paste for text)`. No panic, no frozen UI.
- No session selected → toast `Ctrl-V: no session selected`.
- Tree focus → toast `Ctrl-V: focus a terminal pane first`.
- `curl -k -X POST https://127.0.0.1:8822/api/sessions/$ID/uploads --data-binary @img.png -H 'Content-Type: image/png'` with **no** `Authorization` header → `401 Unauthorized`. Proves auth gating (no allow-list change).

## Deviations from Plan

**None — plan executed as written.** All three tasks landed with the artifact shapes, behaviour, gating, and tests the plan specified. The only minor deltas were stylistic:

- The plan's example for `encode_rgba_as_png` referenced `image::ColorType::Rgba8.into()`; on `image` 0.25 the encoder wants `ExtendedColorType::Rgba8` directly (no `.into()`). Took the direct form; semantically identical.
- I picked option (b) from the plan's mpsc choice: dedicated `upload_outcome_tx` channel + `UploadOutcome { ok, message }` (mirrors `agent_tasks_tx` exactly). The plan explicitly listed this as the preferred shape.
- The plan suggested mentioning the binding in the command palette help text "if and only if the surrounding code structure makes it a one-line addition; otherwise skip." The Ctrl-K chord hint update was the cleanest one-line discovery surface; no palette entry change was needed beyond that.

## Known Stubs

None. The route writes real files, fires real `send-keys`, and the TUI flow does a real `arboard` clipboard read + real daemon upload end-to-end. No mock data, no placeholder.

## Self-Check: PASSED

```bash
# Created files
[ -f crates/agentum-server/src/routes/uploads.rs ] && echo "FOUND: routes/uploads.rs"
# → FOUND: routes/uploads.rs

# Modified files
grep -q "routes::uploads::router" crates/agentum-server/src/lib.rs && echo "FOUND: router merge"
# → FOUND: router merge
grep -q "pub mod uploads" crates/agentum-server/src/routes/mod.rs && echo "FOUND: mod registration"
# → FOUND: mod registration
grep -q "pub async fn upload_image" crates/agentum/src/commands/terminal/api.rs && echo "FOUND: upload_image"
# → FOUND: upload_image
grep -q "spawn_ctrl_v_image_paste" crates/agentum/src/commands/terminal/app.rs && echo "FOUND: Ctrl-V handler"
# → FOUND: Ctrl-V handler
grep -q "arboard" crates/agentum/Cargo.toml && echo "FOUND: arboard dep"
# → FOUND: arboard dep

# Commits
git log --oneline | grep -E "260526-jc2-0[123]" | wc -l
# → 3 (5df27b9, 1c2a778, 78a69be)
```

All claims verified.

## Commits

| Hash | Type | Message |
|------|------|---------|
| `5df27b9` | feat | `feat(quick-260526-jc2-01): add POST /api/sessions/{id}/uploads route` |
| `1c2a778` | feat | `feat(quick-260526-jc2-02): TUI Client::upload_image + arboard/image deps` |
| `78a69be` | feat | `feat(quick-260526-jc2-03): wire Ctrl-V image paste in TUI app.rs` |

## Test Infrastructure Notes

- Pre-existing flakiness: `commands::terminal::app::osc52_tests::plain_terminal_uses_bare_osc52` and its sibling `inside_tmux_uses_dcs_passthrough` race against each other when the cargo test runner parallelises across crates. The test guards `$TMUX` with `unsafe { std::env::set_var/remove_var }` inside `with_tmux_env`, and parallel test bodies that read `$TMUX` during the guard window observe the wrong branch. **Confirmed reproducible on the base commit (`c40d48a`) without any of my changes** by running `cargo test -p agentum-server -p agentum --lib` five times in a row. Out of scope for this plan — logged here for the next person who sees a red CI run.

## Follow-ups (explicitly NOT in this plan)

- **Dashboard parity:** wire the same flow via `FileReader.readAsArrayBuffer` → `apiUrl('/api/sessions/{id}/uploads')` POST in `dashboard/src/lib/components/TerminalPanel.svelte` or `Terminal.svelte`. Same wire path, same auth, no additional schema work needed.
- **Drag-and-drop:** dashboard could accept a drag-dropped image file the same way. Already gated on the server side by the same route — the dashboard just needs the `dragover` + `drop` listeners and a single `request<UploadResponse>('POST', `/api/sessions/${id}/uploads`, …)` call.
- **Multi-image paste:** arboard returns a single image at a time; clipboard managers that hold multiple images aren't supported. Users who need this can paste each image with successive Ctrl-V presses.
- **OSC52 test flake:** the env-var-juggling tests need a process-wide mutex (similar to `tests_helpers_create` in board.rs) to serialise. Not blocking; logged.

## Threat Flags

None. The route surface is fully covered by the plan's STRIDE register (T-up-01 through T-up-06 + T-up-SC), all dispositions are `mitigate` (or `accept` for already-out-of-scope items), and the implementation honours every mitigation:

- T-up-01 (filename construction): sanitize_ext + UUIDv4-derived random hex.
- T-up-02 (DoS): 25 MiB inline cap + DefaultBodyLimit route layer.
- T-up-03 (path traversal): expand_workdir + `workdir.join(".agentum-uploads")`.
- T-up-04 (unauthenticated upload): merged ahead of auth middleware; is_public unchanged.
- T-up-05 (send-keys injection): daemon-controlled relative path with no shell metacharacters, `false` for append_enter.
- T-up-06 + T-up-SC (accepted): host-clipboard tampering by other apps; arboard/image dep supply-chain — both outside this plan's scope.

No new trust boundaries introduced.
