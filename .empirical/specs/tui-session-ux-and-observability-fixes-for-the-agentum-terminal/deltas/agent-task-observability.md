# Agent Task Observability

## Purpose

Keep each supported agent session's plan, todos, and background task state
correct, isolated, and visibly diagnosable across local and SSH hosts.

## ADDED Requirements

### Requirement: Claude plan, todo, and task state follows its session

The daemon SHALL derive supported Claude plan, todo, and background-agent state
from the transcript belonging to the requested Agentum session, including when
multiple sessions share a working directory.

#### Scenario: Initial read populates without a new file event

- **GIVEN** a supported Claude transcript already contains plan and task records
- **WHEN** `/api/sessions/{id}/agent-tasks` is first requested
- **THEN** the response includes the parsed current state without waiting for a
  subsequent filesystem notification

#### Scenario: Appended records update the state

- **GIVEN** a watcher or reconciler is active for a Claude session
- **WHEN** complete JSONL records for `ExitPlanMode`, `TodoWrite`, `TaskCreate`,
  `TaskUpdate`, completion/deletion, or supported subagent dispatch are appended
- **THEN** the cached state updates and clients are notified or converge on the
  next reconciliation poll

#### Scenario: Same-workdir sessions remain isolated

- **GIVEN** two modern Claude sessions share a workdir and each writes a
  transcript pinned to its Agentum session id
- **WHEN** either transcript changes
- **THEN** only that session's state changes

#### Scenario: Session runs on an SSH host

- **GIVEN** a Claude session belongs to an SSH host controlled by the daemon
- **WHEN** its task state is requested
- **THEN** transcript discovery and reads occur on that host rather than against
  a same-shaped local path
- **AND** read failures identify the host and do not substitute another local
  transcript

#### Scenario: Transcript lifecycle changes

- **GIVEN** a transcript is created after watching starts, replaced, truncated,
  or survives a daemon restart
- **WHEN** the next event or reconciliation occurs
- **THEN** parsing resumes from a safe cursor or rebuilds from the correct file
  without duplicating, losing, or cross-pollinating task state

#### Scenario: Agent context is cleared

- **GIVEN** cached plan/task state exists
- **WHEN** the session records `/clear` or `/compact`, or the reset endpoint is
  called
- **THEN** the cached state is cleared and pre-reset records are not replayed

### Requirement: Agent task panel reports source state honestly

The TUI and agent-task API SHALL distinguish current empty data from
unsupported tools, missing transcripts, host read failures, stale cached data,
and incompatible daemons.

#### Scenario: Supported session has no tasks

- **GIVEN** a readable supported Claude transcript contains no recognized plan,
  todo, or agent-task records
- **WHEN** the panel renders
- **THEN** it shows a current empty-state message

#### Scenario: Tool parser is unsupported

- **GIVEN** the selected session uses a tool without a transcript parser
- **WHEN** the panel renders
- **THEN** it identifies the tool as unsupported instead of showing the same
  empty state as a current Claude session

#### Scenario: Refresh fails after good data

- **GIVEN** the panel has a last good session snapshot
- **WHEN** a later API or host read fails
- **THEN** the snapshot remains visible with a stale marker
- **AND** the user can copy a diagnostic containing the session, daemon, host,
  version, and relevant log/transcript context

#### Scenario: Focus changes the observed session

- **GIVEN** primary and auxiliary panes show different Agentum sessions
- **WHEN** focus moves between those panes
- **THEN** the Plan / Todos / Agents panel shows the state of the focused pane's
  session and never the other pane's cached state
