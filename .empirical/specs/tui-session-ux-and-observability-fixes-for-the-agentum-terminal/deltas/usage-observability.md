# Usage Observability

## Purpose

Expose current, source-qualified account and session usage while making stale
or unavailable data explicit.

## ADDED Requirements

### Requirement: Usage panel distinguishes account and session data

The Usage panel SHALL show the active daemon's Claude account snapshot
separately from aggregates and metrics derived from running Agentum sessions.

#### Scenario: Render live usage

- **GIVEN** `/api/usage/claude` returns a fresh OAuth-enriched snapshot and
  running sessions contain token/cost metrics
- **WHEN** the Usage panel renders
- **THEN** it labels the account-wide Claude window separately from per-tool
  aggregates and per-session rows
- **AND** it displays source and freshness without presenting estimates as
  exact billing data

#### Scenario: First fetch happens immediately

- **GIVEN** the authenticated TUI run loop has started
- **WHEN** the Usage panel first becomes visible
- **THEN** an authenticated usage request has already been started without
  waiting for the periodic refresh interval

#### Scenario: Refresh fails after a good snapshot

- **GIVEN** the panel has a last good usage snapshot
- **WHEN** a later refresh fails or times out
- **THEN** the last good values remain visible and are marked stale
- **AND** the in-flight guard is cleared so a later refresh can retry
- **AND** a copyable diagnostic identifies the failed operation and daemon

#### Scenario: No usable account source exists

- **GIVEN** Claude is absent, OAuth is unavailable, transcripts cannot be read,
  the daemon is unreachable, or the daemon lacks the route
- **WHEN** the panel renders
- **THEN** it shows the corresponding explicit state rather than a blank header,
  zero percentage, or indefinitely loading panel

### Requirement: Claude usage endpoint degrades safely

The authenticated Claude usage endpoint SHALL collect from the daemon host,
redact credentials, and return a source-qualified snapshot even when OAuth
enrichment is unavailable.

#### Scenario: OAuth enrichment succeeds

- **GIVEN** the daemon can read a current Claude OAuth credential and the
  upstream usage service responds
- **WHEN** `/api/usage/claude` is requested
- **THEN** the response contains the available five-hour/seven-day utilization,
  binding reset time, source, and locally scanned token/cost fields

#### Scenario: OAuth enrichment is unavailable

- **GIVEN** no credential exists or the upstream request fails
- **WHEN** `/api/usage/claude` is requested
- **THEN** the route still returns a successful local-scan snapshot when local
  data is readable
- **AND** it does not invent a utilization percentage or expose a credential
