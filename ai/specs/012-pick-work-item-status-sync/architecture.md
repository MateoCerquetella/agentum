# Architecture — Spec 012: Pick the work item, sync its status through the session lifecycle

- **Spec:** `ai/specs/012-pick-work-item-status-sync/spec.md`
- **Status:** Architect
- **Surfaces:** `crates/agentum-desktop/ui` (New Workspace issue picker) · `crates/agentum-server` (`TrackerPhase::InReview`, session-start reactor, PR/merge poller, worktree bind coords) — a thin front-end pick + a thin lifecycle layer, both sitting **on top of spec 010's already-shipped Projects v2 binding and write path**.

> **Grounding caveat (binding on every phase after this one).** This blueprint was written in the `new-chat-refresh` worktree, which is **59 commits behind `origin/develop` and is missing specs 009 and 010**. Spec 010 (RELEASED v0.60.0) already shipped: the per-repo Projects binding `{project_id, status_field_id, status_mapping}`, the `updateProjectV2ItemFieldValue` **Project-column write *inside* `apply_tracker_transition`**, the fuzzy option-ID discovery + **nearest-earlier-phase fallback**, and the `done_closes_issue` knob. **None of it is visible in this tree.** Every `file:line` below is approximate. **Reuse-010-over-rebuild is the #1 rule of this spec** — the Developer must re-ground each `:line` on fresh `origin/develop` and, before writing any Projects/binding code, confirm 010 didn't already build it. This spec *extends* 010's mapping with one phase and *drives* 010's seam from two new triggers; it builds **nothing** that 010 already owns.

---

## 1. Design overview — two halves that meet at one seam

The feature is two thin layers bolted onto one existing write seam. It adds no new write path — every status change on GitHub/Linear/Projects flows through the **single** function `agentum_server::task_sink::apply_tracker_transition`, which 010 already taught to move the Project Status column.

```
HALF A — pick & bind (F1, UI + registry)
  Wizard step-3 Tracker picker  (active Project's open issues, PRs excluded)
     → applyLinkedWorkItem(item)            [useComposerState.ts, existing attach seam]
     → createWorktree(...)                  [store/slices/worktrees.ts]
     → POST /api/worktrees/create           [routes/worktrees.rs]
     → registry Worktree persists:  linked_issue  +  tracker_provider  +  tracker_url
                                    (the coords a later transition needs)

HALF B — lifecycle → write-back (F2/F3/F4, agentum-server)
  session start ─┐
  PR opens ──────┤→ resolve_binding(worktree) → (provider, tracker_url)
  PR merges ─────┘        │
                          ▼
        next_phase_write(current_phase, target)  ← monotonic-forward guard (pure)
                          │ Some(phase)
                          ▼
        apply_tracker_transition(store, provider, tracker_id, tracker_url, phase)
                          │
        ┌─────────────────┼─────────────────────────────┐
        ▼                 ▼                              ▼
   status/* label     Linear state                Project Status option   ← 010's arm, FREE
   (github arm)       (linear arm)                (010, for a bound repo)      for a bound repo
                                                  + issue close on Done (010 done_closes_issue)
```

**The meeting point is `apply_tracker_transition`.** Half A persists just enough on the worktree that Half B can call that seam with `(provider, tracker_url, phase)`. Because 010 already wired the Projects column write *inside* that seam with **zero call-site edits**, every transition 012 fires — for a 010-bound repo — moves the board column *for free*. 012 writes the labels/Linear/Project column trigger machinery; it never writes the column code.

The **only genuinely new backend subsystem** is a single module — `crates/agentum-server/src/tracker_sync.rs` — holding (a) the session-start **reactor** (a lifecycle-bus subscriber → InProgress) and (b) the PR/merge **poller** (a timer loop → InReview → Done). Everything else is a reuse of 010's write path plus small extensions (`TrackerPhase::InReview`, three `#[serde(default)]` registry fields, one TS picker model).

---

## 2. Non-negotiable invariants (numbered — regressing any one reintroduces a paid-for bug)

1. **Reuse 010, never rebuild it.** Do not re-implement the Projects binding, the `status_mapping`, the `updateProjectV2ItemFieldValue` write, the option-ID discovery, or `done_closes_issue`. 012 only *adds* an `InReview` entry to the mapping and *calls* the existing seam. Re-ground on `origin/develop` before touching anything Projects-shaped.
2. **One launch path.** The InProgress trigger is a **bus subscriber**, never inline gh in `routes::sessions::spawn_agent_into_pane`. No tracker/gh code may enter the launch path or be able to throw into it. YOLO translation, `pane_env`, `--settings`, MCP wiring stay untouched.
3. **Idempotent · best-effort · never-halt.** Every transition (session-start, PR-open, merge) is idempotent; a failed label/Linear/Projects/gh call logs (`tracing` + `HarnessEvent::Log` where a run context exists) and the session/gate/poll proceeds. No transition throws into launch or into the poll loop.
4. **Monotonic-forward, no-thrash.** Status only ever advances (`next_phase_write` guard). A plain session reopened in a Done worktree must not drag the card back to InProgress; a harness ReadyToTest must not regress to InReview when a PR opens; the session-start InProgress must converge with the harness's own InProgress (same phase → no-op).
5. **Fail-closed binding.** An unparseable/missing remote, no `activeProject`, or an empty Project yields **no bind and no transition** — never a fabricated or wrong-issue one. Mirrors `deriveWizardTracker` / `BaseRefPicker`.
6. **No webhooks → poll only.** PR-open/merge is a bounded, backed-off, best-effort `gh` loop (self-hosted ⇒ no inbound webhooks — `board_sync.rs:14`). Do **not** reintroduce `capture-pane`-style push-snapshot polling; this is a separate GitHub-API poll, not a pane poll.
7. **Registry serde-alias-FREE.** New `Worktree` fields are `Option<String>` with `#[serde(default)]` and **no `#[serde(alias)]`** (spec-004 lesson: an alias/dup field wipes the registry to `[]`).
8. **`gh` behind the existing seam.** The poller invokes `gh` through the same resolver `task_sink` already uses (`gh_bin()` / the fake-`gh` subprocess indirection), never a hardcoded `"gh"`, so tests inject a stub. (Note recon follow-up #277 — reuse the existing seam, don't add a fourth `gh_bin` dup.)

---

## 3. Per-feature design home (every AC → concrete seam)

Legend: **[reuse]** = call/extend existing code; **[build]** = new code; `~:` = approximate line, re-ground on develop.

### F1 — Pick & bind the work item

| AC | Home | Reuse / build |
|----|------|---------------|
| **1** issue picker over active Project, PRs excluded, from the wizard's step-3 Tracker section | `CreateWorkspaceWizard.tsx` step-3 Tracker (`~:890-929`, display-only today) renders a picker fed by the `github` slice `projectViewCache` scoped to `settings.githubProjects.activeProject`. Source rows from **[reuse]** `gh_get_project_view_table` (`commands/gh_projects.rs:~760`). | **[build]** picker UI; **[reuse]** read path + `ProjectPicker` active-project selection. |
| **2** selecting binds it; persisted on the worktree with coords for label + Project targets | Select → **[reuse]** `applyLinkedWorkItem(item)` (`useComposerState.ts:~1312`, the one attach seam) → on create **[reuse]** `createWorktree(...)` (`store/slices/worktrees.ts`) threads the item → `api.worktrees.create` → **[build]** `routes/worktrees.rs` create handler writes `linked_issue` **[reuse]** + `tracker_provider="github"` + `tracker_url=<issue url>` **[build]** onto the registry `Worktree`. | see §6. **Do not** flip the wizard to a gated run; it stays a plain create. |
| **3** picking optional, non-fatal; empty/unreachable Project shows honest empty state, never blocks | Picker returns `[]` when no `activeProject` / gh unavailable / remote repo; step advances with no bind. Mirrors `deriveWizardTracker` fail-closed. | **[build]** empty-state; **[reuse]** the wizard's existing non-blocking step contract. |
| **4** pure model, jsdom-free, `bunx vitest` green | **[build]** `components/new-workspace/work-item-picker-model.ts` (+`.test.ts`): `deriveIssueOptions(projectView) → WorkItemOption[]` (open **issues** only) and `buildBindPayload(row) → LinkedWorkItem`. | Same seam as the existing `create-workspace-wizard-model.ts`. |

> **Consistency addition:** the reverse entry `launchWorkItemDirect` (`lib/launch-work-item-direct.ts:~193`, Project-board → pre-linked workspace) must populate the **same** `tracker_provider`/`tracker_url` coords, so a board-launched workspace also drives status. One shared `buildBindPayload` covers both entry paths.

### F2 — In Progress on session start (any bound session)

| AC | Home | Reuse / build |
|----|------|---------------|
| **5** session-start hook on the one launch path fires InProgress for a bound worktree; resolves `(provider, tracker_url)`; writes both label + Project option via 010 | **[build]** `tracker_sync.rs` **reactor**: subscribes to the session-lifecycle bus, maps `session.workdir → worktree`, calls `resolve_binding` → `apply_tracker_transition(..., InProgress)` **[reuse]**. See §5. | Trigger = bus event, **not** inline gh in spawn (inv. #2). |
| **6** fires for a plain workspace (no gated run); idempotent; converges with harness InProgress; no-thrash (fake-`gh` asserted) | Guarded by **[build]** pure `next_phase_write(current, target)` (§4) + persisted `tracker_phase`. Harness's own InProgress (`harness/drive.rs ~:133`) stays as-is (idempotent); the persisted-phase guard dedupes both. | inv. #4. |
| **7** best-effort/never-halt; unbound worktree = silent no-op | `apply_tracker_transition` is already best-effort (`Ok(Skipped)` on non-applicable); reactor wraps its own resolve in `if let Some((p,u)) = resolve_binding(...)` and logs any `Err`. | inv. #3. |

### F3 — In Review on PR open

| AC | Home | Reuse / build |
|----|------|---------------|
| **8** new `TrackerPhase::InReview` + `status/in-review` label + Linear "In Review" + InReview entry in 010's `status_mapping` (option-ID discovery + nearest-earlier fallback → InProgress) | **[build]** add variant to `TrackerPhase` (`task_sink.rs:~218`); **[build]** `status/in-review` in `GithubStateMap` (now **five** mutually-exclusive labels); **[build]** Linear "In Review" in `LinearStateMap` (configurable, fail-closed skip); **[reuse]** 010's discovery to resolve an InReview option — see §4 for the ordering + collision handling. | Do not add new discovery code — reuse 010's. |
| **9** PR-open detector: bounded poll of `gh pr list --head <branch>`; first non-draft PR → persist `linked_pr` + fire InReview | **[build]** `tracker_sync.rs` **poller** — see §7. Persists `linked_pr` **[reuse existing field]**. | Poll is the sanctioned model (inv. #6). |
| **10** poll bounded, best-effort, backed off; gh failure logs without halting (fake-`gh`-nonzero asserted) | Poller loop: per-call timeout + per-tick cap + loop backoff on repeated failure; each error logged + skipped. | inv. #3, #6. |

### F4 — Done on merge

| AC | Home | Reuse / build |
|----|------|---------------|
| **11** poller detects merge (`gh pr view <n> --json state,mergedAt` → `MERGED`) → InProgress→Done for the bound item; moves label + Project option to Done; closes issue per 010's `done_closes_issue` | **[build]** merge branch of the poller → `apply_tracker_transition(..., Done)` **[reuse]**. Issue-close is 010's, not new. | The PR's own `Closes #N` also closes it — both are fine (idempotent close). |
| **12** terminal: after Done, stop polling that PR; suite + vitest green | `tracker_phase == "done"` excludes the worktree from enumeration; `next_phase_write(Some(Done), _) = None`. Restart-safe because `tracker_phase` is persisted. | inv. #4. |

---

## 4. The `InReview` phase design (ordering, mapping, collision)

**Add the variant with a fixed position in the canonical order:**

```
Todo(0) < InProgress(1) < InReview(2) < ReadyToTest(3) < Done(4)   ( + Blocked, off the line )
```

This ordering is chosen so InReview's **nearest-earlier mapped phase is InProgress** — exactly the fallback the spec pins (`InReview → InProgress`). It also makes ReadyToTest sort *after* InReview, which is the correct monotonic behaviour: a **gated** run that reached ReadyToTest (unit-green, pre-QA) must **not** visually regress to InReview when a PR opens, while a **plain** session walks InProgress → InReview → Done cleanly. Done(4) always wins over everything (merge is terminal).

**Per-provider mappings:**

- **GitHub label:** `status/in-review`. The github arm now maintains **five** mutually-exclusive `status/*` labels — adding `status/in-review` removes the other **four** (todo, in-progress, ready-to-test, done). Update the "remove the others" set from three to four. Idempotent label-ensure as before.
- **Linear:** add an "In Review" name to `LinearStateMap` (default `"In Review"`, overridable via `linear.json` `state_map` / `AGENTUM_LINEAR_STATE_*`). A team with no such state is a **logged skip**, not an error (fail-closed, same as 010's missing-state handling).
- **Projects (010):** add an `InReview` entry to `status_mapping`, resolved by **010's existing** single-select option-ID discovery with fuzzy tokens `{"in review","review","reviewing","code review","pr open"}`, and 010's **nearest-earlier-phase fallback** (→ InProgress) when no review-ish option exists. **No new discovery code and no human board-config is required to ship.**

**Collision with 010's ReadyToTest→"review" mapping — resolved, non-fatal.** 010 already maps a "review"-ish column to ReadyToTest. If Mateo's board has only one review-ish option, InReview's fuzzy match may resolve to the *same* option ID as ReadyToTest. This is a **fold**, and it is **safe**: (a) each phase resolves independently — InReview resolving to option X does **not** displace ReadyToTest's mapping; (b) in practice a plain session never sets ReadyToTest (no gated run) and a gated run rarely opens a PR mid-gate, so the two states rarely coexist on one card; (c) if the board *does* have a distinct "In Review" option, the fuzzy match prefers it automatically with zero code change. **Working default ships without any board reconfiguration.** Carry-forward to Mateo/reviewer (§11): if he wants a visually distinct **In Review** column, he adds the option to his Project — the fuzzy discovery picks it up, no code edit.

---

## 5. Session-start → InProgress seam

**Where it hooks:** the **session-lifecycle event bus**, not inline in `spawn_agent_into_pane` (inv. #2 — nothing that can throw may enter the launch path). Precedent: the harness already subscribes to this bus for settle detection.

**Trigger event:** emit/consume a `SessionStarted { session_id, workdir }` signal at the *tail* of `spawn_agent_into_pane`. The Developer must first check on `origin/develop` whether a clean "started/created" broadcast already exists (the UI sidebar reacts to session lifecycle, so one likely does). If it does → subscribe to it. If not → add a **one-line pure broadcast** (a channel send, no gh, no fallible work) at the end of the spawn. Either way the launch path gains no failure surface.

**Resolution chain (in the reactor):**

```
SessionStarted{workdir}
  → registry lookup: worktree whose path == workdir
  → resolve_binding(worktree):
        if tracker_provider.is_some() && tracker_url.is_some()
            → Some((provider, tracker_url))         // github or linear
        else → None                                  // silent no-op (AC7)
  → guard: next_phase_write(worktree.tracker_phase, InProgress)
        → None  ⇒ skip (already ≥ InProgress: converges with harness, prevents Done→InProgress regress)
        → Some(InProgress) ⇒ apply_tracker_transition(..., InProgress); persist tracker_phase="in_progress"
```

- **`(provider, tracker_url)` come straight off the worktree** (persisted at F1 bind time, §6) — **no `git remote get-url` per event** (spec 009 killed that N×remote sweep for TCC-prompt-storm reasons). The slug (owner/repo) needed for labels/Projects is parsed from `tracker_url`, exactly as the github arm already does.
- **Idempotency + no-thrash:** the persisted-`tracker_phase` monotonic guard makes a re-start / reconnect / multi-tab a no-op (skip), and makes the session-start InProgress and the harness's own InProgress converge on the same phase. The transition itself remains idempotent as a second line of defence.
- **Best-effort:** the reactor never blocks the session; `resolve_binding` short-circuits to `None` for unbound worktrees; any `apply_tracker_transition` `Err` is logged and dropped.

---

## 6. Bind persistence design (what F1 must persist)

**Question:** does `linked_issue` + remote suffice, or must we persist the Project item id / issue url / provider?

**Decision — persist the minimal write coords, do NOT persist the Project item id.** Add **three** `#[serde(default)] Option<String>` fields to registry `struct Worktree` (`routes/worktrees.rs:~46`), all **serde-alias-FREE**:

| Field | Value | Why |
|-------|-------|-----|
| `tracker_provider` | `"github"` \| `"linear"` | Dispatch key for `apply_tracker_transition`; explicit beats inferring from URL. |
| `tracker_url` | canonical issue URL | The **exact** argument `apply_tracker_transition(.., tracker_url: Option<&str>, ..)` already takes. The github arm parses owner/repo/number from it (labels + Projects item-resolve); Linear uses it directly. Persisting it once avoids a per-event `git remote` call. |
| `tracker_phase` | last-written canonical phase, lowercase | The monotonic no-thrash guard (§5) + poller terminal-stop (§7), **restart-safe**. Earns its keep three ways. |

Existing `linked_issue` / `linked_pr` / `linked_linear_issue` are retained (`linked_pr` is written by the poller on PR-open).

**Why NOT persist the Project item id** (open question 4): 010's Projects write inside `apply_tracker_transition` resolves/attaches the item with **idempotent `addProjectV2ItemById`-by-content** from the repo binding + the issue's node id. A persisted item id would (a) go stale if the item is removed/re-added, (b) duplicate a resolution 010 already owns, and (c) couple the *registry* to Projects internals. The one extra `gh` call per Projects write to re-resolve is negligible and 010 already pays it. **Decisive default: don't persist it.** *Developer confirm on develop:* if 010's write path accepts a pre-resolved item id and *skips* its resolve when present, then an optional `project_item_id: Option<String>` (serde-default, no alias) is a permissible pure optimization — otherwise omit it.

**Serde safety (inv. #7):** all new fields are `Option<String>` + `#[serde(default)]`, **no `#[serde(alias)]`**. An old registry missing these fields must deserialize each to `None`, **not** wipe the registry to `[]`. A Rust test in F1 asserts an old-shape JSON round-trips to `None` and preserves the worktree list.

---

## 7. The poller design (the one new backend subsystem)

**Placement — a background worker in `crates/agentum-server`, spawned at server boot, sibling to `board_sync` / host-metrics.** Module: `crates/agentum-server/src/tracker_sync.rs` (co-located with the session-start reactor — both resolve bindings from the registry and call `task_sink`, so they share `resolve_binding`, `next_phase_write`, and the persisted-phase guard).

**Why not `agentum-watchdog`:** the watchdog is a tmux-pane-tail subsystem with zero `gh`/tracker/registry coupling; adding a GitHub-API poller there would violate crate boundaries and drag `gh`/store deps into it. **Why not git-route-triggered:** a PR is typically opened externally (`gh pr create` in the terminal, or the GitHub UI) — agentum never observes that event, so there is no reliable route to hang a trigger on. A poll is the only correct model (inv. #6).

**Loop (default cadence 45 s, env `AGENTUM_TRACKER_POLL_SECS`):**

```
every N secs:
  worktrees = registry.list().filter(w =>
       w.tracker_provider == Some("github")     // GitHub-only PR detection in v1
    && w.branch.is_some()
    && w.tracker_phase != Some("done"))          // terminal-stop, restart-safe (F4 AC12)
  for w in worktrees (bounded: cap ≤ K per tick, per-call timeout ~10s):
     (owner, repo) = parse_slug(w.tracker_url)
     if w.linked_pr is None:
        pr = gh pr list --head <branch> --repo owner/repo
                --json number,state,isDraft,url            // best-effort; nonzero → log+continue
        if pr.exists && !pr.isDraft:
           persist linked_pr = pr.number
           if next_phase_write(w.tracker_phase, InReview):
              apply_tracker_transition(.., InReview); persist tracker_phase="in_review"
     else:
        v = gh pr view <linked_pr> --repo owner/repo --json state,mergedAt
        if v.state == "MERGED" (or mergedAt present):
           if next_phase_write(w.tracker_phase, Done):
              apply_tracker_transition(.., Done); persist tracker_phase="done"   // terminal
  on repeated gh failure: exponential loop backoff (rate-limit friendly)
```

- **Works for remote/SSH worktrees too:** the poll queries GitHub by `(repo, head-branch)`, not the local git dir — so it fires once the branch is *pushed*, regardless of local vs SSH checkout. No push ⇒ no PR ⇒ no InReview (a natural, correct gate).
- **Branch collisions** across repos are disambiguated by `--repo owner/repo`.
- **Bounded + best-effort + never-halt:** per-call timeout, per-tick cap, loop-level backoff; every `gh` nonzero is logged and skipped; the loop never panics or halts (inv. #3, #6).
- **Draft PRs** (open question 5): `isDraft == true` is **not** a trigger; the **first non-draft** PR → InReview.
- **`gh` via the seam** (inv. #8): the poller resolves the binary through `task_sink`'s existing `gh_bin()` indirection so the fake-`gh` subprocess pattern injects a stub in tests.

---

## 8. Build order — four independently gated slices

Each slice is a `feature_list.json` entry (matching the spec's harness wiring). Gate commands per slice below; the FIRST failing test to write is named so the Developer starts red.

**Shared gate vocabulary:** backend = `cargo test -p agentum-server --lib` (fake-`gh` subprocess pattern from `task_sink` tests); UI build = `bun run build --prefix crates/agentum-desktop/ui`; UI model = `bunx vitest run`. **No `tsc` gate** (`shared/*` is a vite alias, unresolvable by bare tsc — grep-pin instead of typecheck).

### Slice F1 — `pick-work-item` (AC 1–4)
- **First failing test:** `work-item-picker-model.test.ts` → `deriveIssueOptions excludes PRs and closed issues` (pure, jsdom-free).
- Then: `buildBindPayload shapes a LinkedWorkItem`; a Rust test `worktrees::tests::create_persists_tracker_coords_without_wiping_registry` (asserts new fields persist AND an old-shape registry round-trips to `None`).
- **Gate:** `bunx vitest run` green + `bun run build --prefix crates/agentum-desktop/ui` succeeds + `cargo test -p agentum-server --lib` green.

### Slice F2 — `in-progress-on-start` (AC 5–7)
- **First failing test:** `tracker_sync::tests::next_phase_write_is_monotonic_and_idempotent` (pure: `None→InProgress=Some`, `InProgress→InProgress=None`, `Done→InProgress=None`).
- Then: `tracker_sync::tests::session_start_fires_inprogress_for_bound_worktree` (fake-`gh`), `..._is_no_op_for_unbound_worktree`, `..._converges_with_harness_inprogress_no_thrash`.
- **Gate:** `cargo test -p agentum-server --lib` green.

### Slice F3 — `in-review-on-pr` (AC 8–10)
- **First failing test:** `task_sink::tests::inreview_writes_in_review_label_and_removes_other_four` (fake-`gh`).
- Then: `tracker_sync::tests::poll_open_nondraft_pr_fires_inreview_and_persists_linked_pr`, `..._skips_draft_pr`, `..._gh_failure_never_halts` (fake-`gh` exits nonzero).
- **Gate:** `cargo test -p agentum-server --lib` green.

### Slice F4 — `done-on-merge` (AC 11–12)
- **First failing test:** `tracker_sync::tests::poll_merged_pr_fires_done_then_stops` (fake-`gh`: `pr view → MERGED` → Done fired once; second tick excluded by `tracker_phase == "done"`).
- Then: `..._done_closes_issue_when_knob_on` (reuse 010's path).
- **Gate:** `cargo test -p agentum-server --lib` + `bunx vitest run` + `bun run build --prefix crates/agentum-desktop/ui` all green.

---

## 9. Open questions — resolved (decisive defaults; carry-forwards flagged)

1. **Poller placement + cadence.** → **Background worker in `agentum-server::tracker_sync.rs`, spawned at boot** (needs registry + `task_sink` + `gh`, all in `agentum-server`; watchdog has no tracker coupling; no git-route trigger exists for externally-opened PRs). **Cadence 45 s** (`AGENTUM_TRACKER_POLL_SECS`), backoff on failure. *Non-blocking carry-forward: confirm 45 s with Mateo — default ships.*
2. **InReview vs the "review" board column.** → **InReview gets its own `status_mapping` entry via 010's fuzzy discovery + nearest-earlier fallback (→InProgress).** Fold onto a shared review-ish column is safe/non-fatal (independent resolution, no displacement of ReadyToTest). **No board reconfig required to ship.** *Carry-forward to Mateo/reviewer: if he wants a distinct "In Review" column, he adds the Project option — fuzzy match auto-adopts it, zero code change.* (§4)
3. **Bind granularity.** → **Any agent session start in a bound worktree fires InProgress** (including a plain terminal-tab agent), guarded monotonic + idempotent. This is the spec's whole point ("any bound session"); the guard prevents thrash/regression. *Confirm with Mateo — default ships.*
4. **Persist the Project item id at pick time?** → **No.** Persist `tracker_provider` + `tracker_url` only; rely on 010's idempotent `addProjectV2ItemById`-by-content (avoids staleness, no registry↔Projects coupling). *Developer confirm on develop: if 010's write skips its resolve when handed a pre-resolved id, an optional `project_item_id` (serde-default, no alias) is a permitted optimization.* (§6)
5. **Draft PRs.** → **First non-draft PR → InReview** (`isDraft == true` does not trigger). *Confirm — default ships.*

---

## 10. Tradeoffs / rejected alternatives

- **Bus reactor vs routing every plain workspace through `start-work`/harness.** Rejected routing-through-harness: it would force a gated run onto every plain create (violates AC5/AC6 "any bound session, no gated run") and couple plain creates to harness machinery. The bus reactor is decoupled, best-effort, and honours the one-launch-path invariant.
- **Webhooks vs poll.** Rejected webhooks: self-hosted ⇒ no inbound (`board_sync.rs:14`). A bounded, backed-off `gh` poll is the only sanctioned model.
- **Three registry fields vs a sibling `tracker_bindings.json`.** Chose registry fields: the `linked_*` coords already live on the registry `Worktree`, so co-locating tracker coords keeps one source of truth per worktree and avoids a join + a drift-prone second file. Sibling store rejected. (Constrained by inv. #7.)
- **Persist Project item id vs re-resolve.** Chose re-resolve (§6) — no staleness, reuses 010.
- **Inline-in-spawn InProgress vs bus reactor.** Chose bus reactor — never let gh throw into the launch path (inv. #2).
- **Monotonic-forward guard vs pure idempotency alone.** Chose monotonic: pure idempotency would still let a session reopened after Done drag the card back to InProgress. Monotonic gives backward-protection *and* no-thrash *and* harness/session convergence *and* poller terminal-stop from one pure `next_phase_write`.
- **New `TrackerPhase::InReview` vs reusing ReadyToTest for PR-open.** Chose a distinct phase: PR-open and unit-gate-green are semantically different lifecycle events; folding them would make a plain session's "PR open" indistinguishable from a gated run's "tests green" and break the monotonic ordering. Distinct phase + optional board-column fold gives the right model with no forced board config.

---

## 11. Reviewer focus (carry forward — verify specifically)

1. **010 reuse, not rebuild.** Re-grounded on `origin/develop`: confirm the Projects write *inside* `apply_tracker_transition`, the `status_mapping` + option-ID discovery + nearest-earlier fallback, and `done_closes_issue` all exist and are **called/extended**, not re-implemented. Confirm the exact `apply_tracker_transition` signature and the `tracker_id` argument the arms need.
2. **Registry serde safety (inv. #7).** New fields are `#[serde(default)] Option<String>` with **no `#[serde(alias)]`**; an old-shape registry round-trips to `None` and does **not** wipe to `[]` (there is a named test for this — verify it actually deserializes an old fixture).
3. **One launch path (inv. #2).** The InProgress trigger is a bus subscriber; `spawn_agent_into_pane` gains at most a pure broadcast and **no** gh/fallible/tracker code, and nothing can throw into launch.
4. **Best-effort / never-halt (inv. #3, #6).** Every transition and every poller `gh` call logs on failure and never panics/halts; poller is bounded + backed-off; the `gh`-exits-nonzero test proves the loop survives.
5. **InReview mapping / label set.** InReview resolves independently and does **not** displace ReadyToTest's option (fold is acceptable); the github arm now maintains **five** mutually-exclusive `status/*` labels and removes the other four. Confirm the Linear "In Review" fail-closed skip.
6. **Monotonic no-thrash + terminal (inv. #4).** `next_phase_write` blocks Done→InProgress on session-reopen, converges harness-InProgress with session-InProgress, and — with the persisted `tracker_phase` — makes the poller's Done terminal-stop **restart-safe** (a merged PR is not re-transitioned after a server reboot, avoiding a done_closes_issue reopen/close flap).

**Carry-forwards genuinely needing Mateo (non-blocking — working defaults ship):** poll cadence (45 s); whether he wants a *distinct* "In Review" Project column vs a fold onto his existing review column; confirmation that "any agent session" (not just a coding agent) is the intended InProgress trigger.
