# CLI reference (`agentum`)

The CLI is **BYO-tool**: `--tool` is required on `new` and accepts any
executable on `PATH`. agentum ships with **no default tool** — the user
picks per session.

Four first-class interchangeable executors are targeted:

| Executor     | `--tool` value | Notes                                       |
|--------------|----------------|---------------------------------------------|
| Claude Code  | `claude`       | Anthropic's official CLI agent              |
| Codex        | `codex`        | OpenAI's CLI coding agent                   |
| Gemini       | `gemini`       | Google's Gemini CLI agent                   |
| Hermes       | `hermes`       | Open-weights agent                          |

Additional tools (`opencode`, `aider`, `cursor`, or any custom binary)
are supported but treated as unvalidated passthrough — agentum trusts
whatever is on `PATH`.

## Synopsis

```
agentum new <name> --tool <cli> --dir <path> [--model <m>] [--arg KEY=VAL]… [--up]
agentum up <name>                       # start a registered session
agentum down <name>                     # stop gracefully (SIGTERM, then SIGKILL after 5 s)
agentum kill <name>                     # immediate SIGKILL
agentum rm <name> [--force]             # remove (must be down unless --force)
agentum ls [--running] [--tool <t>]     # list sessions
agentum ps                              # alias for `ls --running`
agentum open <name>                     # tmux attach passthrough (detach: Ctrl-b d)
agentum tail <name> [-n 30] [-f]        # show last N lines (or follow)
agentum send <name> <text>              # send text + Enter
agentum keys <name> <key-spec>          # raw tmux keys, e.g. 'C-c'
agentum serve [--port 8822] [--no-tls]  # start dashboard
agentum auth show                       # print bearer token
agentum auth rotate                     # generate a new bearer token
agentum config get <key>
agentum config set <key> <value>
agentum config edit                     # open $EDITOR on config.toml
agentum doctor                          # check tmux, XDG dirs, db, cert, port
agentum --version
agentum --help
```

## Semantics

- **No `register` / `start` split** — `agentum new <name> --up` is the
  shortcut.
- **No `--yolo`** — pass agent-specific flags through `--arg`. Example
  for Claude:
  `agentum new alpha --tool claude --dir ~/proj --arg dangerously-skip-permissions=true --arg model=opus`.
  agentum forwards these as `--<key>` (or `--<key>=<value>`) to the
  configured tool.
- **No default tool** — `agentum new` errors if `--tool` is omitted.
  Set a per-user default with
  `agentum config set default_tool claude` (still explicit at the CLI).
- **DB lazy-init** — first `new` or `serve` creates
  `$XDG_DATA_HOME/agentum/db.sqlite` and the migrations apply.

## Exit codes

| Code | Meaning                                    |
|------|--------------------------------------------|
| 0    | ok                                         |
| 1    | generic error                              |
| 2    | usage / bad args                           |
| 3    | not-found (no session by that name)        |
| 4    | already-exists                             |
| 5    | backend not reachable (`serve` down)       |
| 6    | tmux missing / unhealthy                   |
| 7    | tool binary not found on `PATH`            |
