# Retire Macos Release And Optimize Supported Platforms

## Request

> Immediately remove darwin-aarch64 and darwin-x86_64 from the live v0.98.11 updater manifest so no additional Macs receive the broken update. Retire macOS from all future Agentum releases because the owner will not use or support it: remove macOS build matrix jobs, DMGs, app updater artifacts, macOS signing/notarization and release verification paths, macOS release documentation and Homebrew cask publication assumptions, while preserving source compatibility where feasible. Optimize the remaining supported Linux and Windows release pipeline for correctness, reliability, and avoidable work without broad unrelated application rewrites. Add regression tests proving no macOS artifact or updater entry can be published and that Linux/Windows artifact, updater, checksum, restricted-content, and publication gates remain intact. Bump to the next patch version, verify all feasible repository checks, review the release diff, merge through the protected branch flow, publish the new release, and audit its public artifacts and updater manifest.

## Goal

Stop distributing an untrusted macOS application, make Linux x86_64 and
Windows x86_64 the only supported desktop release targets, and publish a
version-consistent patch release whose public artifacts and updater manifest
cannot advertise or contain macOS payloads. The reduced release graph must
retain every existing integrity, updater-signature, restricted-content,
checksum, source-package, and protected-publication gate for the supported
platforms while removing avoidable macOS and Homebrew work.

## Acceptance Criteria

- [ ] [AC-1] The public `releases/latest/download/latest.json` endpoint no
  longer contains `darwin-aarch64` or `darwin-x86_64`. The workflow first
  attempts to replace the v0.98.11 manifest in place; if GitHub's immutable
  release policy rejects that mutation, the immutable response is recorded and
  containment is completed by publishing the new supported-platform release,
  after which the live endpoint is independently re-read and contains no
  Darwin key or macOS URL.
- [ ] [AC-2] The release build matrix contains exactly
  `x86_64-unknown-linux-gnu` on Ubuntu and `x86_64-pc-windows-msvc` on Windows;
  no Apple target, macOS runner, DMG, app archive, Apple credential, signing,
  notarization, stapling, Gatekeeper, or macOS release-audit path remains in the
  publishing workflow.
- [ ] [AC-3] Release aggregation fails closed unless every required Linux and
  Windows installer/updater artifact is present, and it rejects any staged
  `.dmg`, `.app`, Apple-target artifact, or Darwin updater entry before a draft
  can be created.
- [ ] [AC-4] `latest.json` contains exactly `linux-x86_64` and
  `windows-x86_64`; both URLs resolve to the version-matched artifacts and both
  detached updater signatures verify against Agentum's embedded updater key.
- [ ] [AC-5] Homebrew cask publication and its deploy key/checksum handoff are
  removed. The final publication job depends directly on the fully verified
  private release draft, shortening the supported release graph without
  weakening protected-main, tag, source-package, checksum, immutable-release,
  or restricted-content gates.
- [ ] [AC-6] Current installation and security documentation identify Linux
  and Windows as the supported packaged desktop platforms. The install script
  returns a clear unsupported-platform error on Darwin instead of constructing
  a nonexistent macOS asset URL. Historical changelog entries and runtime
  codepaths remain historical/source-compatible and are not rewritten.
- [ ] [AC-7] macOS-specific Tauri source configuration needed for developers to
  compile locally may remain, but certificate-free release identity is removed
  and no release, installer, updater, documentation, or CI contract claims
  macOS support.
- [ ] [AC-8] Workspace packages, the Tauri config, lockfile-owned packages, and
  changelog all report the next patch version after 0.98.11, with no bundle ID
  or updater public-key rotation.
- [ ] [AC-9] Focused release/install/security tests and the broader feasible
  repository suite pass; regression tests prove the two supported targets and
  reject reintroduction of macOS release artifacts or updater metadata.
- [ ] [AC-10] A reviewed patch is promoted through `develop` and `staging` to
  protected `main`, then an annotated version tag publishes a public release
  whose checksums, required asset roster, updater signatures, updater URLs, and
  absence of macOS payloads pass a post-publication audit.

## Scope

- Contain the live updater endpoint and record immutable-release behavior.
- Change the release matrix, artifact staging/aggregation, updater manifest,
  release notes, Homebrew dependency, publication graph, and regression tests.
- Remove release-only macOS scripts and documentation that would otherwise
  advertise or validate an unsupported binary distribution.
- Advance release metadata by exactly one patch and publish it through the
  repository's issue, PR, promotion, tag, and release conventions.
- Make only evidence-backed release optimizations created by removing retired
  targets and their exclusive steps; preserve supported-platform security
  checks.

## Non-goals

- No broad application performance rewrite, dependency upgrade, UI redesign,
  data migration, API change, or runtime feature removal.
- No deletion of cross-platform Rust/TypeScript branches merely because they
  mention macOS, and no rewriting of historical changelog or archived planning
  records.
- No Gatekeeper bypass, ad-hoc replacement release, unsigned updater artifact,
  fabricated Apple credential, or mutation of immutable release data through
  unsupported means.
- No Linux architecture expansion, Windows ARM release, Windows code-signing
  expansion, updater key rotation, bundle identifier change, or change to the
  separate Agentum TUI repository.

## Verification

- Capture the current public v0.98.11 manifest, checksums, release immutability
  response, and configured secret names without exposing secret values.
- Run the focused Python release-workflow, install-script, and desktop-security
  contracts before and after implementation, plus shell syntax checks.
- Run repository formatting, locked metadata, lint, test, UI typecheck/test,
  and production-build checks that are feasible on the Linux development host.
- Inspect the complete diff for unsupported platform residue, accidental
  Linux/Windows drift, secret exposure, unsafe deletion, and weakened gates.
- Run a non-publishing GitHub rehearsal on the feature head and require both
  supported native jobs and release aggregation to pass.
- After promotion and publication, independently download `latest.json`,
  `SHA256SUMS`, and every supported artifact; verify the roster, checksums,
  updater signatures, URLs, version metadata, and absence of Darwin/macOS data.

## Capability Deltas

- `deltas/supported-desktop-release.md` establishes the bounded Linux/Windows
  release, updater, unsupported-platform, and publication contract.
