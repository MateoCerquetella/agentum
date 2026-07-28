# Data model

Agentum persists operational state in SQLite with WAL enabled. The schema is
applied in order from `crates/agentum-store/migrations/`; migration
`0030_agentum_sdd.sql` establishes the authoritative specification-driven
development model.

Repository artifacts and database rows have separate jobs. The `.agentum/`
tree contains portable project intent: one static manifest, immutable
specification revisions, and later-phase artifacts only when they contain real
information. Mutable run status, approvals, attempts, leases, external links,
and delivery state never enter Git as workflow metadata.

## SDD aggregates

The normalized tables are grouped by responsibility:

- `sdd_specs`, `sdd_repo_artifact_sets`, and `sdd_spec_revisions` own stable
  `SPC-<ULID>` identity, the repository's immutable artifact-set identity, and
  immutable specification history.
- `sdd_runs`, `sdd_artifact_revisions`, `sdd_tasks`, and `sdd_attempts` own the
  current run aggregate, artifact provenance, immutable task intent plus
  separate runtime status, and isolated provider attempts.
- `sdd_capability_grants`, `sdd_leases`, `sdd_patch_ledger`, and
  `sdd_verification_results` bound writes and preserve the evidence needed to
  apply, verify, roll back, or quarantine a change.
- Migration `0033_sdd_browser_evidence.sql` adds `sdd_evidence_blobs`,
  `sdd_browser_evidence`, and `sdd_browser_evidence_blobs`. Blob rows contain
  immutable content metadata for Agentum-owned storage; manifest rows bind one
  typed check result to its run, attempt, grant, and specification revision;
  join rows require explicit capture, console-transcript, and
  network-transcript roles. Raw capture bytes never enter SQLite or the project.
- `sdd_approval_requests` and `sdd_approval_decisions` bind a human decision to
  the exact run revision and digest. A later specification revision invalidates
  the earlier decision.
- `sdd_external_links` and `sdd_import_jobs` record provider-neutral source
  provenance and deterministic import previews.
- `sdd_delivery_previews` and `sdd_delivery_actions` keep commit, push, pull
  request, tracker, and release effects behind an explicit, expiring,
  hash-bound confirmation. Ambiguous external results can remain
  `sync_pending` and be retried without losing Ready state.
- `sdd_events` is the durable cursor-ordered audit stream; `sdd_outbox` makes
  downstream delivery retryable.
- `sdd_idempotency` stores the exact response for each mutating request in the
  same transaction as its state change. `sdd_create_sagas` makes interrupted
filesystem and provider work visible and recoverable.

Migration `0034_sdd_remote_worker.sql` is the separate host-subsystem journal.
`sdd_remote_worker_runs` binds a run to one host, registered repository hash,
stable repository artifact-set ULID, specification revision, base commit,
provider, approval digest, and sequential phase checkpoint.
`sdd_remote_worker_requests` stores request hashes, exact replay responses,
attempt paths, and crash stages. `sdd_remote_worker_patch_journal` records
bounded operations and rollback preimages before authoritative filesystem
writes. `sdd_remote_worker_lease` is a validated, expiring singleton lease, so
separate SSH subsystem processes cannot execute remote SDD work concurrently.
Blocked requests can retry with a new id only when the immutable bindings and
checkpoint still match; same-id/different-payload requests are conflicts.

Browser evidence submission is one transaction: the store validates the
one-use attempt grant, recomputes every manifest SHA-256, requires at least one
capture plus exactly one console and network transcript, inserts blob metadata,
manifests and role links, consumes the grant, advances the run CAS, appends the
durable event/outbox row, and stores the idempotent response. Replay is checked
before consumed-grant rejection. Existing content hashes can be reused only
when length, media type, and storage path also match.

The optional `evidence_digest` and `evidence_manifest_hashes_json` columns on
`sdd_artifact_revisions` are populated only for `review.md`. They prove the
independent review consumed the complete current manifest set. Runtime browser
results are also represented in `sdd_verification_results`; the store requires
each typed `browserCheck` result hash/status to match a manifest from the same
attempt and refuses unaccounted manifests. Recovery preserves submitted
evidence but revokes every live capability and pauses interrupted runs.

## State model

Runs advance through these phases:

```text
specification → design → planning → implementation → verification
→ review → ready → delivery → completed
```

`ready` means the work is locally implemented, verified, and independently
reviewed. It does not imply a commit, push, merge, tracker mutation, release, or
other external side effect.

Runs, tasks, and attempts use the same status vocabulary:

```text
idle | queued | running | waiting | retry_scheduled | pausing | paused
| blocked | canceling | canceled | failed | succeeded
```

## Mutation and side-effect boundary

Every mutating command carries a caller-generated request ID and expected
aggregate revision. Agentum commits the compare-and-swap transition, artifact
metadata, approval changes, durable event, idempotent response, and outbox row
in one SQLite transaction. A duplicate request is a read of the stored
response; a stale revision is rejected without changing state.

Model execution, network calls, Git operations, process execution, and
filesystem publication happen outside database transactions. Their results
return through another revision-checked transition. Creation is represented by
`sdd_create_sagas` before an external worktree or authoring attempt starts, so
a crash cannot turn those resources into invisible state.

The general session, event, notes, channel, and authentication tables remain
owned by their existing store migrations. They support Agentum's terminal and
collaboration features but are not an alternative source of SDD truth.
