# Plan

## 1. Add binary-safe host file writing

- [ ] Extract the existing secure local/SSH writer around `&[u8]` while keeping
  the string API and credential timeout compatible.
- [ ] Add an upload-sized bounded byte-write entry point.
- [ ] Test non-UTF-8/NUL preservation, private destination mode, atomic path
  guards, and stdin-only SSH command construction.

## 2. Make the upload transaction host-aware

- [ ] Acquire canonical host/session lifecycle leases and reload both records.
- [ ] Require an absolute persisted workdir and build the safe destination below
  `.agentum-uploads` without daemon-home expansion for SSH sessions.
- [ ] Replace local tmux validation, file write, and path injection with
  host-runtime equivalents while preserving no-Enter semantics.
- [ ] Keep event, response, and clipboard completion strictly after success.

## 3. Verify and integrate

- [ ] Run focused upload, clipboard, host-runtime, session, and tmux tests.
- [ ] Run formatting, compile checks, and configured serial workspace tests.
- [ ] Review host revision/lock ordering, exact target identity, binary/path
  safety, failure ordering, and compatibility.
- [ ] Record evidence and integrate the capability delta independently.
