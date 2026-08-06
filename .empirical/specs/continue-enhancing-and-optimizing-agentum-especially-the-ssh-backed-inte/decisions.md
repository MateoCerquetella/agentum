# Decisions: Continue Enhancing And Optimizing Agentum Especially The Ssh Backed Inte

Record concise, externally reviewable evidence and choices here. Do not store
private chain-of-thought, prompts, credentials, secrets, or scratchpad text.

## D-001: Isolate monitoring and remove avoidable interactive buffering

Status: Accepted

### Evidence

- The terminal WebSocket connector currently leaves TCP Nagle enabled.
- Its bidirectional pump prioritizes outbound key events over inbound output.
- Pane liveness probes currently ride the same streaming ControlMaster whose
  failure they must detect.
- The installed executable predates the already-reviewed image-upload fix.

### Options

1. Keep two SSH pools and probe the streaming pool through itself.
2. Move probes onto the interactive pool.
3. Add a low-volume observer pool, retain compressed streaming, disable
   compression/Nagle on latency-sensitive paths, and make the WebSocket pump
   fair.

### Chosen approach

Choose option 3. It removes circular liveness observation without competing
with keystrokes, while preserving the existing streaming throughput design.

### Trade-offs and risks

One additional persistent SSH connection may exist per active saved host. It is
low-volume and bounded by the same host revision, keepalive, eviction, and
cleanup rules as the other pools. Speculative local echo remains out of scope,
so physical network RTT still establishes the lower bound for visible echo.

### Verification

Pin role-specific control paths, cleanup of all current roles, role-specific
compression, Nagle-disabled WebSocket construction, fair pump scheduling, and
independent liveness-probe routing in deterministic tests.
