# AutoWiki build — agent instructions

You are building **spec 001 AutoWiki** in the agentum repo. Read these BEFORE
touching code:

- **Spec** (problem, ACs, scope): `ai/specs/001-autowiki/spec.md`
- **Architecture** (the build map — exact artifact shapes + `file:line` citations):
  `ai/specs/001-autowiki/architecture.md`  ← your primary reference
- **Repo guide:** `CLAUDE.md`

## Where you are

Worktree `feat/autowiki` off `origin/develop` @ `fe1a2a6a`. All paths are relative
to this worktree root. `cd` here for `cargo` / `npm`. (Ignore any sibling
`new-idea` worktree — it is 237 commits stale.)

## Non-negotiable invariants (architecture.md §5)

1. **One launch path.** Agent spawns go through `spawn_agent_into_pane`
   (`routes/sessions/provision.rs:91`). Never bespoke tmux/argv.
2. **YOLO mandatory** on spawned agents: `flags: vec![YOLO_MARKER]`.
3. **Inconclusive ≠ success.** Missing/garbled `index.json` ⇒ `.status.json{failed}`
   ⇒ UI shows an error. Mirror `parse_qa_verdict` (`harness/helpers.rs:132`).
4. **Auth.** `/api/wiki` rides the global `require_token` layer — do NOT add it to
   `is_public` (`auth.rs:74`).
5. **Slug guard.** `GET /api/wiki/{slug}` rejects any slug outside
   `^[a-z0-9][a-z0-9-]*$` before joining a path (traversal).

## The gate

`verify.sh` is the unit gate — run it and get it green before declaring a feature
done. `qa.sh` is the browser QA gate (the `wiki-view` slice only).

## Build commands (cargo lives at `~/.cargo/bin`)

- Backend: `cargo test -p agentum-server --lib`
- UI: `npm run build --prefix crates/agentum-desktop/ui`
