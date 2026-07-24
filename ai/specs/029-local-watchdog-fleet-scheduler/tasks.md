# Spec 029 Tasks — Local Watchdog Fleet Scheduler

- **Status:** Architect planned
- **Order:** F1 -> F2 -> F3; do not parallelize these slices
- **Architecture:** `architecture.md`

## F1 — Typed local tmux batch protocol and runner

**Acceptance criteria:** AC1, AC3, AC4, part of AC11

- [ ] Add `crates/agentum-tmux/src/local_batch.rs` and export it from `lib.rs`.
- [ ] Define request ids, validated `%<digits>` pane identity, `BatchSample`, total
  `BatchOutcome::{Sample,Gone,Retry}`, action outcome, `BatchProbe`, `BatchResult`, boxed-future
  `LocalBatchRunner`, and production `TmuxLocalBatchRunner`.
- [ ] Generate a fresh UUID-v4 nonce for every physical invocation and implement the exact
  `AGENTUM-BATCH/1` probe and confirmation/action grammars from Architecture §2.3–2.4.
- [ ] Build one direct tmux argv command sequence for every probe. Do not invoke a shell and do not
  call a one-target helper inside the target loop.
- [ ] Implement strict state-machine parsing: known request ids only, exact nonce, record order,
  complete sections, matching begin/end pane identity, close record, and total outcomes.
- [ ] Implement the optional one-invocation `finish`: authoritative exact-target confirmation for
  provisional absence and one identity-guarded compact per admitted unique target.
- [ ] Add an internal recording subprocess seam. Assert exact argv requests and invocation counts
  for 100 targets, both with and without the optional finish.
- [ ] Add parser fixtures for valid multi-target captures; raw delimiter/record collisions; wrong
  nonce/request; malformed pane id; duplicate/reordered/truncated/partial frames; target absent;
  disappearance/replacement; mixed good/bad targets; action delivered/retry; and transport/UTF-8
  failure. Assert no cross-target attribution.
- [ ] Run `cargo fmt --all -- --check`, focused `agentum-tmux` tests, and `git diff --check`.

**F1 gate:** every request has one typed final outcome; 100 healthy probes record one local tmux
invocation and an action/confirmation cycle records at most two.

## F2 — One local fleet scheduler

**Acceptance criteria:** AC1, AC2, AC5, AC6, AC8, part of AC11

- [ ] Add `crates/agentum-watchdog/src/fleet.rs` with `FleetState`, monotonic checked generations,
  local registration state, target grouping, pending compact metadata, and injected
  `Arc<dyn LocalBatchRunner>`.
- [ ] Before wiring the fleet, extract the existing `watch_session` mutable state and ordered body
  into one private `SessionMachine`/helper. Both the new local path and retained remote task must
  call it in this slice; no copied classifier/effect body is allowed.
- [ ] Change `Watchdog` reconciliation to retain the authoritative Running query and callback,
  maintain local registrations, and retain task handles only for SSH registrations.
- [ ] Preserve constructor behavior. Add a test-only runner constructor/seam without changing
  server boot wiring.
- [ ] Initialize every local registration deadline to `tokio::time::Instant::now() + 1s`; implement
  earliest-deadline waking without a spin loop.
- [ ] Snapshot due registrations, deduplicate exact target strings, make one `probe`, then
  generation-filter and fan out accepted results to independent due/current registrations.
- [ ] Queue local unmanaged compaction on context low. Admit it only after a current non-crash
  sample in the next target batch; dedupe delivery by target; retain contributor generations.
- [ ] Hold the fleet commit boundary through `finish` and effects. Remove/retarget/non-Running
  reconciliation must invalidate the prior generation and pending action before it returns.
- [ ] Add paused-time tests for no initial early sample; local active/recent 1s; settled 2s;
  completion-based deadlines; Retry cadence; and no empty-fleet spin.
- [ ] Add 100-session tests for unique targets and shared-target dedupe/fanout, including independent
  tool/activity/cooldown state.
- [ ] Add a barrier fake covering in-flight removal, non-Running, deletion, retarget, and
  re-registration. Assert zero stale effects/actions after removal completes.
- [ ] Remove the local per-session task route in the same slice. Add a source guard proving no
  local reconciliation spawn/sample loop remains.
- [ ] Run focused watchdog scheduler tests, focused tmux tests, fmt, and diff hygiene.

**F2 gate:** one local scheduler owns all local deadlines; 100 due local sessions use at most two
runner invocations; no stale generation can act after authoritative removal.

## F3 — Ordered behavior and remote compatibility closure

**Acceptance criteria:** AC6, AC7, AC9, AC10, AC11 and full regression coverage

- [ ] Audit the F2 `SessionMachine` extraction against the former `watch_session` line by line.
  Reuse `bottom_lines`, `classify_activity`, hashes, adapter signatures, tool canonicalization,
  intentional-stop handling, and `emit`; remove any duplicate or obsolete local helper.
- [ ] Route accepted local samples through queued-local compaction and remote samples through the
  existing immediate SSH compaction sink.
- [ ] Preserve exact order: Gone/crash -> prior delivered compact/current context -> tool drift ->
  activity -> deadline. A terminal result retires the registration/task immediately.
- [ ] Pin the existing event names and payloads: `session.started`, `session.crashed`,
  `watchdog.compact`, `harness.context_rotation_requested`, `session.tool_changed`,
  `agent.working`, `agent.finished`, `agent.awaiting_input`, and `agent.input_resolved`.
- [ ] Assert local Gone intentional-stop silence and one non-intentional Crashed+target-clear+
  persisted/broadcast `pane_exited` event; assert a pane race Retry changes nothing.
- [ ] Assert compact delivery starts the five-minute per-session cooldown only on success; shared
  delivery sends once but emits for each current contributor; managed sessions never receive it.
- [ ] Assert crash precedence, context precedence, two-sample tool drift before same-sample activity,
  crash/tool/activity payloads, and the complete initial/working/finished/awaiting/resolved matrix.
- [ ] Keep remote sessions on `agentum_tmux::ssh::sample_pane` and streaming ControlMaster with
  unchanged 3/6s cadence. Add a recording test proving no remote registration enters the local
  batch and no SSH invocations are added.
- [ ] Update `CLAUDE.md` crate-map/watchdog description for local fleet + retained remote workers.
- [ ] Append harness features in this order:
  `local-tmux-batch-protocol`, `local-watchdog-fleet-scheduler`,
  `watchdog-fleet-behavior-compatibility`.
- [ ] Extend `.harness/verify.sh` with focused tmux/watchdog tests, the local-loop source guard,
  `cargo fmt --all -- --check`, `cargo test --workspace --lib --exclude agentum-desktop` when the
  known Sherpa dylib blocks desktop, and `git diff --check`.
- [ ] Extend `.harness/qa.sh` with an isolated HOME/AGENTUM_HOME/TMUX_TMPDIR 100-session fake-runner
  scenario. A real local tmux smoke is optional and must skip explicitly when tmux is absent.
- [ ] Run the focused suites, isolated QA for all three features, backend workspace tests,
  `cargo check -p agentum-watchdog -p agentum-tmux`, fmt, shell/JSON validation, source guard, and
  `git diff --check`. Record the known full-workspace/UI blockers, do not claim those gates green.

**F3 gate:** AC1–AC11 are executable and green; local process invocation count is constant; remote
sampling and every public lifecycle effect remain compatible.

## Final Developer evidence checklist

- [ ] `cargo test -p agentum-tmux local_batch --lib -- --nocapture`
- [ ] `cargo test -p agentum-watchdog fleet --lib -- --nocapture`
- [ ] `cargo test -p agentum-watchdog --lib -- --nocapture`
- [ ] `HARNESS_FEATURE_ID=local-tmux-batch-protocol .harness/verify.sh`
- [ ] `HARNESS_FEATURE_ID=local-watchdog-fleet-scheduler .harness/verify.sh`
- [ ] `HARNESS_FEATURE_ID=watchdog-fleet-behavior-compatibility .harness/verify.sh`
- [ ] Run the same three ids through `.harness/qa.sh`
- [ ] `cargo test --workspace --lib --exclude agentum-desktop`
- [ ] `cargo check -p agentum-tmux -p agentum-watchdog`
- [ ] `cargo fmt --all -- --check`
- [ ] `jq empty .harness/feature_list.json`
- [ ] `bash -n .harness/verify.sh .harness/qa.sh`
- [ ] local per-session-loop source guard
- [ ] `git diff --check` and `git status --short`
