# Handoff Rules

Every transition between roles produces a **handoff contract**. The contract is small (one screen), explicit, and stored in the spec folder.

Use `ai/contracts/templates/handoff_contract.md` as the structure.

---

## What every handoff must contain

1. **Summary** — one paragraph: what was done, what's next.
2. **Completed work** — bullet list, no hidden assumptions.
3. **Pending work** — what's not done and why (out of scope, blocked, deferred).
4. **Important decisions** — tradeoffs made, with the one-line *why*.
5. **Risks** — what could still go wrong.
6. **Questions** — unresolved ambiguities. Use `question_contract.md` for any that need a human.
7. **Recommended next step** — which role acts next, on what.

---

## Send-back triggers (when a handoff must go *backward*, not forward)

The Orchestrator runs `validate_handoff.md` at every gate. If the gate fails, the handoff goes to the **shallowest** upstream role that can fix the issue.

| Trigger | Send back to |
|---|---|
| Spec ambiguous / vague AC | PM |
| Spec is solution-shaped, not problem-shaped | Analyst |
| Architecture impossible at named scope | PM |
| Design blocks implementation | Architect |
| Design flaw revealed by tests | Architect |
| Implementation doesn't match spec | Developer |
| Acceptance criterion unmet (code-level) | Developer |
| Maintainability concerns | Developer |
| AC unobservable in practice | PM |
| User value didn't land despite code working | PM |
| Requirement turned out wrong | PM (then Analyst if foundational) |

When sending back, always include:
- which gate item failed
- specific evidence (quote / line / file)
- fix direction (not implementation)

---

## Storage

Save handoffs under the spec folder:

```
ai/specs/<spec-id>/handoffs/<NN>-<from>-to-<to>.md
```

Where `NN` is a 2-digit sequence within the spec.

---

## Anti-patterns

- ❌ Vague summary ("did some work, ready for next")
- ❌ Hiding unresolved questions inside Notes
- ❌ Recommending a next step that contradicts the gate outcome
- ❌ Approving a handoff with "...but…" caveats — if there's a but, the gate failed
- ❌ Sending everything back to PM (lazy routing — always pick the shallowest)
