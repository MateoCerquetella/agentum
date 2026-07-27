# Agentum SDD contract

Agentum owns one provider-neutral specification workflow. A saved specification has a canonical `SPC-<ULID>` identity and one artifact root in its authoritative worktree:

```text
.agentum/
├── manifest.json
└── specs/
    └── spc-<ulid>-<slug>/
        └── spec.md
```

The manifest contains only the format, schema version, and artifact-set identity. The directory slug is cosmetic. Later phases publish `design.md`, `plan.json`, `decisions.md`, and `review.md` only when those artifacts contain real information. Runtime status, credentials, approvals, external links, and delivery state belong in Agentum's database, not the manifest.

Agentum creates authoritative and attempt worktrees below its data directory. Providers run in disposable attempt worktrees through argument-vector `CommandSpec` values with an allowlisted environment, output and time limits, and process-tree cancellation. Providers do not write configuration into customer repositories and cannot directly approve or deliver their own work.

The HTTP contract is under `/api/sdd`. Mutations carry a unique `requestId` and an `expectedRevision`. Artifact writes additionally compare the expected content hash. Approval digests bind the specification revision, artifact hashes, workflow policy, and workspace fingerprint.

## Release behavior

New Spec creates an external authoritative worktree and a disposable provider attempt. A successful authoring attempt publishes a validated `spec.md`; Standard + Guarded then stops at the hash-bound specification approval. After approval, Agentum advances through the artifact, implementation, verification, and independent-review phases and stops at Ready. Commit, push, pull request, tracker, and release effects require a separate Deliver preview and confirmation.

Run, attempt, event, approval, task, lease, patch, and outbox state is durable across restart. A changed artifact invalidates its approval and downstream state; malformed or identity-changing external edits block without overwriting the file. The capability endpoint is authoritative for providers, source adapters, remote lifecycle, and delivery actions, so unavailable integrations are disabled with a reason instead of silently downgraded or reported Ready.

## Provider qualification and platform isolation

Release tags cannot publish until a hardened self-hosted runner executes
`agentum-sdd-provider-conformance` against all seven bundled provider CLIs with
real authentication. For each provider the runner copies the zero-pollution
demo fixture, proves cancellation before save, authors and validates `spec.md`,
recovers the hash-bound Guarded approval checkpoint in a fresh process, creates
design and typed plan artifacts, accepts and applies a scoped diff, proves the
acceptance test failed before and passes after implementation, runs independent
review in a separate provider process, rejects malformed output, and stops at
Ready without delivery. The uploaded report contains only source-bound hashes
and stable statuses; the reusable release workflow verifies its checksum,
source SHA, exact provider set, cases, and Ready/no-delivery state.

Custom manifests run through the same executable and lifecycle. A passing
receipt is signed with an installation-owned Ed25519 key in Agentum's secure
credential vault. Editing the manifest or evidence, copying the receipt to
another installation, losing secure storage, or changing the required suite
invalidates approval. There is no operator-supplied digest-only approval path.

Linux provider and verification processes require Bubblewrap. DEB and RPM
packages declare that dependency; AppImage and raw-binary users must install
`bwrap` themselves, and the capability remains unavailable until it is found.
Provider sandboxes mask host `/tmp`, `/var/tmp`, and all of `/run` (including
user keyrings, D-Bus, SSH-agent, Docker, and desktop Unix sockets), then mount
only the selected provider runtime, its provider-specific authentication file,
the read-only attempt, and the fixed writable staging/runtime paths. Symlinked
Node launchers are resolved through the canonical read-only runtime rather than
re-exposing the host runtime directory.
macOS uses the system Seatbelt launcher with repository read-only/provider
staging-only policy and a network-disabled verification policy. Windows has no
Agentum-enforced restricted-token/AppContainer filesystem sandbox in this
release, so the typed platform boundary is `remote_client_only` and all local
provider and verification capabilities are disabled there before adapter
lookup, staging creation, executable lookup, or spawn. Provider-native sandbox
flags and Windows process-tree cancellation are never accepted as isolation.
Windows can still start an SDD run through an exact, version-matched SSH worker
probe: remote authoring and every later phase stay on that registered host and
are projected into the desktop database. No local provider fallback is
available. Provider parity on Windows therefore requires a conformant remote
worker; a Windows-local provider run remains unavailable.

## Browser verification evidence

`plan.json` can attach provider-neutral `browserChecks` to a task. Each check binds a globally unique check ID, HTTP(S) target, AC references, load condition, viewport, one total deadline, and a closed assertion union (`page_loaded`, `text_present`, `selector_visible`, or `url_contains`). Runtime results never enter `plan.json`.

During verification Agentum issues a one-use `browser_evidence.submit` grant to the active verification attempt, launches a disposable Chromium profile, and creates a fresh cookie/storage-isolated context for every check. Before navigation, Agentum enables target-scoped CDP Fetch interception. Redirect hops and subresources are continued only for the exact approved origin; cloud metadata, cross-port, private, and other cross-origin requests are blocked before transmission. Without configuration, only literal loopback targets and `localhost` are accepted. Operators can replace that default with a comma-separated list of exact credential-free origins in `AGENTUM_SDD_BROWSER_ALLOWED_ORIGINS`.

Screenshots and bounded redacted transcripts are stored outside repositories under Agentum's data directory as immutable SHA-256 blobs. SQLite binds each manifest and blob role to the run, attempt, specification revision, check, capability grant, and durable event. Console data from the shared MCP listener is never attributed to an SDD attempt; its evidence coverage is explicitly `none`. Main-document network coverage comes from the exact isolated target. Pause, cancellation, attempt failure, successful submission, verification completion, and restart revoke any live grant.

Independent review receives the typed verification records plus the exact browser evidence manifests. `review.md` metadata records a digest and sorted manifest-hash set, and the Ready transition independently compares it to current evidence. Delivery previews include the same evidence-set digest and become stale if it changes. The Run Center Evidence view shows attribution, redacted target, assertion/AC status, coverage, manifest hashes, and lazily loads authenticated capture blobs from `/api/sdd/runs/{run_id}/evidence/{evidence_id}/blobs/{sha256}`. Local attempts require a supported local Chrome/Chromium runtime. Remote attempts require the version-matched worker to resolve and supervise Chrome/Chromium on the registered SSH host; an unavailable runtime fails verification rather than producing green evidence.

## New Spec source intake

Source intake uses a closed, versioned request shape. Callers cannot submit arbitrary provider objects, credentials, external identifiers, or claimed source revisions. Agentum accepts these source variants:

```text
{ type: "socratic", context }
{ type: "markdown", markdown }
{ type: "github", url, expectedSourceRevision? }
{ type: "linear", connectionId?, identifier, expectedSourceRevision? }
{ type: "jira", connectionId, siteId, key, expectedSourceRevision? }
{ type: "openspec", path, expectedSourceRevision? }
```

Use `POST /api/sdd/repos/{repo_id}/sources/preview` before creating a reference-backed spec. Preview is read-only and returns normalized Markdown, diagnostics, imported task/design availability, a source revision, and a deterministic digest. Creation re-reads and re-normalizes the source. If `expectedSourceRevision` no longer matches, Agentum returns `409 source_revision_changed` before allocating a spec, worktree, or durable run.

Markdown and conventional OpenSpec imports are enabled. OpenSpec paths must identify an active or archived change below the repository's `openspec/changes/` tree; traversal, symlinks, unknown files, malformed deltas, unstable reads, and oversized input fail closed. GitHub import is enabled only when Agentum verifies an authenticated `gh` session for `github.com`; the server derives external identity and revision from the provider response. Linear import uses the selected connection from Agentum's secure vault and never reads the retired plaintext token field. Jira import uses the selected, revision-bound Cloud connection and site.

The OpenSpec adapter is an independent Rust implementation; Agentum neither invokes nor depends on the OpenSpec CLI. It accepts the documented conventional `proposal.md`, `specs/<capability>/spec.md`, optional `design.md`, `tasks.md`, and `spec-driven` metadata shape. Explicit export preserves imported capability, ADDED/MODIFIED/REMOVED operation, requirement name, scenario name, design, and checklist intent when their normalized provenance is still present; native Agentum-only intent receives deterministic output plus a warning wherever the conventional format cannot carry typed scopes, commands, or AC mapping. CI validates import/export round trips against an official MIT-licensed OpenSpec fixture pinned to upstream commit [`c33fcb3fdb729455b114bdcfad84df01b3531bfe`](https://github.com/Fission-AI/OpenSpec/commit/c33fcb3fdb729455b114bdcfad84df01b3531bfe). Exact file hashes and license provenance live beside the fixture in `crates/agentum-server/tests/fixtures/openspec/official/`; release tests never fetch or execute upstream code.

New Spec requires an explicit source-checkout policy. `require_clean` refuses a dirty checkout, `committed_base` intentionally excludes uncommitted changes and uses the selected commit, and `snapshot` validates supported dirty content and creates a recoverable hashed snapshot outside the repository.

## Integration credential boundary

The embedded desktop stores SDD credentials in the operating-system credential vault. The standalone server refuses persistence unless `AGENTUM_SDD_VAULT_MASTER_KEY` is an externally supplied base64-encoded 256-bit key; its AES-256-GCM vault is written atomically with no-follow directory containment and restrictive permissions. A locked, corrupt, unavailable, symlinked, junction-backed, or improperly permissioned vault makes the affected capability unavailable. Selected-connection aliases contain only an ID and resolve the canonical secret, so token rotation and deletion cannot leave a usable duplicate. Tokens, OAuth state, refresh tokens, API-token email addresses, and device private keys never enter SQLite or capability responses.

Jira OAuth requires a credential-free HTTPS `AGENTUM_JIRA_OAUTH_BROKER_URL`. Agentum requests and validates exactly `read:jira-work`, `write:jira-work`, and `offline_access`; binds start and one-time redemption to hashed state and a vault-held Ed25519 device key; requires explicit selection for multi-site grants; and replaces rotating refresh tokens under credential-revision CAS. Jira read and delivery capabilities fail closed unless the encrypted grant and sanitized database metadata agree. The independently deployable `agentum-jira-broker` serves start, callback, one-time redemption, refresh, and health endpoints. It retains codes and tokens only in bounded process memory; its durable database contains only public device keys, refresh-token SHA-256 digests, revisions, and timestamps, never Jira issue data or recoverable tokens. Because the exact scope set omits Atlassian's separate identity scope, its returned account identity is a device-bound local grant identifier and the display label is derived from sanitized Jira sites. Production TLS, Atlassian registration, DNS, and secrets remain operator-owned; the fail-closed container and reverse-proxy deployment is documented in [`deploy/jira-oauth-broker/README.md`](../deploy/jira-oauth-broker/README.md).

Advanced email/API-token authentication is disabled by default. A local desktop or self-hosted operator may explicitly set `AGENTUM_JIRA_ALLOW_API_TOKEN_AUTH=true`; the UI then displays a warning and requires an acknowledgement before `POST /api/sdd/integrations/jira/api-token/connect` accepts a tenant URL, email, and API token. Agentum validates the credential directly against that exact `*.atlassian.net` tenant, stores it only in the secure vault, and never sends it through the OAuth broker. Jira mutations still require a hash-bound Deliver preview and confirmation.

At Ready, the Run Center composes a closed delivery intent (`commit`, `push`, `pullRequest`, `trackerComment`, `trackerStatus`, `trackerFieldUpdate`, `release`, or one-shot `openSpecExport`) and sends the exact typed actions in `previewDelivery`. Confirmation can select only action IDs returned by that preview token. No delivery action is inferred from a tracker or run state.

Remote Ready runs use the same preview/confirm contract through the fixed SSH
subsystem. Before issuing a token, Agentum asks the registered worker for a
side-effect-free snapshot and binds the actor, aggregate revision, approved
plan, remote workspace hash, hidden worktree-identity hash, branch, projected
artifact hashes, host-computed artifact-set hash, and any OpenSpec destination
check into the digest. Confirmation repeats that inspection. A remote URI is
never opened as a desktop path: commit, push, pull request, release, and
OpenSpec publication execute only in the worker's registered authoritative
worktree. Tracker APIs remain desktop integrations and use an Agentum-owned
neutral working directory, never a substitute local checkout.

Each confirmed remote repository action has a typed, bounded request and a
durable host-side idempotency record. Dependencies must have a durable
successful result. Definite failures and ambiguous network outcomes leave the
run at Ready; an explicit retry increments the attempt identity and reconciles
the stable commit trailer, remote head, PR/release marker, or export hashes.
After a desktop restart, an interrupted local action claim becomes
`sync_pending`; the same preview can retry it without rerunning successful
dependencies. Delivery can never manufacture a green lifecycle transition or
downgrade to local Git/filesystem execution.

Only sanitized provenance is placed in `spec.md` frontmatter and `sdd_specs`. Provider-derived work-item identity is recorded in `sdd_external_links`; immutable normalized Markdown/OpenSpec snapshots are recorded in `sdd_import_jobs` in the same transaction as the spec, run, approval, event, and outbox. Raw caller objects and credentials are not persisted.

The hard cutover includes a hash-accounted migration tool for this repository's retired SDD roots. Preview is read-only. Apply refuses dirty, active, symlinked, special, or unaccounted legacy content; archives every source outside the repository; publishes `.agentum` atomically; and then removes only verified inventoried files.

## Restricted-content release check

Run the repository boundary check in every build:

```sh
scripts/check-sdd-boundary.sh --boundary-only
```

Release owners also supply a deny-pattern file from outside the checkout. The file can contain private names, URLs, credential fingerprints, or other organization-owned comparison material without committing that material to Agentum:

```sh
scripts/check-sdd-boundary.sh /secure/release/restricted-patterns
```

The scanner reports matching file names only. The supplied patterns and matching lines are not printed.

## Remote SSH worker deployment

Remote SDD execution uses the versioned `agentum-sdd-worker` executable and
the fixed OpenSSH subsystem name `agentum-sdd-v1`. The desktop never sends a
repository path or generated shell command. The administrator registers each
repository by its desktop-provided SHA-256 identity and one stable 26-character
artifact-set ULID in an owner-only configuration file:

```json
{
  "schemaVersion": 1,
  "hostId": "00000000-0000-4000-8000-000000000000",
  "repositories": [
    {
      "identitySha256": "<64 lowercase hex characters>",
      "artifactSetId": "<26-character ULID>",
      "path": "/srv/projects/example"
    }
  ]
}
```

The signed release roster currently publishes
`agentum-sdd-worker-<version>-linux-x64` for x86_64 Linux remote hosts. Verify
that file against the release's `SHA256SUMS`, install it as mode `0755`, write
the configuration as the SSH worker account with mode `0600`, and first run
both checks locally:

```sh
/usr/local/libexec/agentum-sdd-worker --version
/usr/local/libexec/agentum-sdd-worker --check-config --config /etc/agentum/sdd-worker.json
```

Linux aarch64 is not a published worker architecture in this release. Agentum
therefore keeps remote lifecycle unavailable on those hosts unless an
administrator builds the exact tagged source for aarch64, verifies it, and
installs a version-matched worker. No architecture fallback or emulation is
selected automatically. Desktop installers and updater archives do not deploy
or configure a worker on any SSH host; installation and `sshd_config` changes
are always an explicit administrator action.

Then configure `sshd_config` with an absolute executable and configuration
path, validate the SSH daemon configuration, and reload it using the host's
normal service manager:

```text
Subsystem agentum-sdd-v1 /usr/local/libexec/agentum-sdd-worker subsystem --config /etc/agentum/sdd-worker.json
```

The executable refuses linked, group-readable, non-owner, oversized, or
malformed configuration; repository paths are opened only from this
registration. The protocol is a closed, length-prefixed JSON union and runs at
global concurrency one. Durable request replay, phase sequencing, attempt
paths, rollback preimages, patch state, and the cross-process lease live in
`sdd-worker.sqlite` below Agentum's data directory. EOF, timeout, and explicit
cancellation terminate provider and verification process trees.

`GET /api/sdd/repos/{repo_id}/remote-capability?provider=<id>` performs a
real fixed-subsystem probe and reports the exact worker version, repository
registration, stable artifact-set identity, and current provider readiness.
When that exact probe succeeds, New Spec can author remotely and the desktop
atomically projects the authored specification, approvals, phase artifacts,
task completion, verification evidence, independent review, and Ready
checkpoint into the main SDD aggregate. Design through review is sequential in
this release. Typed browser checks execute on the registered host; only
bounded, redacted, content-addressed evidence crosses the subsystem boundary.

Active request identity and checkpoints are durable. On desktop restart an
in-flight request is marked interrupted and the run pauses; an explicit resume
reuses the same phase identity and the worker's durable replay record before
advancing. A changed worker version, repository registration, provider,
artifact-set identity, or base commit fails closed. Agentum never falls back to
a desktop-local provider, Git checkout, or repository filesystem for a remote
run.
