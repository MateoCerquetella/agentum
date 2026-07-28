---
schema: 1
id: SPC-1E1KF5X3AZD063RG9H26A6BP8Z
revision: 1
title: Spec: Unified `.agentum-harness` — merge SDD + Harness Engine (010)
source: legacy-import:ai/specs/010-unified-agentum-harness/spec.md@sha256:6be033ffd18b13d11c09caa529ef8ec9d23ab4e88cc883017ed54e2b9adddbe7
---

# Spec: Unified `.agentum-harness` — merge SDD + Harness Engine (010)

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

> # Spec: Unified `.agentum-harness` — merge SDD + Harness Engine (010)
>
> > **STATUS: SPLIT (PM, 2026-06-17).** Too big for one screen — kept as the **full-vision reference**. Built via 5 dependency-ordered children:
> > - **010a** — unified `.agentum-harness/` surface + zero-footprint adoption + migration *(foundational; build first)*
> > - **010b** — unified state model + per-worktree durable, rebuildable board *(needs 010a)*
> > - **010c** — lifecycle: spec→backlog + feature triple + auto-advance + HITL-at-QA *(needs 010a/b; absorbs the existing Harness Engine)*
> > - **010d** — verification hardening: init-as-a-phase / Bootstrap Contract + E2E verify + externalized completion / false-green + clean-state atomic handoff *(needs 010c)*
> > - **010e** — instructions & observability: router `AGENTS.md` + thematic docs + process-observability (why-it-passed) + decision log *(needs 010c)*
>
> ## Goal
>
> Replace agentum's **two parallel harnesses** — the SDD `ai/` playbooks (authoring: spec → role gates) and the Harness Engine `.harness/` (execution: feature backlog → verify gate → auto state) — with **one agentum-managed surface, `.agentum-harness/`**. A developer opens **any** repo, authors a spec, watches it broken into a verify-gated backlog, and sees it advance **todo → done automatically** — with **no generic machinery copied into the repo**. The only per-repo footprint is the durable `.agentum-harness/` deliverables, which agentum scaffolds and which commit to git.
>
> ---
>
> ## User Value
>
> **In one line:** one harness instead of two — open the repo, get the whole spec→gates→verify→done loop, with nothing generic to install.
>
> Today there are **two overlapping systems** to install and reason about: SDD `ai/` (per-repo, **gitignored** → specs not durable, manual `STATE.md`) and the Harness Engine `.harness/` (per-repo `feature_list.json` + verify gate, committable). They cover the **same arc from opposite ends** and double the install tax + mental model. Cost of leaving it: every repo re-pays both, and the concepts drift. Persona: the **self-hoster** running the workflow across many worktrees in agentum (primary user). This is the **harness work parked in 009**.
>
> ---
>
> ## Requirements
>
> **Surface & footprint**
> - **Single surface:** `.agentum-harness/` **replaces both** `ai/` and `.harness/` — one folder, one mental model.
> - **Generic machinery central:** SDD playbooks/roles/orchestrator **and** the Harness Engine logic + commands live **once in agentum** (read-only / MCP tools — the direction that already "supersedes install-a-skill"). **Never** copied into the repo.
> - **Minimal durable footprint:** only **project-specific** artifacts live in `.agentum-harness/` and are **committable to git** — the spec(s), the feature backlog + state, decision/observability logs, and the project's own `verify.sh` / `AGENTS.md` / `init.sh`. agentum **scaffolds** them; the developer never hand-installs.
> - **`AGENTS.md` is a router, not an encyclopedia (L04):** ≤200 lines, ≤15 **hard** constraints (segregated from soft guidance), project summary + quick-start + on-demand links to thematic docs under `.agentum-harness/docs/`. agentum prepends **only the router** to feature prompts; thematic docs load by topic.
>
> **Lifecycle & verification (the engine)**
> - **Initialization is its own gated phase (L06):** before any feature advances, an **init gate** must pass — `init.sh` green = reproducible env **+ ≥1 passing example test** + a **Bootstrap Contract** in `AGENTS.md` (how to start / test / see progress / resume) + an ordered backlog (≥3 features w/ acceptance criteria) + an **"init complete" commit**. Features never run on an unverified foundation.
> - **spec → backlog pipeline:** an authored spec is **broken into the verify-gated backlog** the engine drives (SDD "what" feeds Harness "execute").
> - **Feature = the triple, harness-owned (L07/L08):** every backlog feature carries **behavior description + executable verify command + current state**, advances **one at a time (WIP=1)**, and on `done` persists **evidence (commit ref + verify output)**. **Only the harness** sets state — an agent never self-promotes; `done` is **irreversible** within a run. Granularity = "completable in one run."
> - **Unified state model:** SDD role gates (PM → Architect → Developer → Tester → Reviewer) and Harness feature states (pending → coding → verifying → done/blocked) **merge into ONE todo→done lifecycle** on one agentum board.
> - **Completion is externalized + end-to-end (L09/L10):** a feature reaches `done` only when `verify.sh` **exits 0 AND** runtime evidence is persisted — the agent's self-report is **never** sufficient, and the single human confirm is a **checkpoint on top of** passed verification, not a substitute. `verify.sh` must exercise **real end-to-end behavior** across component boundaries (not unit/mock-only) and enforce architectural constraints **mechanically**; gate-failure output names what broke, why, and the fix.
> - **Auto-advance, one gate:** features auto-advance the lifecycle; agentum **pauses for ONE human confirmation when the verify gate (QA) passes**, then marks **done**.
> - **Agent can actually run the gate (L02):** scaffolding grants the executing agent **least-privilege access** to run `init.sh` / `verify.sh` / the project's build+run commands — referencing them isn't enough. The environment is **self-describing and reproducible**.
>
> **State, durability & continuity**
> - **Durable + rebuildable:** repo `.agentum-harness/` is the **durable source of truth** (git); agentum's store is a **rebuildable index** (scan `.agentum-harness/` → restore board; no data loss on wipe).
> - **Per-worktree, per-branch state:** `.agentum-harness/` state is carried by the **branch** (each worktree is its own checkout) — an agent sees only its branch's state ("locked inside the repo"). agentum **aggregates** worktrees into one board but **owns none of it**. State is split **per-spec** (`<spec>/state.json` + append-only logs), **never one global mutable file** — so concurrent worktrees don't collide and **git merge is the reconciliation**.
> - **Durable "why," not just "what" (L05/L11):** persist an **append-only decision log** per spec (date, choice, **rejected alternatives**) **plus** per-feature **process observability** (the verify criteria/rubric that defined done, the gate outcome + reason, a transition trace). `handoff.md` overwrite must **not** be the only continuity record. A reviewer can answer *"why was this accepted?"* from committed artifacts without re-running.
> - **Every session ends clean (L12):** each state transition is **atomic** (commit-or-rollback — no half-advanced feature persists across sessions); at session end the **working tree is clean, build + verify pass**, and artifacts reflect actual state. `init.sh` is the **idempotent** standard startup path.
> - **Migration:** existing `ai/specs/` (001–009) + the demo `.harness/` map into `.agentum-harness/` **without hand-rewrite**.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] A repo with **neither** `ai/` nor `.harness/` runs the full loop with **no generic machinery copied in** — only `.agentum-harness/` is created, **scaffolded by agentum**
> - [ ] `.agentum-harness/` holds **exactly** the durable project artifacts — **no** generic playbooks/roles/engine code in the repo — and is **committable** to git (no `/ai/`-style gitignore)
> - [ ] **Init gate (Bootstrap Contract):** before any feature advances, a fresh `.agentum-harness/` lets a session start, **≥1 example test passes**, a backlog (≥3 features w/ acceptance criteria) exists, and an **init checkpoint is committed**
> - [ ] **Cold-start test:** a fresh agent session, given only the committed `.agentum-harness/`, can answer *what the project is, how it's organized, how to run it (`init.sh`), how to verify it (`verify.sh`), and current progress* — with no external context
> - [ ] **`AGENTS.md`** is ≤200 lines with ≤15 enumerated hard constraints; detailed/topic guidance lives in separately-loadable thematic docs, **not** inlined into every feature prompt
> - [ ] An authored spec is **broken into a verify-gated backlog** visible on **one** agentum board, each feature carrying behavior + verify command + state
> - [ ] Features **auto-advance** todo → … → verifying with **no manual state edits**; the board reflects it live; **WIP=1** (one active feature per spec at a time)
> - [ ] **False-green test:** a feature whose agent claims success but whose `verify.sh` **fails (or whose evidence is absent)** stays `verifying`/`blocked` and does **not** advance
> - [ ] **E2E gate:** a **cross-component** change cannot reach `done` unless `verify.sh` ran and passed an **end-to-end** check; a unit-only green must not advance it
> - [ ] When the **verify gate passes (QA)**, agentum **stops for ONE human confirmation** (a checkpoint on top of passed verification); only after confirm → **done**
> - [ ] **Why-it-passed:** for any `done` feature, the board/store can show **which verify criteria, the gate result, and the transition trace** — not just that it's done — plus the spec's **decision log** (incl. rejected alternatives)
> - [ ] **Atomic clean handoff:** state is committed **only** when verify passes; **no** feature is left half-done at session end; reopening a repo/worktree starts from a **clean, committed, rebuildable** state with no manual diagnosis
> - [ ] **Clear** agentum's store, reopen the repo → the board **re-derives** state by scanning `.agentum-harness/` (no progress lost)
> - [ ] **Two worktrees on different branches** each drive their own spec; each branch's `.agentum-harness/` reflects **only its own** state; the board shows **both, keyed by worktree**; merging a branch carries its state **without clobbering** the other
> - [ ] Existing **001–009** specs and the demo **`.harness/`** are readable under the unified model **without hand-rewrite**
>
> ---
>
> ## Dependencies
>
> - **Harness Engine** (`harness.rs`, `/api/harness/*`, `HarnessEngine.tsx`, the `.harness/` contract: `AGENTS.md`/`init.sh`/`verify.sh`/`handoff.md`/`feature_list.json`) — the execution + verify-gate + auto-state half **already exists**; this **absorbs** it.
> - **SDD playbooks** (now at `~/.claude/ai/`) + user-level commands — the authoring/role half.
> - **MCP-tools direction** ("supersedes install-a-skill into `~/.claude/skills`; no per-agent skill files") — the mechanism for central, no-per-repo delivery.
> - **agentum worktree/session model + orchestration** (task DAGs, decision gates) — state keyed per worktree; the auto-advance engine.
> - **Live `HarnessEvent` WS stream + desktop "Harness" board** — the runtime-observability layer this builds the process-observability layer onto.
> - **Gitignore policy** — `.agentum-harness/` must be trackable like `.harness/` is today (and unlike `/ai/`, which is ignored at `.gitignore:36`).
>
> ---
>
> ## Risks
>
> - **Two state models merging** — `STATE.md` (role/phase, manual) vs `feature_list.json` (engine-written). Collapsing a "role gate" and a "feature verify" into one lifecycle is the **core design risk**; mismatched semantics could leak.
> - **Largest spec yet** — flag PM to **split** at "completable in one run" granularity: (a) `.agentum-harness/` surface + migrate `ai/`+`.harness/`, (b) init-as-a-phase + Bootstrap Contract, (c) spec→backlog pipeline + feature triple, (d) unified board + auto-advance + HITL-at-QA + process observability, (e) E2E verify + clean-state/atomic handoff, (f) central-machinery/MCP delivery + durability/rebuild.
> - **`verify.sh` is irreducibly per-project** — "zero per-repo files" can't be absolute; the build/test config must live somewhere. Frame: **generic central, project config in `.agentum-harness/`** (scaffolded, acceptable).
> - **Auto-advance hides failures (L09/L10)** — carry from 008/009 **and** the Harness Engine's known "agent reports green without driving"; mitigated by requiring `verify.sh` to run **real E2E** + **persisted evidence** as a gating precondition. A green unit-only gate must not advance cross-component work.
> - **`AGENTS.md` bloat over time (L04)** — instructions only accumulate; a 600-line file burns context budget and buries constraints. Needs the size cap + periodic audit/expiry discipline.
> - **Init/execution conflation (L06)** — if `init.sh` stays a one-shot smoke test, features advance on an unverified foundation ("tiles on wet concrete"); the init **gate** must produce the full Bootstrap Contract first.
> - **Lost "why" (L05)** — `handoff.md` overwrite discards decision history; per-spec state captures *what* but not *why*, risking reversed design choices across sessions/branches — the decision log mitigates.
> - **Source-of-truth split** — repo `.agentum-harness/` (durable) vs agentum store (index): define which wins on conflict.
> - **Central-machinery coupling** — a playbook/engine change in agentum shifts behavior across **all** repos (no per-repo pinning) → may need versioning.
> - **Migration fidelity** — 001–009 + `STATE.md` history must map cleanly; **Knowledge Visibility Gap** (stale > absent) is the metric.
> - **Multi-worktree collisions** — a single shared mutable state file (the old monolithic `STATE.md`) is a **merge magnet** across branches → split state **per-spec** + append-only; **ACID** writes (atomic commit, per-worktree isolation), **worsened if sessions end dirty** → require the clean-state handoff so git merge reconciles committed states, never half-transitions.
>
> ---
>
> ## Notes
>
> - **Decision (user):** merge SDD + Harness Engine into **one surface named `.agentum-harness/`**; specs/state stored **per-repo + durable in git** (not agentum-only — data-loss risk).
> - The slash commands are **already user-level**; the **MCP-tools direction** is the chosen delivery mechanism, so "no per-repo generic files" **aligns with where agentum is already headed** (skill-file install is being retired).
> - **Naming:** `.agentum-harness/` (dotfolder) signals "agentum-managed project surface," parallel to `.git/` — but unlike `.git`, its contents are **meant to be committed**.
> - **Out of scope this round:** multi-user/team state sync; non-agentum environments (markdown still works by hand there); issue-tracker/GitLab posting.
> - **Motivating incident (real, 2026-06-17, observed while drafting this spec):** a second workstream (009a, developer phase) was editing the **same** `ai/STATE.md` in the **same working dir**; a read→re-read flipped `009`→`009a` underneath the author, and writing 010's pointer would have **stomped live 009a work**. Root cause = ONE shared mutable `STATE.md` + no worktree isolation + `ai/` gitignored (so even worktrees wouldn't *reconcile* it — each would carry an isolated, never-merged copy). This is precisely the failure mode 010 removes: **per-spec state files (010b) + committable `.agentum-harness/` (010a) + worktree-per-branch** → two specs never collide, in real time **or** at merge. Worktrees alone only defer a monolithic-`STATE.md` clash to merge time; per-spec state is the actual fix.
> - **Audited against the full [learn-harness-engineering](https://walkinglabs.github.io/learn-harness-engineering) course (12 lectures).** Folded in: repo as system of record + cold-start/ACID (L03), router-style `AGENTS.md` (L04), decision-rationale continuity (L05), init-as-a-phase / Bootstrap Contract (L06), WIP=1 + feature triple (L07/L08), externalized + end-to-end verification, no premature victory (L09/L10), harness-owned observability incl. process artifacts (L11), clean atomic session handoff (L12). L01 (harness-induced failure) + L02 (five subsystems: instructions/tools/env/state/feedback) are the framing — Tools/Environment now explicit. Verdicts: L01/L07 honored as-was; all others folded in above.
