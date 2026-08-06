# Live VPS verification — revision 7

- Observed at: 2026-08-05T18:26Z
- Client: installed release build at `/Users/mateocerquetella/.local/bin/agentum`
- Installed SHA-256: `70600f81086cd3acbcfd188772c19bf1dcbf756772ccbf0291fe8e4861b3910d`
- Target: saved SSH host and existing remote tmux session (address redacted)

## Input fidelity

The installed Agentum TUI was launched in a real PTY, its terminal pane was
focused, and the following marker was entered through the TUI input path:

```text
empirical-λ-🛠-BYTES-check
```

An independent remote `tmux capture-pane` read showed that exact marker in the
target pane. Unicode characters were preserved, and `BYTES` appeared only as
the intentional substring of the marker—not as a substituted payload or debug
label. The persistent SSH input-writer process remained alive after delivery.

## Latency

Before the fix, an operation reusing the live SSH control master took about
5.25 seconds because each child SSH process re-ran the user's expensive
configuration match command. After the fix, the equivalent pooled remote
`true` operation took 0.75 seconds and `tmux capture-pane` took 0.72 seconds.
This is approximately a 7x improvement (about 86% lower per-operation latency).

Cold connections continue to load the user's SSH configuration. Only commands
that target a verified existing private control socket skip reparsing it.

## Paste reliability

The real-tmux multi-kilobyte paste regression passed ten consecutive runs with
the paced 64-byte hex-line transport. Full workspace package checkpoints also
passed under Empirical's immutable evidence runner.

## Artifact note

Agentum TUI is a terminal-only application, so browser automation is not an
applicable verifier. This durable PTY observation records the real interactive
terminal result without claiming browser or screenshot evidence.
