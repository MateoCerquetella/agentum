# Design: Supported-Platform Desktop Release

## Current failure and containment boundary

The public v0.98.11 release is immutable and its updater manifest advertises
ad-hoc-signed macOS replacement bundles that the native release jobs proved
Gatekeeper rejects. The in-app updater verifies the Tauri updater signature,
installs the replacement bundle, and restarts; updater authenticity therefore
does not provide Apple platform trust. GitHub reports the release as
`immutable: true`, so an authorized in-place replacement attempt is expected
to fail and must not be bypassed. Containment completes when a newer supported-
platform release becomes GitHub's latest release and its live manifest contains
no Darwin entry or macOS URL.

## Supported release data flow

1. Preflight retains version equality, protected-main identity, verified signed
   annotated tag, provider evidence, restricted-content policy, and version
   collision checks. It requires only the updater-signing key/password and the
   restricted-content patterns; the Homebrew deploy key is removed.
2. The build matrix is an explicit two-entry allow-list:
   `x86_64-unknown-linux-gnu` on `ubuntu-22.04` and
   `x86_64-pc-windows-msvc` on `windows-latest`.
3. Shared UI, pinned Rust, Tauri, updater-signing, and restricted-content steps
   remain. Apple matrix entries, dylib staging, DMG/app staging, native macOS
   audit, and macOS upload globs are deleted. Linux and Windows branches retain
   their exact installers, updater artifacts, native format checks, and Linux
   runtime/library/install audits.
4. Each build uploads only its scoped `dist` output. Aggregation downloads the
   two artifacts, verifies the complete required allow-list, rejects symlinks
   and duplicate basenames, and explicitly fails on `.dmg`, `.app`,
   `apple-darwin`, `macos-`, or Darwin updater residue.
5. Updater signature verification still runs against every staged supported
   updater bundle. `latest.json` emits exactly `linux-x86_64` and
   `windows-x86_64`, verifies the key set and version-matched URLs, and cannot
   be drafted when either artifact/signature is missing.
6. Release notes are generated from the exact changelog section without an
   appended macOS install note. Checksums cover every final asset except the
   checksum file itself. Source packaging and restricted-content scans remain.
7. The private draft job becomes the direct prerequisite of publication. The
   Homebrew checksum handoff, tap clone, cask rewrite, deploy key, and job are
   removed, reducing the critical path without weakening release validation.
8. Publication remains a one-time transition of the exact private draft. The
   public audit downloads every asset and independently verifies names,
   checksums, updater signatures, URLs, versions, and the absence of retired
   platform data.

## Installer and documentation behavior

The release installer recognizes Linux as its supported shell-install target.
Darwin fails during platform detection with an explicit unsupported-platform
message before asset naming, network download, DMG mounting, or application
replacement. Windows remains distributed through the native installer rather
than the POSIX installer script. README badges, current install tables, current
security guidance, and the release workflow describe Linux and Windows only.
Historical changelog entries and archived plans remain historical evidence.

## Source compatibility boundary

Retiring binary distribution does not justify deleting cross-platform runtime
branches. Tauri's macOS Info.plist, microphone entitlement, framework metadata,
and conditional Rust/TypeScript code may remain for local source builds. The
forced ad-hoc `signingIdentity: "-"` release setting is removed. CI and release
tests assert only that no packaged or updater path can publish macOS; they do
not claim that local source compilation on macOS is supported or tested.

## Version and promotion

The initial correction advanced the workspace, owned lockfile packages, Tauri
config, and changelog from 0.98.11 to 0.98.12 without changing the bundle
identifier or updater public key. Its signed tag remains an unpublished record
because the publication workflow stopped safely on a nondeterministic raw-byte
binary scan. Advance the correction to 0.98.13, promote it through `develop`,
`staging`, and `main`, create a verified signed annotated `v0.98.13` tag from
the exact protected `main` tip, and invoke publication from that tag.

## Regression contract

Focused tests must prove:

- exactly two build targets and no macOS runner/target/release verifier;
- no Homebrew secret, checksum artifact, job, or publication dependency;
- exact Linux/Windows required assets and explicit retired-payload rejection;
- exactly two updater keys and no Darwin/macOS emit call or URL;
- retained updater signature, checksum, source archive, immutable draft,
  protected tag/main, restricted-content, Linux library, and Windows format
  gates;
- Darwin installer failure before any download or filesystem mutation;
- version consistency and unchanged updater identity.

## Failure handling

- Immutable v0.98.11 mutation rejection: record it; do not delete the release,
  retarget its tag, or bypass immutability. Complete containment with v0.98.13.
- Missing supported artifact/signature: fail aggregation before draft creation.
- Retired payload residue: fail aggregation even if the required allow-list is
  otherwise complete.
- Rehearsal or publication-gate failure: preserve the failed run and any pushed
  signed tag, repair on a new reviewed commit, and use a new patch version.
- Promotion drift: revalidate the exact commit after each protected-branch
  merge and publish only the current main tip.
- Public audit mismatch: do not report completion; preserve immutable evidence
  and publish a new corrective patch rather than mutating released bytes.

## Acceptance trace

- AC-1: immutable response plus post-v0.98.13 live endpoint audit.
- AC-2, AC-3: two-target matrix and fail-closed aggregation allow/deny lists.
- AC-4: exact two-key manifest and updater-signature verification.
- AC-5: Homebrew removal and direct verified-draft publication dependency.
- AC-6, AC-7: installer/docs retirement with bounded source compatibility.
- AC-8: consistent patch metadata and stable updater/bundle identity.
- AC-9: local, GitHub-native, and independent review evidence.
- AC-10: issue/PR promotions, signed tag, public artifact audit, and archive.
