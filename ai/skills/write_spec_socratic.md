# Skill: Write Spec (Socratic)

## When to use this skill

Use this when the spec idea is **vague, novel, or stakeholder-driven** — typically a Business Analyst (or anyone in BA mode) trying to crystallize a fuzzy ask into a concrete, implementable `spec.md`.

If you already have a clear written ask with acceptance criteria, use `write_spec.md` instead — it's faster.

---

## How this skill works

You will run a structured **five-pass Socratic interview** with the human. Each pass has a goal, 2–3 probing questions, and a known anti-pattern to call out. Ask **one pass at a time**. Wait for the answer. Reflect it back in your own words before moving on.

At the end, you crystallize the answers into a `spec.md` draft using `ai/specs/_template/spec.md` and run a self-check.

**Output style during the interview:** short. One question at a time when possible, two max. No bullet walls. The point is to think *with* the human, not interrogate them.

---

## Opening (always do this first)

Ask exactly:

> "In one sentence: what problem are you trying to solve, and for whom?"

If they answer with a solution instead of a problem ("I want a button that…", "we need a dashboard that…"), gently redirect:

> "That sounds like a solution. What's the underlying pain that solution would relieve? Who feels it?"

Do **not** proceed until you have a problem-framed answer with a "who."

---

## Pass 1 — WHO

**Goal:** identify the real user(s) and the boundary between primary and secondary stakeholders.

Probes:
- "Who experiences this problem most acutely today?"
- "Is there anyone else affected — even indirectly?"
- "How often do they hit this? Daily? Once a quarter?"

**Anti-pattern to flag:** "everyone" or "all users." Push back: "Can you name a specific role or persona we've worked with?"

---

## Pass 2 — WHAT

**Goal:** define the smallest observable change in the world that would mean the problem is solved.

Probes:
- "If this were solved tomorrow, what would the user *do differently*?"
- "What's the smallest version of this that would still be useful?"
- "What's explicitly **out of scope** for this round?"

**Anti-pattern to flag:** scope creep ("...and also it should..."). Capture the extras in a separate "future ideas" note and bring focus back.

---

## Pass 3 — WHY

**Goal:** surface the business or user value, and the cost of doing nothing.

Probes:
- "Why now? What changed?"
- "What's the cost of not solving this in this iteration?"
- "Who is asking for it, and what's their measure of success?"

**Anti-pattern to flag:** "the boss asked for it" with no downstream value. Probe one level deeper: "What outcome are they trying to drive?"

---

## Pass 4 — WHEN IS IT DONE (acceptance criteria)

**Goal:** turn "done" into 3–6 testable, observable conditions.

Probes:
- "How will we know the user can do the thing now?"
- "What's the test — manual or automated — that proves it?"
- "What would a stakeholder demo look like?"

**Anti-pattern to flag:** vague verbs (*improve, enhance, support, handle*). Force concrete verbs: *create, save, return, display, reject, log*.

Capture as `- [ ]` checkboxes.

---

## Pass 5 — WHAT COULD GO WRONG

**Goal:** surface risks, dependencies, and hidden assumptions early so they reach the Architect, not the Reviewer.

Probes:
- "What's the most fragile part of this idea?"
- "What does this depend on that we don't control?"
- "What assumption are we making that we haven't tested?"

**Anti-pattern to flag:** "nothing" or "should be straightforward." Press once: "If a developer asked you what's hard about this, what would you say?"

---

## Crystallization

Once all five passes are complete:

1. Open `ai/specs/_template/spec.md` as the structure.
2. Create a new spec folder: `ai/specs/<NNN>-<short-kebab-name>/` (where `NNN` is the next free 3-digit number).
3. Write `spec.md` populated from the interview:
   - **Goal** ← Pass 2 (smallest observable change)
   - **User Value** ← Pass 1 + Pass 3
   - **Requirements** ← Pass 2 (in-scope items)
   - **Acceptance Criteria** ← Pass 4 (as checkboxes)
   - **Dependencies** ← Pass 5 (external + prior specs)
   - **Risks** ← Pass 5
   - **Notes** ← anything that didn't fit, including "out of scope for this round"
4. Show the draft to the human. Ask: *"Did I capture this right? Anything to add, remove, or sharpen?"*
5. Iterate until they say yes.

---

## Self-check before handoff to PM

Before you mark this skill complete, verify:

- [ ] The Goal sentence names a *user action*, not a feature
- [ ] Every acceptance criterion is observable (someone could write a test for it)
- [ ] At least one risk is named, even if low
- [ ] No vague verbs (*improve, enhance, support*) in acceptance criteria
- [ ] The spec fits on one screen — if it doesn't, it's too big; split it
- [ ] "Out of scope" is explicit

If any check fails, do one more focused round of questioning on just that area.

---

## Handoff

Update `ai/STATE.md`:
- `current_spec`: `<NNN>-<name>`
- `current_role`: `pm`
- `current_phase`: `pm`
- log decision: `"Spec drafted via Socratic interview; handoff to PM for criteria refinement."`

Then invoke the PM role (`ai/roles/pm.md`) — or, in HITL mode, pause and ask the human to confirm before handing off.
