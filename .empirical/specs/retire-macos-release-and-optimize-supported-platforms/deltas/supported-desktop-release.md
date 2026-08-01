## Purpose

Agentum publishes a bounded, verifiable desktop release for the platforms the
project actively supports, and unsupported platforms cannot enter artifacts,
updater metadata, installation instructions, or publication dependencies.

## ADDED Requirements

### Requirement: Desktop release targets are explicit and bounded

Agentum MUST build packaged releases only for Linux x86_64 and Windows x86_64,
MUST validate each target on its native GitHub runner, and MUST fail before
draft creation if a macOS or Apple-target payload is staged.

#### Scenario: A supported patch release is assembled

- **WHEN** all matrix builds complete for a version-matched protected release
- **THEN** aggregation accepts the complete Linux and Windows artifact roster
  and rejects any `.dmg`, `.app`, `apple-darwin`, or `macos-*` payload

#### Scenario: A retired target is reintroduced

- **WHEN** a workflow or staged artifact contains a macOS runner, Apple target,
  DMG, app archive, or Darwin updater key
- **THEN** the release regression contract or aggregation gate fails before a
  public release can be created

### Requirement: Updater metadata names only supported platforms

Agentum MUST publish exactly `linux-x86_64` and `windows-x86_64` in
`latest.json`, and each version-matched artifact MUST have a detached signature
that verifies against the embedded updater public key.

#### Scenario: A client checks for an update

- **WHEN** a supported packaged client reads the live latest-release endpoint
- **THEN** its platform entry resolves to the public version-matched artifact
  and carries the verified detached signature

#### Scenario: A retired Mac client checks for an update

- **WHEN** the updater manifest is read for a Darwin platform
- **THEN** no Darwin entry or macOS artifact URL is available

### Requirement: Unsupported packaged installation fails clearly

Agentum MUST document Linux and Windows as its packaged desktop platforms and
MUST reject Darwin in the release installer before constructing or downloading
an asset URL.

#### Scenario: The release installer runs on macOS

- **WHEN** platform detection identifies Darwin
- **THEN** installation stops with a clear unsupported-platform error and does
  not download, mount, copy, or approve an application bundle

### Requirement: Publication preserves supported-platform trust gates

Agentum MUST retain version/tag/main authority, updater-signature verification,
restricted-content scans, exact checksums, private draft staging, source-package
validation, and immutable final publication while removing macOS-only and
Homebrew-only work.

#### Scenario: A required supported artifact is missing

- **WHEN** Linux or Windows output, its updater signature, or another required
  release asset is absent or fails validation
- **THEN** the workflow stops before the private draft can become public

#### Scenario: Every supported gate passes

- **WHEN** the exact protected tag produces the complete verified roster
- **THEN** the draft is published once and a public audit reproduces its
  checksums, updater signatures, URLs, version, and supported platform set
