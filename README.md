# Agentum

A self-hosted workspace for one person, all in Rust:

- **Personal**: mail, calendar, notes, a dashboard and a Secretary that runs on *your own* model,
  so your private life never reaches a cloud provider.
- **Coding**: Claude Code, Codex and OpenCode running on your machines, in threads you watch live
  and steer: questions and permissions, queued or steered messages, diffs, git, pull requests,
  files, an editor and terminals, side chats, search over everything that was said.
- **Plugins**, in the style of BB's: each one a manifest, a hub part and a web part mounted in slots.

```
browser ──HTTP + one WebSocket──▶ agentum (hub: axum + SQLite, serves the Leptos/wasm web app)
                                     ▲ one WebSocket per machine
             agentd on each machine ─┘ (runs the agents, files, git, terminals)
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the crates, the protocol and the plugin API,
and [docs/COMPARE-BB.md](docs/COMPARE-BB.md) for how moving around threads compares with BB and
the old Agentum desktop app.

## Build

```
rustup target add wasm32-unknown-unknown x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock>
# plus wasm-opt (binaryen), brotli, cargo-zigbuild and zig
deploy/build.sh        # dist/agentum-<arch>, dist/agentd-<arch>
```

## Run

1. The hub: `deploy/agentum.service` (or just run `agentum` with `AGENTUM_DATA` and `AGENTUM_BIND`),
   behind a TLS proxy; the two WebSocket locations are in `deploy/nginx-locations.conf`.
2. Each machine: Settings → Machines → Add machine shows a one-time command:
   `agentd install --hub wss://<hub>/agentd --token <token> --name <machine>` (a systemd user
   service; the agents' CLIs must be installed and signed in on that machine).

The project grew out of an earlier personal dashboard (CERQ) and replaces the author's use of BB.

## License

MIT
