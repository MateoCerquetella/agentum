# Decisions: Tui Session Ux And Observability Fixes For The Agentum Terminal

Record concise, externally reviewable evidence and choices here. Do not store
private chain-of-thought, prompts, credentials, secrets, or scratchpad text.

## D-001: Extend the existing two-pane model

Status: Accepted

### Evidence

- `App::split_right`, `TermSlot`, independent stream handles, focus cycling,
  persisted split percentage, and resize messages already implement a primary
  plus auxiliary terminal layout.
- The requested workflow needs an additional terminal/agent beside the current
  session, not an arbitrary window manager.

### Options

1. Replace the pane model with a vector/tree supporting arbitrary N panes.
2. Extend the existing two-pane model with an explicit create destination.

### Chosen approach

Use option 2. Add `CreateDestination` to the create flows and attach successful
auxiliary launches to `split_right` while preserving the primary stream.

### Trade-offs and risks

This deliberately caps the visible terminal layout at two panes. It minimizes
layout/stream churn and matches the contract's non-goal. Creation and start are
still separate operations, so partial failure must retain a recoverable row and
produce one contextual diagnostic.

### Verification

Unit-test destination handling and stream preservation; verify terminal,
Claude, Codex, unavailable-tool, and start-failure cases in a real PTY.

## D-002: Model observability as data plus status

Status: Accepted

### Evidence

- Usage errors currently become `None`; agent-task errors also become `None`.
- The renderers cannot distinguish loading, empty, stale, unsupported,
  incompatible, or unreachable states from those values.

### Options

1. Continue logging failures only and infer UI state from missing payloads.
2. Carry typed success/failure/status metadata alongside the last good data.

### Chosen approach

Use option 2. Add source/freshness metadata to server responses and typed TUI
outcomes. Preserve the last good data on failure but mark it stale and retain a
copyable diagnostic.

### Trade-offs and risks

More states increase model and renderer complexity. Central enums and exhaustive
matches prevent silent fallback. Wire additions remain backward compatible,
and the task client accepts the legacy response shape.

### Verification

Test every status transition, especially initial failure, failure after success,
retry success, unsupported tool, incompatible daemon, and empty current data.

## D-003: Resolve transcripts on the session host

Status: Accepted

### Evidence

- Sessions carry `host_id` and can run through `host_runtime` on SSH hosts.
- `TranscriptStore::ensure_started` currently converts `session.workdir` to a
  local `PathBuf`, so an SSH session is read from the wrong machine.
- Local `notify` cannot watch a remote filesystem path.

### Options

1. Support task panels only for local sessions and mark SSH unsupported.
2. Copy remote transcripts to the daemon through a new synchronization agent.
3. Abstract transcript reads and reconcile SSH files through existing
   host-runtime SSH execution while retaining local watchers.

### Chosen approach

Use option 3. Keep `notify` for low-latency local updates and add bounded
periodic reconciliation for both local recovery and SSH reads.

### Trade-offs and risks

Remote polling adds SSH work and cannot be as immediate as local notifications.
Use bounded intervals, incremental offsets, and only active/requested slots.
Never fall back from a failed remote read to a local same-shaped path.

### Verification

Test local creation/append/replacement/truncation, two same-workdir sessions,
daemon restart, mocked SSH reads, SSH failure, and missed-event reconciliation.

## D-004: Store structured redacted diagnostics

Status: Accepted

### Evidence

- `ErrorEntry` retains a string but the overlay flattens and truncates it.
- Runtime context already contains versions, session/host identity, daemon
  reachability, and resolvable log paths.
- Deferred OSC-52 clipboard output already avoids corrupting the alternate
  screen.

### Options

1. Concatenate more context ad hoc at every error call site.
2. Store a structured diagnostic and render/copy one centrally redacted report.

### Chosen approach

Use option 2. Add an explicit diagnostic builder plus a compatibility wrapper
for current `push_error` call sites, and copy the selected full report with the
existing deferred clipboard mechanism.

### Trade-offs and risks

Heuristic redaction can miss new secret shapes or over-redact harmless text.
Cover known credential formats with tests, never include raw request headers,
and keep explicit operation/session arguments for high-value paths.

### Verification

Test multiline preservation, all context fields, unknown values, local/SSH log
paths, selection/copy bytes, and bearer/password/hook-token redaction.

## D-005: Preserve terminal input while changing Create to `C`

Status: Accepted

### Evidence

- Lowercase `c` is an existing tree-only card-hint binding.
- Printable terminal keys are forwarded when a terminal has focus.
- A global printable `C` binding would swallow normal agent/shell input.

### Options

1. Bind uppercase `C` globally.
2. Bind uppercase `C` only when the tree owns input and expose global creation
   through the command palette.

### Chosen approach

Use option 2. Remove the tree `n` binding, preserve lowercase `c`, update all
hints to `C`, and keep literal terminal `C` forwarding.

### Trade-offs and risks

Creating from terminal focus takes the global command-palette path rather than
one printable key. This avoids corrupting prompts and remains discoverable.

### Verification

Test `C`, `c`, and `n` in tree focus, literal `C` in both terminal panes, and
all help/palette hints.
