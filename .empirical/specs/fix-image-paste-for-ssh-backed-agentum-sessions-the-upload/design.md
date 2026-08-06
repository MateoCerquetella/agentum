# Design: Host-Aware Image Paste

## Diagnosis

`routes/uploads.rs` ignores `session.host_id`. It calls local-only tmux helpers,
expands the workdir with the daemon's home, creates/writes a local file, and
sends the path locally. Every SSH session therefore fails at the first local
tmux check (or, worse, could collide with an unrelated local target).

## Transaction

1. Resolve the preliminary session only to identify its immutable host binding.
2. Acquire host then session lifecycle leases in the repository's canonical
   order, reload the session and saved host under those leases, and retain both
   through validation, write, and input injection.
3. Require the persisted effective workdir to be absolute; remote session create
   already resolves it against the SSH host.
4. Validate the exact target with `host_runtime::has_session`.
5. Generate the existing daemon-controlled safe relative filename and join it
   beneath the session workdir.
6. Write raw bytes through a new host-aware private atomic byte-write helper.
   Local and SSH branches share the existing safe directory/destination guards;
   SSH content travels only on stdin and receives an upload-sized timeout.
7. Type the relative path with `host_runtime::send_keys`, then emit the existing
   event/response and resolve the optional clipboard request.

## Compatibility and failure ordering

Body and MIME validation remain before host I/O. Success signaling remains
after tmux validation, file write, and path injection. A failure may leave an
already-written private upload if the final pane send fails, matching the prior
local transaction, but it cannot report success or complete the broker.

## Verification

- Binary local byte-write test, including NUL/non-UTF-8 bytes and private mode.
- Remote atomic-write script/argv test proving content is stdin-only.
- Upload helper tests for absolute workdir containment and unchanged safe path.
- Existing exact remote tmux resolution, host lifecycle, clipboard, upload,
  event, MIME, size-limit, and workspace suites.
