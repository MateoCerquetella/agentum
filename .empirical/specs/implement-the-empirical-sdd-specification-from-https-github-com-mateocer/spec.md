# First-class Empirical SDD source intake

## Request

Implement the Empirical SDD protocol 0.20 repository format from
https://github.com/MateoCerquetella/empirical-sdd as a first-class, read-only
New Spec source in Agentum.

## Goal

An Agentum user can select an existing local Empirical feature, inspect an
immutable preview of its contract and supporting artifacts, and start an
Agentum-owned SDD run from that exact revision without copying text or allowing
the importer to mutate or execute the upstream workflow.

## Acceptance Criteria

- [ ] [AC-1] `GET /api/sdd/capabilities` advertises an available, previewable
  `empirical` source adapter without requiring Node.js, an Empirical executable,
  network access, or installed agent skills.
- [ ] [AC-2] A preview request with source
  `{ "type": "empirical", "path": ".empirical/specs/<feature>" }` accepts an
  Empirical protocol 0.20 schema-4 feature and returns its title, normalized
  Markdown, deterministic `sha256:` source revision, source path, sorted
  capability names and count, design availability, actionable plan-item count,
  and bounded diagnostics.
- [ ] [AC-3] The importer preserves the source `spec.md` contract and translates
  every valid ADDED, MODIFIED, and REMOVED capability Requirement and Scenario
  into stable Agentum authoring context with human-readable capability,
  operation, requirement, and scenario provenance.
- [ ] [AC-4] Optional `design.md` is exposed as imported design context and
  actionable Markdown checklist or ordered-list entries in optional `plan.md`
  become deterministic serial imported tasks; decisions, state journals,
  evidence, credentials, and other runtime material are not imported.
- [ ] [AC-5] Preview is read-only. Creation reopens and renormalizes the source;
  if `expectedSourceRevision` differs from the new snapshot it returns HTTP 409
  `source_revision_changed` before allocating a specification, worktree,
  provider attempt, approval, or run.
- [ ] [AC-6] Unsafe references, traversal, absolute paths, invalid feature names,
  symlinks, replacement races, unsupported or malformed configuration/state
  schemas, malformed deltas, invalid UTF-8, excessive files/depth/bytes, and
  unexpected artifact shapes fail closed with an actionable source error and no
  repository or database mutation.
- [ ] [AC-UI-1] [UI] Run Center's New Spec dialog shows an Empirical source card,
  accepts the canonical feature path, sends the typed source payload, and after
  Preview source visibly reports the immutable revision, capability count,
  imported task count, design availability, and diagnostics while respecting
  server capability gating.
- [ ] [AC-7] Documentation describes the supported Empirical boundary, immutable
  preview/create behavior, local-only constraint, and pinned upstream protocol
  revision `d8ee7e1bdaa53bfc92e278524a40e61d16125f64` without claiming runtime or
  export compatibility.
- [ ] [AC-8] Existing Markdown, Socratic, GitHub, Linear, Jira, and OpenSpec
  source behavior; Agentum provider isolation; approval digests; browser
  evidence; and Deliver-only side effects remain unchanged.

## Scope

- Add a dependency-free Rust Empirical feature importer beside the existing
  OpenSpec importer.
- Extend the closed source request union, capabilities response, preview/create
  route, stored import snapshot, and remote fail-closed classification.
- Extend the desktop SDD client, source model, New Spec dialog, and tests.
- Add upstream-pinned fixture provenance and document the compatibility seam.
- Use anchored no-follow repository reads, bounded deterministic collection,
  and stable revision hashing.

## Non-goals

- Replacing Agentum's database-backed SDD lifecycle with Empirical's local state
  machine.
- Executing or bundling the Empirical CLI, MCP server, TypeScript library,
  Node.js, npm, or global skills.
- Importing Empirical events, locks, evidence, discovery records, context pages,
  capability archives, agent handoff state, or private runtime state.
- Remote-repository Empirical intake in this release.
- Exporting or mutating `.empirical`, authoring Empirical state/events, or
  claiming bidirectional/runtime compatibility.
- Weakening Agentum's source-checkout, sandbox, provider, approval, evidence,
  or Deliver boundaries.

## Risks

- A feature can change between reads. The importer takes two bounded anchored
  snapshots and rejects identity or hash drift; creation independently re-reads
  and compares the caller's expected revision.
- Repository paths are attacker-controlled. Only the exact
  `.empirical/specs/<feature>` shape and safe lowercase feature identifiers are
  accepted, and every directory/file read is no-follow and repository-anchored.
- Future Empirical schemas may change semantics. Only protocol 0.20 schema 4 is
  accepted; other versions fail explicitly instead of being guessed.
- Markdown plans are less typed than Agentum plans. Imported tasks are serial,
  scope-free, command-free context and the diagnostic makes that limitation
  visible before authoring.

## Verification

- Rust unit tests cover canonical import, stable provenance rendering, sorted
  capabilities, optional artifacts, task parsing, size limits, schema errors,
  malformed deltas, unsafe paths, symlinks, and snapshot races.
- Server route tests prove preview creates no durable aggregate, creation binds
  the exact revision, revision drift returns 409 before allocation, and remote
  repositories reject the local-only source without fallback.
- React model/client/integration tests cover the source option, typed request,
  capability gate, preview counts/diagnostics, and visible dialog state.
- Run `cargo fmt --all -- --check`, targeted Rust tests, UI unit/integration
  tests, the production UI build, repository boundary checks, and the broad
  workspace test command supported by available dependencies.
- Exercise New Spec in a real browser against a running app/test surface and
  save a repository-relative screenshot showing the Empirical preview state.
- Independently review the final diff for path safety, mutation ordering,
  compatibility regressions, and scope adherence.

## Capability Deltas

- `deltas/sdd-source-intake.md` adds first-class Empirical source intake while
  retaining Agentum as the authoritative execution and delivery owner.
