# Remote Session Reliability Specification

## Purpose

Define reliable, secret-safe, host-aware lifecycle behavior for Agentum sessions
that run through saved SSH hosts.

## Requirements

### Requirement: Authenticated remote agent launch

Agentum MUST launch supported agents on a ready SSH host with configuration
accepted by that agent. Secrets MUST use a supported secret channel and MUST
NOT be embedded in process argv or user-visible errors.

#### Scenario: Codex uses the Agentum MCP tunnel

- **Given** a ready SSH host and authenticated Agentum MCP reverse tunnel
- **When** the user starts a Codex session
- **Then** Codex receives the MCP URL and bearer-token env-var reference
- **And** the referenced env var contains the token
- **And** the literal token is absent from argv

### Requirement: Host-aware remote lifecycle

Every start or resume of an SSH-owned session MUST validate and launch on its
saved host, using that host's filesystem, compatible tmux, and shell.

#### Scenario: Agentum resumes a remote session

- **Given** an idle or stopped session whose workdir exists only remotely
- **When** Agentum performs startup resume
- **Then** it tests and starts that session through its saved SSH host
- **And** it does not inspect or create the remote path locally

#### Scenario: User starts a remote terminal

- **Given** the daemon and remote host use different login shells
- **When** a terminal session starts remotely
- **Then** the command resolves a usable shell on the remote host

### Requirement: Remote startup diagnostics

Agentum MUST preserve and classify the earliest actionable failure instead of
replacing it with a secondary tmux target error.

#### Scenario: Remote agent exits immediately

- **Given** SSH/tmux setup succeeds
- **When** the agent rejects configuration and exits immediately
- **Then** Agentum reports bounded, redacted pane output describing that failure
- **And** it cleans up partial session state

#### Scenario: Remote prerequisite is absent

- **Given** SSH succeeds
- **When** the workdir, selected binary, or required tmux capability is absent
- **Then** start is rejected before agent launch with the missing prerequisite

### Requirement: Readable TUI errors

The TUI MUST render complete lifecycle errors within the overlay width while
retaining its existing navigation and clear/close controls.

#### Scenario: Start returns a multi-line remote error

- **Given** start returns a structured or multi-line diagnostic
- **When** the error overlay opens
- **Then** the diagnostic wraps into visible rows rather than clipping one line

### Requirement: Self-healing remote session transport

Agentum MUST recover remote pane output and input from transient or stale pooled
SSH transport failures within bounded time. Recovery MUST preserve the current
saved-host revision and MUST NOT silently discard, duplicate, replace, or
decorate accepted input with transport framing or implementation labels.

#### Scenario: Typed UTF-8 input crosses the session transport

- **Given** a local or SSH-backed session is selected in the terminal pane
- **When** the user types printable ASCII or multi-byte Unicode text
- **Then** the target process receives the exact ordered UTF-8 bytes once
- **And** transport labels such as `Bytes` or `BYTES` are absent

#### Scenario: Persistent input channel wedges

- **Given** a connected remote session whose persistent input SSH channel stops
  draining
- **When** the user sends input
- **Then** the write is bounded and the wedged child is killed and reaped
- **And** the same input is attempted through the bounded per-exec path
- **And** later input attempts to restore the fast persistent channel

#### Scenario: User pastes structured text

- **Given** the focused pane receives bracketed paste containing spaces,
  newlines, punctuation, and non-ASCII characters
- **When** Agentum frames and delivers that paste
- **Then** every input byte reaches the selected pane in order
- **And** no serializer name, debug representation, or control envelope is
  delivered as user text

### Requirement: Responsive pooled SSH interaction

Agentum MUST keep keystrokes, pane-output streams, and liveness observation on
independent saved-host/revision-scoped SSH pools. Interactive paths MUST avoid
avoidable small-packet buffering, output MUST remain serviceable during
sustained input, and host invalidation MUST retire every pool role.

#### Scenario: User types while the agent emits output

- **Given** an SSH-backed terminal is receiving sustained key events and pane output
- **When** the bidirectional WebSocket pump services both directions
- **Then** outbound input and inbound output both continue making progress
- **And** latency-sensitive connections do not retain Nagle or bulk compression buffering

#### Scenario: Pane streaming pool silently wedges

- **Given** the pane log continues growing but its streaming SSH child forwards no bytes
- **When** the next observer-pool liveness checks run
- **Then** Agentum detects persistent lag without relying on that streaming pool
- **And** repairs only the streaming role and resumes from an atomic offset
- **And** no keypress is required

#### Scenario: Saved host changes

- **Given** a host has interactive, streaming, and observer masters
- **When** that host is edited or deleted
- **Then** Agentum retires all three exact revision-scoped masters before committing the mutation

### Requirement: Host-aware session uploads

Agentum MUST ship the verified host-aware upload implementation in the runnable
binary used for SSH-backed sessions, preserving private atomic remote writes and
exact-pane path injection.

#### Scenario: User pastes an image after updating Agentum

- **Given** the installed binary contains the reviewed host-aware upload route
- **When** Ctrl-V obtains an image and uploads it for an SSH-backed session
- **Then** the image is written only below the saved remote workdir
- **And** its safe relative path is typed once into the exact remote pane
