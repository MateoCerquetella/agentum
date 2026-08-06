# Remote Session Reliability Delta

## MODIFIED Requirements

### Requirement: Responsive pooled SSH interaction

Agentum MUST reuse healthy pooled SSH connections for interactive work, MUST
detect and retire a genuinely wedged pool, MUST recover a silent pane-output
channel without waiting for user input, and MUST avoid background channel
traffic that competes unnecessarily with user input.

#### Scenario: Persistent pane-output channel silently wedges

- **Given** an attached SSH-backed session whose pane-output child remains alive
  but no longer forwards newly appended pane bytes
- **When** no keyboard input is sent
- **Then** Agentum detects the lack of transport progress within a bounded interval
- **And** it validates or retires only the streaming master
- **And** it re-synchronizes the pane snapshot and log offset before resuming output
- **And** the interactive master and accepted input remain undisturbed

#### Scenario: Remote pane is healthy and idle

- **Given** an attached SSH-backed session with no new pane output
- **When** its output-liveness interval elapses
- **Then** Agentum distinguishes idle pane state from a wedged transport using a
  bounded, low-frequency check
- **And** it preserves the healthy streaming master and tail
- **And** it does not require keyboard input or generate high-frequency SSH traffic

#### Scenario: Background status work runs during interaction

- **Given** a remote session receiving terminal input or pane output
- **When** title, status, or liveness maintenance becomes due
- **Then** maintenance does not block WebSocket input/output dispatch
- **And** it does not consume the interactive master's channel budget unless the
  operation is intrinsically interactive
