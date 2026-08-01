# Plan: Retire macOS Release and Optimize Supported Platforms

1. Preserve containment and baseline evidence.
   - Save the public v0.98.11 `latest.json`, release immutability metadata,
     asset roster, checksums, configured secret names, and prior native macOS
     rejection logs without recording secret values.
   - Construct the authorized two-platform v0.98.11 manifest and attempt the
     supported in-place replacement once; record GitHub's immutable-release
     rejection and leave the historical release untouched.
   - Run the focused and full Python repository tests before implementation.
   - Covers AC-1 and establishes the baseline for AC-9.

2. Create the required tracked GitHub issue.
   - Document the incident, owner decision, supported-platform boundary,
     implementation approach, acceptance criteria, and release objective.
   - Apply the repository's fix/release/priority labels that already exist.
   - Use the issue number in the feature commit and PR body.
   - Covers the delivery precondition for AC-10.

3. Replace the release regression contract first.
   - Rewrite focused workflow assertions to require exactly the Linux and
     Windows matrix entries, required artifact allow-list, retired-payload
     deny-list, exact two-key updater manifest, direct publication dependency,
     and absence of every macOS/Homebrew release path.
   - Add an executable installer test proving Darwin fails before download or
     filesystem mutation and retain supported Linux mapping assertions.
   - Update desktop security assertions to distinguish retained local-build
     metadata from the removed forced ad-hoc release identity.
   - Run focused tests and record that they fail against v0.98.11 for the
     expected former behavior.
   - Covers AC-2 through AC-7 and AC-9.

4. Simplify and harden the supported release workflow.
   - Remove the Homebrew secret, Apple matrix entries, Apple-only comments and
     build/staging branches, DMG/app globs, macOS audit step, Mac required
     assets, Darwin manifest emissions, appended Mac release note, Homebrew
     checksum handoff/job, and publication dependency.
   - Keep exactly Linux x86_64 and Windows x86_64 with their current native
     runners and package/update formats.
   - Add explicit aggregation rejection for `.dmg`, `.app`, `apple-darwin`,
     `macos-*`, and Darwin updater residue before the private draft.
   - Require exactly two supported updater keys and preserve updater signature,
     restricted-content, checksum, source archive, tag/main, and immutable
     private-draft gates.
   - Delete the release-only macOS verifier and Mac release-note fragment.
   - Covers AC-2 through AC-5.

5. Retire unsupported installation and current distribution claims.
   - Make `scripts/install.sh` reject Darwin during platform detection and
     remove DMG asset/install functions while retaining Linux installation.
   - Update README platform badge, install prose/table, and current security
     documentation for Linux and Windows only.
   - Remove forced ad-hoc signing identity while retaining safe macOS metadata
     needed for source compilation; preserve historical changelog/plans and
     unrelated runtime branches.
   - Covers AC-6 and AC-7.

6. Prepare the corrective patch consistently.
   - Advance the workspace and Tauri versions through 0.98.12 to the unused
     v0.98.13 correction and mechanically
     update only workspace-owned lockfile package versions.
   - Add a focused changelog entry describing macOS retirement, updater
     containment, supported-platform release hardening, and Homebrew removal.
   - Prove bundle identifier and updater public key are unchanged.
   - Covers AC-8.

7. Verify and review locally.
   - Run focused release/install/security tests, shell syntax, full Python
     discovery, formatting, locked metadata, Rust lint/tests, UI typecheck/tests,
     and production build as feasible.
   - Inspect the full diff and run secret/restricted-content checks for scope,
     unsafe deletions, supported-platform drift, stale release claims, and
     weakened gates; repair every actionable finding.
   - Complete Empirical verify and independent review evidence.
   - Covers AC-9 and pre-publication portions of AC-2 through AC-8.

8. Publish the reviewed change through protected branches.
   - Commit only the scoped worktree files with the issue reference, push the
     feature branch, and open the issue-linked PR into `develop`.
   - Reconcile released-main history only through reviewed non-force merges,
     run a non-publishing native release rehearsal, and require both supported
     jobs plus aggregation to pass.
   - Merge and promote `develop` to `staging`, then `staging` to `main`,
     revalidating the exact commit and required checks at each boundary.
   - Covers AC-9 and the promotion portion of AC-10.

9. Tag, publish, and audit v0.98.13.
   - Preserve the failed v0.98.12 workflow and signed tag as evidence; replace
     raw compressed-byte regex scanning with printable ASCII and Windows
     UTF-16 scanning, add regression coverage, and re-run local/native gates.
   - Create and push a signed annotated version tag at the exact protected main
     tip and invoke the publishing release workflow once.
   - Require private draft creation and successful public transition.
   - Download the public manifest, checksums, source archive, Linux artifacts,
     Windows artifacts, and updater signatures; verify all checksums/signatures,
     exact names and URLs, version consistency, two-key updater set, and zero
     macOS/Darwin payloads.
   - Re-read `/releases/latest/download/latest.json`, archive the reviewed
     capability delta, and complete Empirical only after every audit passes.
   - Covers AC-1, AC-4, and AC-10 and confirms all prior criteria.
