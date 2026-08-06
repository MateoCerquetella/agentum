# Remote Session Reliability

## Purpose

Define reliable, secret-safe, host-aware lifecycle behavior for Agentum sessions
that run through saved SSH hosts.

## ADDED Requirements

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
