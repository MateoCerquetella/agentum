# Implementation Plan

1. Extend `agentum-tmux::SshMux` with an observer role, distinct control path,
   current-master cleanup, role-specific compression, and deterministic tests.
2. Route remote pane state probes through the observer pool and include it in
   host warmup/repair without changing the streaming-tail recovery algorithm.
3. Enable TCP_NODELAY for terminal WebSockets and make their established
   bidirectional pump fair; add source-level/unit regression coverage where the
   connector API cannot be instantiated without a socket.
4. Re-run focused upload, clipboard, remote input, tail progress, SSH command,
   and WebSocket tests; repair only failures within the approved scope.
5. Run formatting and the configured full workspace verification, collect
   immutable test and review evidence, and complete independent integration.
6. Install the verified workspace executable to the existing user-local binary
   location and confirm it is newer than the stale August 4 executable.
