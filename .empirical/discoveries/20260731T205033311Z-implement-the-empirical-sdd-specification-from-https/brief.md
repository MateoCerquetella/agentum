# Socratic discovery: Implement the Empirical SDD specification from https://github.com/MateoCerquetella/empirical-sdd into Agentum

- Status: started
- Created: 2026-07-31T20:50:33.311Z
- Updated: 2026-07-31T21:04:10.761Z

## Pass 1: Problem and user

**Question:** Who is this for, what problem do they experience today, and why does solving “Implement the Empirical SDD specification from https://github.com/MateoCerquetella/empirical-sdd into Agentum” matter?

> Agentum users and development teams who already author repository-local Empirical SDD features cannot bring those contracts into Agentum without pasting or rewriting them, which loses capability-delta, design, plan, revision, and provenance information. They need Agentum to consume an Empirical feature as a first-class source while retaining Agentum-owned execution, approvals, evidence, and delivery.

## Pass 2: Observable outcome

**Question:** Describe the smallest end-to-end behavior a user must be able to observe when this succeeds.

> In Run Center, a user selects Empirical as the New Spec source, enters a repository-relative .empirical/specs/<feature> path, previews the exact source revision with title, diagnostics, design availability, capability count, and imported task count, then creates and authors an Agentum specification from that immutable snapshot. If the source changes between preview and creation, Agentum returns a source_revision_changed conflict before allocating any specification, worktree, attempt, or run.

## Pass 3: Boundaries

**Question:** What must the first version include, what is explicitly out of scope, and which constraints cannot change?

> The first version must provide a local, read-only, dependency-free Rust importer for Empirical protocol 0.20 schema 4 feature directories; preserve spec.md, ADDED/MODIFIED/REMOVED capability requirements and scenarios, optional design.md, and actionable plan.md items; expose preview/create API and desktop UI support; and document pinned upstream provenance. It must not execute the Empirical CLI or Node.js, install skills, mutate .empirical state, import journals or private runtime state, replace Agentum's database-backed lifecycle, add remote-source support, or publish an Empirical export. Agentum's existing sandbox, hash-bound approvals, no-side-effect-before-Deliver policy, and source-checkout policy cannot change.

## Pass 4: Failure and risk

**Question:** Which partial failures, invalid data, compatibility breaks, or rollback risks could change the solution?

> Traversal, symlink substitution, repository races, oversized trees, malformed capability deltas, unsupported schema versions, invalid UTF-8, and an active Empirical feature changing during preview are the principal risks. The importer must deny unsafe or unknown input, use anchored no-follow reads and bounded deterministic snapshots, emit actionable typed diagnostics, and leave the repository and Agentum database unchanged on every preview or validation failure. A creation re-read that does not match the previewed revision must fail with HTTP 409 and no fallback or partial allocation.

## Pass 5: Verification

**Question:** How will we prove each behavior in a real browser, including interaction checks and screenshot evidence for visible states?

> Automated Rust unit and route integration tests must prove canonical import, capability/scenario preservation, task extraction, size and schema validation, traversal and symlink rejection, stable revision hashing, read-only preview, and conflict-before-allocation behavior against a fixture pinned to the upstream commit. React model and integration tests must prove the Empirical option, source payload, capability gating, preview summary, and error state. A production desktop build plus a real-browser interaction will open New Spec, choose Empirical, enter a fixture path, render the immutable preview summary and diagnostics, and capture a repository-relative screenshot of that visible state. Workspace tests, formatting, linting, restricted-content checks, and independent diff review must also pass.

## Refined request

> Implement the Empirical SDD specification from https://github.com/MateoCerquetella/empirical-sdd into Agentum
>
> Approved Socratic discovery:
> - Primary user and problem: Agentum users and development teams who already author repository-local Empirical SDD features cannot bring those contracts into Agentum without pasting or rewriting them, which loses capability-delta, design, plan, revision, and provenance information. They need Agentum to consume an Empirical feature as a first-class source while retaining Agentum-owned execution, approvals, evidence, and delivery.
> - Smallest observable outcome: In Run Center, a user selects Empirical as the New Spec source, enters a repository-relative .empirical/specs/<feature> path, previews the exact source revision with title, diagnostics, design availability, capability count, and imported task count, then creates and authors an Agentum specification from that immutable snapshot. If the source changes between preview and creation, Agentum returns a source_revision_changed conflict before allocating any specification, worktree, attempt, or run.
> - Scope, non-goals, and constraints: The first version must provide a local, read-only, dependency-free Rust importer for Empirical protocol 0.20 schema 4 feature directories; preserve spec.md, ADDED/MODIFIED/REMOVED capability requirements and scenarios, optional design.md, and actionable plan.md items; expose preview/create API and desktop UI support; and document pinned upstream provenance. It must not execute the Empirical CLI or Node.js, install skills, mutate .empirical state, import journals or private runtime state, replace Agentum's database-backed lifecycle, add remote-source support, or publish an Empirical export. Agentum's existing sandbox, hash-bound approvals, no-side-effect-before-Deliver policy, and source-checkout policy cannot change.
> - Failure cases and risks: Traversal, symlink substitution, repository races, oversized trees, malformed capability deltas, unsupported schema versions, invalid UTF-8, and an active Empirical feature changing during preview are the principal risks. The importer must deny unsafe or unknown input, use anchored no-follow reads and bounded deterministic snapshots, emit actionable typed diagnostics, and leave the repository and Agentum database unchanged on every preview or validation failure. A creation re-read that does not match the previewed revision must fail with HTTP 409 and no fallback or partial allocation.
> - Required verification: Automated Rust unit and route integration tests must prove canonical import, capability/scenario preservation, task extraction, size and schema validation, traversal and symlink rejection, stable revision hashing, read-only preview, and conflict-before-allocation behavior against a fixture pinned to the upstream commit. React model and integration tests must prove the Empirical option, source payload, capability gating, preview summary, and error state. A production desktop build plus a real-browser interaction will open New Spec, choose Empirical, enter a fixture path, render the immutable preview summary and diagnostics, and capture a repository-relative screenshot of that visible state. Workspace tests, formatting, linting, restricted-content checks, and independent diff review must also pass.

## Workflow handoff

- Workflow: complex
- Feature: implement-the-empirical-sdd-specification-from-https-github-com-mateocer
- Revision: 1
