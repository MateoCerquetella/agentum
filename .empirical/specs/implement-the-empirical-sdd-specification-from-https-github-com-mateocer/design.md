# Design: first-class Empirical SDD source intake

## Context

Agentum already has a closed `CreateSpecSource` union, a pure `NormalizedSource`
boundary, an anchored filesystem capability, deterministic source revisions,
read-only previews, expected-revision enforcement, durable import snapshots,
and a first-class OpenSpec converter. Empirical protocol 0.20 stores one feature
under `.empirical/specs/<feature>/` with a schema-4 `state.json`, required
`spec.md`, `deltas/<capability>.md` files for Complex work, and optional
`design.md` and `plan.md` artifacts. Its capability-delta grammar intentionally
uses the same ADDED/MODIFIED/REMOVED Requirement and Scenario structure already
parsed by Agentum's OpenSpec converter.

The integration therefore belongs at Agentum's source-normalization seam. It
does not need a second workflow engine, process supervisor, MCP client, Node.js
runtime, or database projection.

## Components and ownership

### Rust source adapter

`crates/agentum-server/src/sdd/sources.rs` owns `import_empirical` beside
`import_openspec`.

1. Validate that the caller supplied exactly
   `.empirical/specs/<safe-feature-name>`.
2. Hold a no-follow `AnchoredDirectory` for the repository, then open
   `.empirical`, `specs`, and the feature by child handle.
3. Read `.empirical/config.json` twice and require `schemaVersion: 4` and
   `setupComplete: true`.
4. Take two complete imported-artifact snapshots through the held feature
   handle. A snapshot reads only `state.json`, `spec.md`, optional `design.md`,
   optional `plan.md`, and Markdown files immediately below optional `deltas/`.
   It does not walk `events/`, locks, decisions, evidence, context, discovery,
   or living capability storage.
5. Require schema-4 state whose `activeFeature` equals the path feature and
   whose profile/phase/status values are strings. Treat other state fields as
   opaque runtime data.
6. Bound every file, file count, total bytes, feature-name length, and delta
   count; reject links, special files, invalid UTF-8, forbidden controls,
   unknown delta extensions, and a mismatch between the two snapshots.
7. Parse each sorted delta using the existing Requirement/Scenario parser and
   render stable Agentum `RQ-*` and `AC-*` lines with capability and operation
   provenance. Preserve the complete normalized `spec.md` above that derived
   index so no contract context is lost.
8. Normalize optional design content and parse optional plan actions. Checklist
   items use the existing task parser; ordered Markdown list entries are also
   accepted because Empirical's generated plan template uses ordered prose.
   Imported tasks remain serial, command-free, and scope-free.
9. Hash normalized path/hash pairs plus the config hash into a deterministic
   `sha256:` revision. Runtime events and locks do not invalidate an artifact
   preview, while any imported contract artifact or schema dependency does.

`NormalizedSource` gains a defaulted, sorted `capabilities: Vec<String>` field.
Existing adapters return an empty list except OpenSpec, which can cheaply expose
its already parsed capability set. This keeps preview rendering generic and
backward-compatible for stored JSON.

### HTTP contract and persistence

`crates/agentum-server/src/routes/sdd.rs` adds the closed request variant:

```json
{
  "type": "empirical",
  "path": ".empirical/specs/add-team-invitations",
  "expectedSourceRevision": "sha256:..."
}
```

The capabilities response advertises `{id: "empirical", available: true,
preview: true}`. `prepare_source` runs the importer on a blocking worker,
compares the expected revision, and returns the same `PreparedSource` used by
all other formats. Preview adds `capabilities` and `capabilityCount` to its
response and digest. Creation stores the sanitized normalized snapshot as an
`sdd_import_jobs` record exactly like Markdown, Socratic, and OpenSpec.

No durable allocation occurs before `prepare_source` succeeds, so existing
ordering already guarantees conflict-before-spec/worktree/attempt/run. Remote
creation classifies Empirical with other local-only reference adapters and
returns the existing typed no-fallback error.

### Desktop client and Run Center

`sdd-client.ts` extends the source kind/reference and preview response types.
`run-center-model.ts` adds an Empirical card with canonical placeholder and
marks it as reference-backed. `SddWorkspaceBar.tsx` maps the draft to the typed
payload and displays capability count/names in the existing preview summary.
The server capabilities response remains authoritative: a missing or disabled
adapter cannot be selected or submitted.

### Documentation and provenance

`docs/AGENTUM_SDD.md` documents the local read-only Empirical boundary and
pinned upstream commit. A small fixture/provenance record under server tests
captures the grammar and hashes used by compatibility tests without executing
or vendoring the upstream package.

## Data flow

```text
New Spec dialog
  -> POST source preview {type: empirical, path}
  -> anchored, bounded double snapshot
  -> schema/state/delta validation
  -> NormalizedSource + sha256 revision
  -> visible immutable preview
  -> POST create with expectedSourceRevision
  -> independent re-read and exact comparison
  -> conflict with no allocation OR canonical Agentum authoring attempt
  -> Agentum approvals/evidence/review/Ready/Deliver unchanged
```

## Failure behavior

- Unsafe path, link, special file, malformed JSON/Markdown, invalid schema, or
  bound violation maps through the existing source error boundary and creates
  no state.
- A directory or imported file changing during a snapshot returns `Changed`.
- A source revision changing after preview returns HTTP 409
  `source_revision_changed` with expected and current revisions.
- An optional plan with no actionable items produces an informational
  diagnostic and zero tasks; an empty required spec or malformed delta fails.
- Unsupported future schemas fail with a message naming schema 4 as the only
  supported contract.
- Remote repositories reject Empirical intake explicitly and never read a
  substitute local checkout.

## Compatibility

- The API addition is additive and serde remains deny-unknown for every source
  variant.
- The new defaulted `capabilities` field keeps historical normalized snapshots
  readable.
- Existing source adapters, provider execution, approval digests, browser
  evidence, delivery actions, and database schemas do not change.
- The implementation claims artifact intake compatibility only, not Empirical
  runtime, archive, handoff, or export compatibility.

## Verification design

- Pure importer tests construct canonical and adversarial repositories and
  assert exact normalized output and stable revision behavior.
- A fixture tied to upstream commit
  `d8ee7e1bdaa53bfc92e278524a40e61d16125f64` pins config/state/spec/delta/design/
  plan shapes and license provenance.
- Route tests assert preview purity and expected-revision conflict ordering.
- UI model and integration tests assert selection, request construction,
  capability gating, visible metadata, and diagnostics.
- Real-browser evidence captures the populated Empirical preview state after a
  production UI build.

## Rollback

The feature is additive. Reverting the source enum arm, importer, capabilities
entry, UI option, docs, and tests removes the behavior without data migration.
Already stored `sourceKind: empirical` import snapshots remain inert historical
JSON and do not grant execution or delivery authority.
