# GitHub issue: retire broken macOS distribution

- Issue: https://github.com/MateoCerquetella/agentum/issues/477
- Labels: `type/fix`, `area/ci`, `area/desktop`, `priority/p0`,
  `status/in-progress`, `github_actions`

## Summary

The immutable v0.98.11 release advertises ad-hoc-signed macOS updater bundles
that both native GitHub jobs proved Gatekeeper rejects. The updater installs
the bundle and restarts, leaving an existing Mac installation unable to reopen
normally. The owner has retired macOS distribution rather than provisioning
Apple signing/notarization credentials.

## Motivation

Stop additional Mac clients receiving an untrusted update, eliminate
unsupported macOS artifacts and Homebrew release work, and preserve a
fail-closed, faster release path for the actively supported Linux x86_64 and
Windows x86_64 desktop clients.

## Proposed approach

- Record GitHub's immutable refusal to replace v0.98.11 `latest.json`; do not
  bypass immutable history.
- Remove Apple targets/runners, DMGs/app archives, macOS audit/signing paths,
  Darwin updater entries, Mac release notes, Homebrew secret/checksum/job, and
  current Mac distribution claims.
- Keep safe source-only platform metadata where feasible but remove forced
  ad-hoc release identity.
- Make release aggregation explicitly reject retired Mac payloads and require
  exactly the Linux/Windows artifact and updater rosters.
- Make the POSIX installer reject Darwin before download or filesystem
  mutation.
- Bump to v0.98.12, verify locally and on native GitHub runners, promote through
  develop → staging → main, publish, and independently audit public assets.

## Acceptance criteria

- `/releases/latest/download/latest.json` contains exactly `linux-x86_64` and
  `windows-x86_64` after v0.98.12 publication.
- Release matrix contains exactly Linux x86_64 and Windows x86_64; no macOS
  artifact or updater path remains.
- Aggregation rejects `.dmg`, `.app`, Apple-target, macOS-named, or
  Darwin-manifest residue before draft creation.
- Homebrew publication and its deploy-key dependency are removed; publication
  depends directly on the verified private draft.
- Linux/Windows updater signatures, checksums, restricted-content, source
  archive, protected tag/main, and immutable release gates remain enforced.
- Darwin installation fails clearly before any download; current documentation
  names only supported packaged platforms.
- v0.98.12 is version-consistent, passes local/native verification and review,
  and its public artifact roster and updater manifest pass an independent audit.

## Containment evidence

GitHub reports v0.98.11 `immutable: true`. The authorized replacement attempt
returned HTTP 422: `Cannot delete asset from an immutable release`; the public
manifest SHA-256 remained unchanged. The corrective patch release is therefore
the supported containment mechanism.
