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

Agentum MUST reuse healthy pooled SSH connections for interactive work, MUST
detect and retire a genuinely wedged pool, and MUST avoid background channel
traffic that competes unnecessarily with user input.

#### Scenario: Application reload finds an orphaned master

- **Given** an Agentum-owned ControlMaster socket whose remote TCP path is dead
- **When** the boot or periodic warmer probes it with a real remote no-op
- **Then** Agentum evicts and reopens that exact master
- **And** a successful probe itself satisfies warmup without another handshake

#### Scenario: Shared master is healthy or merely busy

- **Given** a pooled master shared by active remote sessions
- **When** its health probe succeeds or returns a non-mux channel-pressure error
- **Then** Agentum preserves the master instead of disrupting other sessions

#### Scenario: Remote session is idle

- **Given** no pane output has arrived since the last title poll
- **When** the next normal title-poll tick fires
- **Then** Agentum skips the SSH round trip
- **And** a bounded periodic safety tick still checks for a title-only change

#### Scenario: User pastes a large payload

- **Given** one WebSocket input frame exceeds tmux's safe marshalled command size
- **When** Agentum sends it through the persistent SSH writer
- **Then** it splits the bytes into lossless tmux-safe input lines
- **And** all bytes remain in order
