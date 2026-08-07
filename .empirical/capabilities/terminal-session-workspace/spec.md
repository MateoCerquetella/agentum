# Terminal Session Workspace Specification

## Purpose

Define how users create and operate multiple Agentum terminal sessions inside
one keyboard-driven TUI workspace.

## Requirements

### Requirement: Add a terminal or agent from the active session

The terminal workspace SHALL expose a discoverable action from an active
session that creates either a plain terminal session or an installed agent
session and presents it in the auxiliary terminal pane without replacing the
original session.

#### Scenario: Add a plain terminal beside an agent

- **GIVEN** a running agent session is visible in the primary terminal pane
- **WHEN** the user chooses the add-pane action and selects plain terminal
- **THEN** Agentum creates and starts an independent terminal session on the
  same daemon and host with the same working directory by default
- **AND** the original agent remains visible in the primary pane
- **AND** the new terminal is selected and focused in the auxiliary pane

#### Scenario: Add an installed agent

- **GIVEN** Claude and Codex are reported available on the active session's
  target host
- **WHEN** the user chooses either tool in the add-agent flow
- **THEN** Agentum creates and starts that tool through the standard session
  lifecycle path
- **AND** the new session appears in the session tree and auxiliary pane

#### Scenario: Start fails after create

- **GIVEN** the session record is created successfully
- **WHEN** its target process cannot be started
- **THEN** the session remains visible in a recoverable non-running state
- **AND** Agentum reports both the successful create and failed start in one
  actionable diagnostic

### Requirement: Keyboard focus and pane resizing

The terminal workspace SHALL provide keyboard-only forward/backward focus and
bounded resizing for every visible terminal pane while preserving ordinary
terminal input.

#### Scenario: Cycle focus across a split

- **GIVEN** the tree, primary terminal, and auxiliary terminal are visible
- **WHEN** the user presses `F5` repeatedly
- **THEN** focus advances through each visible focus target and wraps
- **WHEN** the user presses `F6`
- **THEN** focus traverses the same targets in reverse

#### Scenario: Resize a split

- **GIVEN** two terminal panes are visible
- **WHEN** the user presses `Ctrl-Shift-Left` or `Ctrl-Shift-Right`
- **THEN** the boundary moves by a bounded step without collapsing either pane
- **AND** both terminal streams receive their resulting dimensions
- **AND** the ratio is restored on the next launch

#### Scenario: Close the focused auxiliary pane

- **GIVEN** the auxiliary terminal has focus
- **WHEN** the user closes the split
- **THEN** the auxiliary stream is stopped and focus moves to the primary
  terminal
- **AND** the underlying auxiliary Agentum session is not killed implicitly

### Requirement: Create-session key binding

The session tree SHALL use uppercase `C` as the direct create-session binding
and SHALL no longer use `n` for that action.

#### Scenario: Create from tree focus

- **GIVEN** the session tree has focus and no modal owns input
- **WHEN** the user presses uppercase `C`
- **THEN** the new-session form opens
- **AND** visible help, hint, and command-palette copy advertise `C`

#### Scenario: Preserve lowercase card hint and terminal input

- **GIVEN** a card-bound session is selected in the tree
- **WHEN** the user presses lowercase `c`
- **THEN** the existing card-hint action runs
- **GIVEN** a terminal pane has focus
- **WHEN** the user types uppercase `C`
- **THEN** the byte is forwarded to that terminal rather than opening a form
