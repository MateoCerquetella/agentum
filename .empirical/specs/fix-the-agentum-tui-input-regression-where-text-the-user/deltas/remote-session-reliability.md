# Remote Session Reliability

## Purpose

Define reliable, secret-safe, host-aware lifecycle behavior for Agentum sessions
that run through saved SSH hosts.

## MODIFIED Requirements

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
