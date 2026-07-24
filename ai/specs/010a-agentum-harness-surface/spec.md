# Spec: Unified `.agentum-harness/` surface — zero-footprint adoption (010a)

> Child 1 of **010** (SPLIT). Foundational — build first. Downstream: 010b (state/board), 010c (lifecycle), 010d (verification), 010e (instructions/observability).

## Goal

A developer opens **any** repo in agentum and adopts the harness with **zero generic files copied in** — agentum scaffolds a single **`.agentum-harness/`** folder holding only the durable project artifacts, committable to git. This replaces today's **two** installs (SDD `ai/` + Harness Engine `.harness/`) with one.

---

## User Value

**In one line:** adoption becomes *"open the repo,"* not *"copy `ai/` + `.harness/` and hand-edit files."* This removes the per-repo install tax that is the exact reason the harness was **parked in 009**.

---

## Requirements

- **Define the `.agentum-harness/` contract:** the **only** repo footprint; holds durable project artifacts (the spec(s), backlog + state, `AGENTS.md` / `init.sh` / `verify.sh`, logs); **scaffolded by agentum**, not hand-created.
- **Generic machinery stays central:** SDD playbooks/roles/orchestrator + Harness Engine logic + commands live **only in agentum** (read-only / MCP tools) — **never** copied into the repo.
- **Committable:** `.agentum-harness/` is **tracked by git** — replace the `/ai/` ignore (`.gitignore:36`) with a rule that tracks `.agentum-harness/` (ignoring only genuinely transient subpaths, if any).
- **Adoption writes nothing generic:** opening a repo with neither `ai/` nor `.harness/` and starting the workflow creates **only** `.agentum-harness/`.
- **Migration:** existing `ai/specs/` (001–009) and the demo `.harness/` map into `.agentum-harness/` **without hand-rewrite**, via a one-time agentum-run migration.

---

## Acceptance Criteria

- [ ] Opening a fresh repo + starting the workflow creates **only** `.agentum-harness/` — no `.claude/`, no `ai/` playbooks, no engine code in the repo
- [ ] No generic playbook/role/engine file exists under the repo; those resolve from agentum centrally
- [ ] `.agentum-harness/` is **tracked by git** (committable); the `/ai/` ignore no longer blocks deliverables
- [ ] `ai/specs/001–009` are readable/usable under `.agentum-harness/` with **no manual rewrite**
- [ ] The demo `examples/harness-demo/.harness/` maps into the unified surface **without hand-edits**

---

## Dependencies

- **MCP-tools direction** — central delivery; supersedes skill-file install (the mechanism for no-per-repo generic files).
- **Harness Engine + SDD playbooks** — the machinery being centralized.
- **agentum scaffolding + worktree model** — writes `.agentum-harness/` into the opened repo/worktree.
- **`.gitignore:36` (`/ai/`) policy change** — required for committable deliverables.

---

## Risks

- **Gitignore scope** — track `.agentum-harness/` but keep real transient noise out → an ignore-**except** rule.
- **Migration fidelity** — 001–009 + `STATE.md` history must map cleanly; **Knowledge Visibility Gap** (stale > absent) is the metric.
- **"Zero generic files" vs project config** — `verify.sh` / `AGENTS.md` are **project-specific** (legitimately in `.agentum-harness/`), not generic machinery; keep the distinction explicit so the AC isn't misread as "no files at all."

---

## Notes

- **In scope:** only the adoption surface + central-machinery boundary + migration. **Out of scope here:** the auto-advance lifecycle, the board, and verification semantics — those are **010b–010e**.
- Inherits all parent-010 decisions and the full lecture audit (see `010-unified-agentum-harness/spec.md`).
