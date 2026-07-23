# Current Milestone

## Current Focus

Mobile reach + polish on the beta. Primary thrust is the PWA (mobile
terminal viewer, mobile session manager) and push notifications for agent
completion/crash. Secondary: smoothing TUI rough edges and keeping CI
green across platforms.

Recent groundwork: new sessions default to creating a git worktree (TUI +
dashboard), and CI is pinned to Rust 1.94.1 to match local fmt/clippy.

---

## Active Specs

- None yet — SDD+DDD was just adopted. First spec to be drafted via
  `/sdd-spec` or `/sdd-spec-socratic`.

---

## Risks

- macOS-only build/clippy/test failures are invisible to the local Linux
  dev loop and only surface on tagged CI runs.
- PWA scope creep (offline support, install prompt) vs. shipping the
  read-only viewer first.
- YOLO-flag spellings for unverified tools (OpenCode, Aider) are unknown.

---

## Open Questions

- Push-notification transport for a self-hosted, no-cloud setup
  (Web Push + VAPID on the daemon)?
- How much write capability should the mobile PWA expose beyond
  kill/restart?
