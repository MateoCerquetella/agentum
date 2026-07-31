# Decisions: first-class Empirical SDD source intake

## D-001: Integrate at the source-normalization seam

Status: Accepted

### Evidence

Agentum already owns a database-backed SDD lifecycle with provider sandboxing,
hash-bound approvals, typed evidence, independent review, Ready, and explicit
Deliver. Empirical protocol 0.20 exposes repository-local specification
artifacts whose capability-delta grammar matches the converter already present
in `sdd/sources.rs`. The requested observable behavior begins in New Spec.

### Options

1. Replace Agentum's lifecycle with a Rust port of the Empirical state machine.
2. Bundle or invoke the Node.js Empirical CLI/MCP server as a sidecar.
3. Add a dependency-free, read-only Empirical adapter to Agentum's existing
   source-normalization boundary.

### Chosen approach

Choose option 3. Import Empirical artifacts as immutable authoring context and
keep Agentum authoritative for all execution, approval, evidence, and delivery
state.

### Trade-offs and risks

This delivers safe interoperability without duplicate state ownership or a new
runtime dependency. It intentionally does not provide bidirectional runtime or
archive compatibility. Documentation and capability labels must keep that
boundary explicit.

### Verification

Tests prove the adapter executes no external binary, mutates no `.empirical`
file, produces only `NormalizedSource`, and enters the unchanged Agentum create
and lifecycle paths.

## D-002: Bind revisions to imported artifacts, not volatile journals

Status: Accepted

### Evidence

Empirical journals, locks, status messages, and evidence can change while the
contract artifacts remain identical. Agentum needs a stable preview digest but
must detect changes to config schema, state identity, spec, deltas, design, or
plan before creation.

### Options

1. Hash the entire `.empirical` tree, including events and locks.
2. Hash only `spec.md` and ignore schema and supporting artifacts.
3. Double-snapshot the bounded imported artifact set and hash normalized
   relative-path/content-hash pairs plus the config dependency.

### Chosen approach

Choose option 3. Validate state identity and schema, import the contract artifact
set through held no-follow handles, reject snapshot drift, and exclude volatile
runtime material from the source revision.

### Trade-offs and risks

Events can advance without invalidating a preview, which is desirable because
they are not imported. Any material artifact or schema change invalidates the
revision. Anchored handles and a second snapshot reduce substitution risk; the
create route still performs a separate revision comparison.

### Verification

Race, symlink, schema, and revision tests assert fail-closed behavior and prove
that event-only changes leave the artifact revision stable while imported-file
changes do not.

## D-003: Convert plan prose conservatively

Status: Accepted

### Evidence

Empirical `plan.md` is Markdown rather than Agentum's typed `plan.json`.
Agentum's imported tasks intentionally have no inferred commands, file scopes,
or parallel-safety claims.

### Options

1. Ignore `plan.md` completely.
2. Guess typed commands, scopes, dependencies, and parallelism from prose.
3. Extract explicit checklist and ordered-list actions as bounded serial task
   intent while leaving commands and scopes empty.

### Chosen approach

Choose option 3. Preserve actionable plan intent without inventing authority or
execution metadata.

### Trade-offs and risks

Nested prose may remain only in the source context and some tasks may need
refinement by the authoring provider. A preview diagnostic and serial defaults
make the lossy boundary visible and safe.

### Verification

Parser tests cover checklists, numbered actions, empty/non-action prose,
acceptance-criterion references, deterministic ordering, and bounded task text.
