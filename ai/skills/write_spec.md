# Skill: write_spec

How to author a small, focused, incremental spec for agentum. Specs live at
`ai/specs/<NNN>-<name>/spec.md` — copy `ai/specs/_template/spec.md` to start.

## Principles

- **One slice.** A spec is the smallest shippable increment that delivers user
  value. If the goal needs an "and", split it.
- **Imitate the repo.** Ground every spec in real code: name the crates, routes,
  components, and existing helpers you'll reuse. Read before you write.
- **Reuse before build.** Default to existing primitives — the one launch path,
  the worktree API, the event bus, the MCP layer. Justify anything new.
- **Testable acceptance.** Every criterion uses an observable verb (returns,
  renders, persists, emits, blocks) — never "works" or "supports".

## Checklist (every spec has)

1. **Metadata** — number, name, status, surface (crate/dir), author, date.
2. **Problem** — the user-felt problem in 1–3 sentences. No solution yet.
3. **Goal** — one sentence, one slice.
4. **Users / personas** — who feels this, in what moment.
5. **Acceptance criteria** — numbered, testable, observable.
6. **Scope & non-goals (YAGNI)** — what this explicitly does NOT do.
7. **Reuse vs build** — grounded in code research: what exists (don't rebuild)
   vs what's genuinely new.
8. **Risks & invariants** — what could go wrong; the principles you must not
   break (see `ai/context/architecture_principles.md`).
9. **Harness wiring** — how this becomes `.harness/feature_list.json` entries and
   what `verify.sh` / `qa.sh` assert (the green gate).
10. **Open questions** — anything that needs a human decision before build.

## Done = handoff-ready

Run the PM gate (`ai/skills/validate_handoff.md`) before handing to the architect,
then update `ai/STATE.md`.
