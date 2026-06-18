<!--
Every PR must trace back to an issue. If there isn't one yet, open it first
(use the issue templates) — that's where the documentation + labels live.
-->

## Linked issue

Closes #<!-- issue number -->

## What changed

<!-- Summary of the change. Keep it to what a reviewer needs. -->

## How it was verified

<!-- Commands run, tests added, manual steps. e.g.
cargo test -p agentum-executor -p agentum-server -p agentum --lib
npm run build --prefix crates/agentum-desktop/ui
-->

## Checklist

- [ ] Linked to an issue above (`Closes #N`)
- [ ] Issue is labeled (`type/*` + `area/*` + `priority/*`)
- [ ] Tests added/updated, or N/A with reason
- [ ] `cargo fmt` / lints clean for touched crates
- [ ] CLAUDE.md updated if architecture/crates/gotchas changed
