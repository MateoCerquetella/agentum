# Plan: first-class Empirical SDD source intake

1. Add an upstream-pinned Empirical protocol fixture and provenance record under
   `crates/agentum-server/tests/fixtures/empirical/official/`, covering schema-4
   config/state plus spec, capability delta, design, and plan artifacts. Verify
   recorded hashes and license metadata in a repository test. (AC-2, AC-3,
   AC-4, AC-7)
2. Extend `NormalizedSource` with sorted capability metadata and implement the
   pure `import_empirical` adapter in `sdd/sources.rs`: canonical path
   validation, held no-follow directory traversal, bounded double snapshots,
   schema/state validation, delta reuse, contract rendering, optional design,
   conservative plan parsing, diagnostics, and deterministic source revision.
   Add focused canonical and adversarial unit tests. (AC-2, AC-3, AC-4, AC-6,
   AC-8)
3. Extend the server's closed source union, capabilities response, preview
   response/digest, create preparation, durable import allowlist, and remote
   fail-closed classification. Add route tests for preview purity, exact
   revision conflicts before aggregate allocation, and existing-source
   regression behavior. (AC-1, AC-2, AC-5, AC-6, AC-8)
4. Extend the desktop SDD client and Run Center source model with the Empirical
   source kind, typed payload, placeholder, capability metadata, and preview
   summary. Add model/client/integration tests for capability gating, selection,
   request construction, visible counts/design/diagnostics, and failure state.
   (AC-UI-1, AC-8)
5. Update `docs/AGENTUM_SDD.md`, relevant README/API text, and changelog with the
   exact supported boundary and pinned upstream revision. Run restricted-content
   and artifact checks. (AC-7, AC-8)
6. Format and run focused Rust/UI tests, full available workspace tests, UI type
   check/build, and source-boundary checks. Repair failures without weakening
   criteria. (AC-1 through AC-8)
7. Start the built app or the closest production-equivalent Run Center surface,
   exercise New Spec -> Empirical -> Preview source in a real browser, and save
   a repository-relative screenshot plus interaction evidence. (AC-UI-1)
8. Independently review the final diff for filesystem safety, mutation ordering,
   compatibility, documentation accuracy, and test sufficiency; repair blocking
   findings and archive the reviewed capability delta. (AC-1 through AC-8)
9. Follow the repository's issue-first delivery workflow: create/link the issue,
   commit with the closing reference, push the feature branch, open a PR into
   `develop`, verify required checks, promote through staging/main where branch
   protections and credentials permit, and publish the release with the exact
   reviewed commit. (AC-7, AC-8)
