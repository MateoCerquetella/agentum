# Fix Image Paste For Ssh Backed Agentum Sessions The Upload

## Request

> Fix image paste for SSH-backed Agentum sessions. The upload route currently checks tmux, creates .agentum-uploads, writes the image, and sends the relative path on the daemon's local machine, causing 'tmux session not active for this session'. Make the entire upload transaction host-aware and safe for both local and saved SSH sessions, preserving exact session/host/tmux identity and existing clipboard-broker behavior.

## Goal

Make image paste place the image in the selected session's actual workdir and
type its relative path into that session's actual tmux pane, whether the session
is local or owned by a saved SSH host.

## Acceptance Criteria

- [ ] [AC-1] Pasting an image into a running SSH-backed session validates the
  exact tmux target on its saved host instead of checking the daemon's local tmux.
- [ ] [AC-2] The uploaded bytes are written atomically as a private regular file
  under `<remote-workdir>/.agentum-uploads/`, and no local shadow file is created.
- [ ] [AC-3] The same safe relative path returned to the client is typed once,
  with a trailing space and no Enter, into the exact remote pane.
- [ ] [AC-4] Local-session image paste retains its existing behavior and response.
- [ ] [AC-5] Host deletion/edit and session lifecycle races cannot mix saved-host
  revisions, write to a different host, or target a reused tmux name.
- [ ] [AC-6] Empty/oversized bodies, unsafe paths/destinations, missing hosts,
  stopped panes, SSH failures, and write failures return an error without
  emitting a success event or completing the clipboard broker as uploaded.
- [ ] [AC-7] Clipboard request correlation, upload response fields, MIME-to-safe
  extension mapping, event payload, and the 25 MiB limit remain compatible.
- [ ] [AC-8] Deterministic tests cover local and SSH routing, exact target use,
  binary byte preservation, and failure ordering.

## Scope

- Session-scoped upload route host resolution and lifecycle locking.
- Host-aware exact tmux validation, private binary file write, and path input.
- Existing direct-upload and clipboard-broker callers.

## Non-goals

- Changing clipboard acquisition, image encoding, accepted MIME types, or UI.
- Adding image transformation, deduplication, cleanup, or cloud storage.
- Supporting a session whose persisted workdir is not absolute/validated.

## Verification

- Focused route/helper tests with fake SSH command transport where available.
- Existing upload, clipboard, host-runtime, tmux identity, and input tests.
- Configured serial all-target package checkpoint and code review.

## Capability Deltas

See `deltas/remote-session-reliability.md`.
