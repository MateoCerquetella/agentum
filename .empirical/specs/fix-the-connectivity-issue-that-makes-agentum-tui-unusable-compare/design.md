# Design: Self-Healing and Fast SSH Sessions

## Overview

Keep the hardened OpenSSH transport in `agentum-tmux::ssh` and port the sibling
project's proven recovery behavior into the current monolithic server modules.
The design treats a remote stream as two long-lived SSH children plus bounded
one-shot control operations:

1. the streaming master carries the pane-log tail;
2. the interactive master carries a persistent input writer and short execs;
3. a real remote no-op proves that each pooled master is usable;
4. tail EOF triggers snapshot-and-offset recovery inside the existing WebSocket;
5. a stuck input write falls back to the resilient one-shot path;
6. host lifecycle cancellation always wins over reconnect or input restoration.

The sibling's module extraction establishes useful boundaries, but copying its
files wholesale would replace this tree's stricter password-secret lifecycle,
record/revision ControlPath namespace, exact tmux identity resolution, and host
mutation registry. Small internal helpers provide the same separation without
changing unrelated public ownership.

## Pooled-master health and warmup

`agentum-tmux::ssh` remains the source of truth for ControlPath construction and
already exposes exact per-host/per-role exit commands. `agentum-server` adds a
bounded, best-effort `evict_ssh_master(host, mux)` wrapper and a health outcome
classifier used by the warmer.

For each `Interactive` and `Streaming` role, the warmer runs a real remote
`true` over that exact pooled connection. The two roles are probed concurrently.

- Success proves the network path and leaves the master warm; no second remote
  no-op is executed.
- A timeout or a recognized pre-session mux failure evicts that exact role with
  bounded `ssh -O exit`, then performs one pooled reopen/verification attempt.
- Another nonzero result (including channel pressure on a live shared master)
  is surfaced or treated as live-but-busy without evicting the shared process.
- The interactive result is required because reverse forwards depend on it;
  streaming warmup remains best-effort.

The existing global warm lock continues to serialize warmup, credential
invalidation, and reverse-forward reconciliation. A successful interactive
probe is followed by the existing PID-generation check and desired-forward
reconciliation. This preserves tunnel correctness while reducing healthy-host
warmup from a probe plus redundant warm command to one network round trip.

## Pane-tail recovery

Extend `spawn_remote_pane_tail` with an explicit `SshMux` argument. Initial
attach selects `Streaming`; recovery can select `Off` as an escape hatch.

Factor tail setup into a local pump object containing the SSH child, bounded
byte receiver, stdout task, and stderr task. Shutdown explicitly kills and reaps
the child and aborts/joins its pump tasks; `kill_on_drop` remains only a backstop.
That explicit cleanup result continues feeding `RemoteStreamRegistration`, so a
host edit never commits while an old-revision SSH child can survive.

When the receiver closes:

1. explicitly clean up the ended tail;
2. retry at most six times with 250 ms, 500 ms, 1 s, 2 s, 3 s, 3 s backoff;
3. for the first three attempts, reload the current saved host under its shared
   lifecycle lease and use the streaming master;
4. at the fourth attempt, evict the streaming master once and switch to
   `SshMux::Off`; remain unmultiplexed for later attempts;
5. sample the pane and log size in one remote command, spawn the replacement
   tail from that offset, then send RIS plus the snapshot before consuming
   buffered tail bytes.

Every sleep, host reload, snapshot, spawn, and client send observes the stream's
cancellation receiver. Cancellation ends recovery and cleanup without opening
the preceding credential revision. Exhaustion emits one bounded interruption
message and returns control to the client's existing reconnect behavior.

## Input recovery and large frames

Rename `encode_input_hex_line` to `encode_input_hex_lines`. Split source bytes at
`agentum_tmux::SEND_KEYS_HEX_CHUNK_BYTES`; each chunk becomes one newline-ended
space-separated hex command. This shares the same tmux-safe bound as local and
one-shot input and preserves ordering for arbitrary WebSocket frame sizes.

The input task retains the current host-reload/cancellation discipline. Before
each batch, it attempts to recreate the persistent writer when absent. A write
and flush is wrapped in a three-second deadline and cancellation select:

- success marks the batch delivered on the fast persistent path;
- I/O failure or timeout explicitly kills/reaps the writer and clears it;
- the same bytes immediately use `send_bytes`, whose SSH operation is already
  bounded and safely retries only proven pre-session mux failures unmultiplexed;
- the next batch attempts to reopen the persistent path.

The protocol has no remote acknowledgement, so a timeout has a narrow
at-least-once ambiguity if the bytes reached tmux just before the path became
unknowable. Availability is preferable to permanent input loss/freeze; the
decision record makes this trade-off explicit.

Queue saturation remains non-blocking to protect the WebSocket task, but every
rejected binary or text input frame is logged with the target instead of being
silently discarded.

## Idle channel-load reduction

Remote pane titles still require polling because tmux consumes OSC title bytes.
Track whether pane bytes arrived since the last title query. A normal 2.5-second
tick skips the SSH call when there was no activity; every second skipped tick
performs a safety query (about five seconds) for rare title-only changes. Any
tail byte re-arms the next poll. This removes avoidable interactive-master
channels for idle panes without delaying active status changes.

## Host mutation and cleanup ordering

The existing host lifecycle lock and remote-stream registry stay authoritative:

- stream registration occurs while holding the shared host lease;
- reconnect and writer recreation acquire a fresh shared lease and reload the
  persisted `Host` before spawning;
- host PUT/DELETE takes the exclusive lease, signals cancellation, waits for the
  tail and writer cleanup acknowledgment, and only then retires old masters;
- recovery never holds the host lease across backoff sleeps or WebSocket sends.

This ordering prevents both stale credentials and deadlocks between mutation,
warmup, and reconnect.

## Failure boundaries

| Failure | Recovery | Bound |
|---|---|---|
| Healthy/cold pooled master | Real no-op also warms it | One SSH round trip |
| Wedged pooled master | Exact-role `-O exit`, one reopen | Probe + exit + reopen deadlines |
| Tail EOF/channel refusal | Snapshot, respawn, repaint | Six attempts, capped backoff |
| Repeated pooled tail failure | Evict streaming master, use mux-off | One eviction per recovery cycle |
| Persistent input stall | Kill/reap, per-exec fallback | Three-second write deadline |
| Input queue full | Log explicit drop | Immediate |
| Host mutation | Cancel and acknowledge child cleanup | Existing five-second mutation bound |

## Verification design

- Pure tests cover health classification, tail backoff/mux escalation, idle
  title-poll gating, and lossless long-input chunking at the shared bound.
- SSH builder tests preserve exact host/revision ControlPaths, role-specific
  exit targeting, auth flags, askpass cleanup, and safe mux-off behavior.
- Server tests preserve host lifecycle cancellation and explicit child reaping.
- Focused crate tests run before format, all-target workspace check, and the
  policy-configured package test checkpoint.
- Independent review checks every acceptance criterion, cleanup path, replay
  boundary, and secret invariant against both the local diff and sibling fixes.
- A live test uses only the repository's existing opt-in SSH seam; evidence is
  reduced to pass/fail timing and lifecycle outcomes with credentials redacted.
