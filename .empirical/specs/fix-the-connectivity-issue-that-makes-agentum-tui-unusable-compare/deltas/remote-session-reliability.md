# Remote Session Reliability

## Purpose

Define reliable, secret-safe, host-aware lifecycle behavior for Agentum sessions
that run through saved SSH hosts.

## ADDED Requirements

### Requirement: Self-healing remote session transport

Agentum MUST recover remote pane output and input from transient or stale pooled
SSH transport failures within bounded time. Recovery MUST preserve the current
saved-host revision and MUST NOT silently discard accepted input.

#### Scenario: Streaming master becomes stale

- **Given** a connected remote session whose streaming ControlMaster no longer
  carries usable channels
- **When** the pane tail exits or a recovery attempt cannot use that master
- **Then** Agentum retries with bounded backoff while keeping the client stream
  alive
- **And** it evicts only the affected streaming master before using a fresh
  unmultiplexed escape connection
- **And** it re-snapshots and resumes from the matching pane-log offset

#### Scenario: Persistent input channel wedges

- **Given** a connected remote session whose persistent input SSH channel stops
  draining
- **When** the user sends input
- **Then** the write is bounded and the wedged child is killed and reaped
- **And** the same input is attempted through the bounded per-exec path
- **And** later input attempts to restore the fast persistent channel

#### Scenario: Host credentials change during recovery

- **Given** a remote stream is active while its saved host is edited
- **When** the host lifecycle cancels existing remote children
- **Then** recovery stops before reopening the old connection revision
- **And** cleanup acknowledges termination and reaping of its SSH children

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
