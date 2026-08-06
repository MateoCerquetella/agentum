# Decisions: Host-Aware Image Paste

## D-001: Route the whole upload transaction through the saved host

Status: Accepted

### Evidence

- The current route calls local-only `agentum_tmux::has_session`, local filesystem
  APIs, and local-only `agentum_tmux::send_keys` without reading `session.host_id`.
- Server lifecycle/start/stream routes already establish host-aware exact-target
  helpers and canonical host-to-session lock ordering.

### Options

1. Skip the tmux check for SSH sessions but keep local file writing.
2. Copy images to the SSH host after a local upload transaction.
3. Execute validation, file write, and path input directly on the saved host.

### Chosen approach

Choose option 3 and hold canonical lifecycle leases across all three operations.

### Trade-offs and risks

- The host lease covers the upload transfer, delaying a concurrent host edit;
  this is required to prevent credential-revision mixing.
- Final input failure may leave a private orphan image, as before, but never a
  false success event.

### Verification

- Review every operation for host routing and failure-before-success ordering.
- Exercise exact remote target resolution and local compatibility tests.

## D-002: Generalize the existing secure writer to raw bytes

Status: Accepted

### Evidence

- `write_remote_file` already provides atomic temp-file replacement, 0600 mode,
  safe parent/destination guards, and SSH-stdin content transport.
- Images are arbitrary bytes and can exceed the current launch-oriented timeout.

### Options

1. Base64 the image into a remote command.
2. Add a separate upload shell protocol.
3. Extract the existing writer's byte implementation and retain the string API
   as a compatibility wrapper.

### Chosen approach

Choose option 3. Add a public byte writer with an upload-sized bound; keep the
credential string writer's existing API and shorter timeout.

### Trade-offs and risks

- Large writes hold one SSH exec and lifecycle lease longer, bounded by timeout.
- Reusing hardened guards is more code-coupled but avoids divergent path safety.

### Verification

- Prove exact binary preservation locally and stdin-only remote transport.
