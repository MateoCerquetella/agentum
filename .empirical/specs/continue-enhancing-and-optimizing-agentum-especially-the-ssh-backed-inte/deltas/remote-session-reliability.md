# Remote Session Reliability Delta

## MODIFIED Requirements

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
