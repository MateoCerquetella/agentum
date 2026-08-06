# Decisions: Fix The Connectivity Issue That Makes Agentum Tui Unusable Compare

Record concise, externally reviewable evidence and choices here. Do not store
private chain-of-thought, prompts, credentials, secrets, or scratchpad text.

## D-001: Select the implementation approach

Status: Accepted

### Evidence

- The local baseline passes 73 `agentum-tmux` tests and 402
  `agentum-server --lib` tests, so the reported unusability is not a compile
  failure.
- This tree already has private record/revision-namespaced ControlPaths,
  noninteractive askpass secret files, 10-minute ControlPersist, separate
  interactive/streaming masters, persistent input, and one-call snapshot/pipe
  setup.
- Its warmer uses `ssh -O check`, which only confirms the local master process;
  it can accept a master whose remote TCP path is silently dead.
- Its remote tail ends the WebSocket on EOF, its persistent input write has no
  deadline or restoration path, and one input frame becomes one unbounded
  remote `tmux send-keys` command.
- The sibling `agentum` project fixes the same symptoms in commits `ec9e7d39`
  (stream/input self-heal), `925dea0c` (wedged-master eviction and bounded
  input), `80ecfb3f` (lossless large-paste chunking), and `3f22a5a4` (idle title
  poll suppression). It also contains a behavior-preserving module extraction
  in `67bd0c06`.

### Options

1. Replace the divergent SSH/server files wholesale with the sibling refactor.
2. Port the proven connectivity behaviors onto this tree's hardened transport
   and factor small recovery helpers without changing public module ownership.
3. Add retries only at the WebSocket/client layer and leave the pooled masters
   and persistent writer unchanged.

### Chosen approach

Choose option 2. Port the sibling's bounded tail recovery, pooled-to-unmuxed
escape, real health probes, input timeout/restoration, input chunking, and idle
poll suppression. Adapt them to the local stream registry, host lifecycle lock,
host-revision reload, exact tmux identity, explicit child reaping, and stricter
askpass/ControlPath safety. Keep the existing public API except where selecting
the tail mux is required.

### Trade-offs and risks

- A full file copy is simpler but would remove newer local secret and host-edit
  protections; the adapted port costs more focused code review but preserves
  those invariants.
- Reconnecting forever would hide a dead host and leak work; use a finite retry
  count, bounded exponential backoff, and existing client reconnect behavior.
- Evicting a healthy shared master disrupts other sessions; evict only timeouts
  or recognized pre-session mux failures and preserve other nonzero outcomes.
- An input timeout can duplicate bytes if delivery happened just before the
  transport became unknowable. The persistent protocol has no acknowledgement,
  so prefer availability and document the narrow at-least-once fallback risk;
  cap the timeout and keep ordering deterministic.
- Structural extraction beyond touched helpers increases merge risk without
  improving runtime behavior, so the sibling's refactor informs boundaries but
  is not copied mechanically.

### Verification

- Unit-test reconnect plans/backoff, mux selection, master eviction builders,
  input timeout outcomes, long-paste chunking, and idle poll decisions.
- Preserve and run the hardened SSH/auth/host-revision regression suites.
- Run format, workspace compile, policy-configured package tests, and an
  independent acceptance/security review.
- Use the repository's own saved-host/live seam when available and keep all
  credentials and host details out of evidence output.
