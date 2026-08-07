# Diagnostics Specification

## Purpose

Make every operational failure safe to copy as a self-contained debugging
report with enough runtime context to act on it.

## Requirements

### Requirement: Copyable self-contained operational diagnostics

The TUI SHALL retain a complete diagnostic report for every user-visible
operational failure and SHALL let the user copy that report without truncation.

#### Scenario: Copy a session-scoped failure

- **GIVEN** a start, stop, create, stream, task, usage, or host operation fails
  for a session
- **WHEN** the user opens the error detail and invokes its copy action
- **THEN** the clipboard contains the operation and original error chain
- **AND** it contains the session id, TUI version, daemon endpoint/state/version,
  target host identity/readiness, and the relevant resolved log path
- **AND** missing values are explicitly labeled unknown

#### Scenario: Preserve multiline detail

- **GIVEN** an underlying error contains multiple lines or a nested cause chain
- **WHEN** the report is viewed or copied
- **THEN** the stored and copied report preserves the complete content even if
  the error-list preview is visually shortened

#### Scenario: Redact secrets

- **GIVEN** an upstream error includes an authorization header, bearer token,
  password, or stored credential value
- **WHEN** Agentum builds, displays, logs, or copies the diagnostic
- **THEN** the secret value is replaced by a redaction marker
- **AND** the remaining debugging context is preserved

### Requirement: Diagnostic log locations are actionable

Diagnostic reports SHALL identify the actual log a user can inspect for the
failed surface instead of referring generically to "the logs."

#### Scenario: TUI failure has a file log

- **GIVEN** the TUI tracing log was initialized successfully
- **WHEN** a TUI or daemon-client operation fails
- **THEN** the report includes the resolved path corresponding to the active
  `$XDG_CACHE_HOME/agentum/tui.log` location

#### Scenario: Session pane has an output log

- **GIVEN** the failure concerns a session with a local or SSH pane log
- **WHEN** the diagnostic is built
- **THEN** it includes the session pane log location appropriate to that host
