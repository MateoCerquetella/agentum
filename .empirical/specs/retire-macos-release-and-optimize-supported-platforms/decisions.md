# Decisions: Supported-Platform Desktop Release

## D-001: Retire macOS distribution instead of weakening trust

Status: Accepted

### Evidence

v0.98.11 uses ad-hoc identity `-`; both native jobs logged skipped
notarization and Gatekeeper rejection. The owner explicitly declined future
macOS use and authorized retirement.

### Options

1. Provision Developer ID and notarization credentials.
2. Retain ad-hoc binaries with user bypass instructions.
3. Retire packaged macOS delivery.

### Chosen approach

Choose option 3. Retire macOS installers, updater entries, release jobs,
Homebrew cask publication, and current distribution claims. Do not ship
another ad-hoc or bypass-dependent Mac application.

### Trade-offs and risks

Mac users receive no packaged updates after containment. Local source
codepaths are preserved where they do not burden the release graph.

### Verification

Static release contracts, public updater key audit, public asset roster, and
unsupported Darwin installer tests prove the retired boundary.

## D-002: Respect immutable release history

Status: Accepted

### Evidence

GitHub's release API reports v0.98.11 `immutable: true` and the repository
immutable-release endpoint reports `enabled: true`.

### Options

1. Delete and re-upload v0.98.11 assets.
2. Delete or retag the release.
3. Publish a corrective patch that becomes the latest endpoint.

### Chosen approach

Attempt only the authorized supported API replacement so the rejection is
recorded, then choose option 3: preserve v0.98.11 and make the next successfully
published supported-platform patch the containment boundary. Never circumvent
release immutability or rewrite a pushed signed corrective tag.

### Trade-offs and risks

Historical direct v0.98.11 URLs remain available, but active clients using
`/releases/latest` stop receiving Darwin metadata after v0.98.13.

### Verification

Capture the mutation response and independently re-read the live latest
manifest after publication.

## D-003: Use a supported allow-list and retired-payload deny-list

Status: Accepted

### Evidence

The existing workflow relies on a four-target matrix and required asset list;
removing entries alone could let stale macOS payloads survive artifact
aggregation or manifest generation.

### Options

1. Remove only Darwin manifest emission.
2. Remove matrix entries without an explicit residue gate.
3. Combine exact supported allow-lists with retired-payload rejection.

### Chosen approach

Choose option 3. Define exactly Linux x86_64 and Windows x86_64 at build,
required-asset, updater-key, and public-audit boundaries, and explicitly reject
Mac payload names and types before draft creation.

### Trade-offs and risks

New supported targets require intentional contract changes, which is desirable
for a security-sensitive release surface.

### Verification

Focused tests inspect matrix entries, required names, deny patterns, manifest
emissions, and public output.

## D-004: Remove the Homebrew job from the critical path

Status: Accepted

### Evidence

The tap consumes only macOS DMG checksums and its job is the sole consumer of
`HOMEBREW_TAP_DEPLOY_KEY` and `release-homebrew-checksums`.

### Options

1. Leave a no-op job.
2. Retain the secret and tap update.
3. Delete the Mac-only handoff and publish from the verified draft.

### Chosen approach

Choose option 3. Remove the secret, checksum handoff, job, release note, and
dependency. Publication depends directly on verified release aggregation.

### Trade-offs and risks

Existing Homebrew cask history is not updated for corrective patches, which matches
retiring Mac distribution and shortens release latency.

### Verification

Workflow tests require absence of all Homebrew names and a direct
`publish -> release` dependency.

## D-005: Bound optimization to removed release work

Status: Accepted

### Evidence

The owner requested broad optimization, but the incident and release objective
provide evidence only for two macOS builds, Mac verification, and Homebrew
publication as avoidable work.

### Options

1. Perform broad dependency and application optimization.
2. Limit optimization to release-path cleanup.
3. Make no optimization beyond deleting matrix entries.

### Chosen approach

Choose option 2. Remove retired jobs, steps, artifacts, secrets, globs, and
conditionals and simplify surviving case logic. Avoid unrelated runtime or
dependency changes in the release patch.

### Trade-offs and risks

Broader performance work remains separate and measurable rather than being
hidden inside an urgent distribution fix.

### Verification

Diff review confirms supported-platform behavior is preserved and changes
remain within release, installer, docs, tests, version, and Empirical scope.

## D-006: Preserve source compatibility without promising support

Status: Accepted

### Evidence

Cross-platform runtime branches and safe Tauri macOS metadata are not
responsible for public distribution, while forced ad-hoc signing is.

### Options

1. Delete all macOS code and configuration.
2. Leave every macOS release path unchanged.
3. Remove release identity and delivery while retaining local-build metadata.

### Chosen approach

Choose option 3. Keep non-release platform code and safe local-build metadata,
remove forced ad-hoc identity, and make no CI or support promise for macOS.

### Trade-offs and risks

Local compilation may continue, but it can regress without a release gate;
that is explicit and preferable to accidental public binaries.

### Verification

Tauri identity/public-key assertions and repository diff review distinguish
source metadata from release claims.

## D-007: Scan textual binary content instead of compressed bytes

Status: Accepted

### Evidence

The exact v0.98.12 source passed the Windows native rehearsal but a second
build failed when a restricted-content regular expression matched the
nondeterministic compressed bytes of the NSIS installer. The workflow stopped
before aggregation, draft creation, or publication.

### Options

1. Retry until the compressed bytes happen not to match.
2. Exclude generated installers from restricted-content scanning.
3. Scan printable ASCII strings universally and Windows UTF-16 strings only
   for PE files, retaining fail-closed policy, enumeration, and error handling.

### Chosen approach

Choose option 3. Classify binary inputs, extract printable single-byte strings
universally and UTF-16LE strings only from PE files, apply the same externally
supplied regex policy to those textual views, and scan explicit bundles below
`target`. Preserve the failed run and signed v0.98.12 tag, then release the
reviewed correction as v0.98.13.

### Trade-offs and risks

Compressed byte coincidences no longer block a release. Actual printable
restricted strings in generated binaries continue to fail, including PE-scoped
Windows resource strings, while non-PE archives are not reinterpreted as
Windows text and extraction/tool failures remain fatal.

### Verification

Regression tests cover compressed control-byte false positives, printable
ASCII matches, PE-scoped UTF-16LE matches, non-PE binary handling, explicit
bundle enumeration, and safe error reporting; both native targets and the
aggregate must pass before promotion.
