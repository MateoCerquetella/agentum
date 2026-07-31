## Purpose

Agentum source intake safely converts external specification formats into
immutable, provider-neutral authoring context without granting them lifecycle
or delivery authority.

## ADDED Requirements

### Requirement: Empirical features are first-class local sources

Agentum MUST expose Empirical protocol 0.20 schema-4 feature directories as a
previewable New Spec source and MUST implement the importer without executing
or depending on the Empirical runtime.

#### Scenario: User previews an Empirical feature

- **WHEN** a user selects Empirical and supplies a valid repository-relative
  `.empirical/specs/<feature>` path
- **THEN** Run Center displays the exact source revision, capability count,
  design availability, task count, and diagnostics before creation

### Requirement: Empirical intent retains stable provenance

Agentum MUST preserve the feature contract and every supported capability
operation, requirement, and scenario as deterministic authoring context.

#### Scenario: Capability deltas are imported

- **WHEN** a feature contains valid ADDED, MODIFIED, or REMOVED Requirement and
  Scenario blocks
- **THEN** the normalized source names their capability, operation,
  requirement, and scenario without silently discarding their meaning

### Requirement: Empirical intake is immutable and fail-closed

Agentum MUST use bounded no-follow anchored reads, MUST reject unsafe or
unsupported inputs without mutation, and MUST bind creation to the exact
previewed source revision.

#### Scenario: Source changes after preview

- **WHEN** an Empirical artifact changes after preview but before creation
- **THEN** creation returns `source_revision_changed` before allocating any
  Agentum specification, worktree, attempt, approval, or run

#### Scenario: Source path is unsafe

- **WHEN** a source reference traverses, is absolute, uses a symlink, exceeds a
  bound, or declares an unsupported schema
- **THEN** the request fails with an actionable source error and neither the
  repository nor Agentum's database is changed

### Requirement: Agentum remains lifecycle authority

Agentum MUST treat Empirical files as read-only source material and MUST retain
its existing provider isolation, approvals, evidence, Ready, and Deliver
contracts.

#### Scenario: Import succeeds

- **WHEN** a valid Empirical preview is used to create a New Spec
- **THEN** Agentum authors and runs its canonical workflow without executing,
  installing, updating, or mutating Empirical runtime state
