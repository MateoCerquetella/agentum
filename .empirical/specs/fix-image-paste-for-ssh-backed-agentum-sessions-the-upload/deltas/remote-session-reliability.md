# Remote Session Reliability Delta

## ADDED Requirements

### Requirement: Host-aware session uploads

Agentum MUST perform every session-scoped upload operation on the session's
saved host and exact tmux target. Uploaded bytes MUST remain private and MUST
NOT be written or injected on a different host when lifecycle state changes.

#### Scenario: User pastes an image into a remote session

- **Given** a running session owned by a saved SSH host
- **When** an image is uploaded for that session
- **Then** Agentum validates its exact tmux pane on the saved host
- **And** writes the image under that host's session workdir
- **And** types the returned relative image path once into that remote pane
- **And** performs no corresponding local tmux or filesystem mutation

#### Scenario: Remote upload cannot complete safely

- **Given** the saved host, workdir, destination, or exact pane is unavailable
- **When** an image upload is attempted
- **Then** Agentum returns an actionable failure
- **And** emits no successful upload event or clipboard completion
