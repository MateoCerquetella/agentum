# Spec NNN — <Name>

- **Number:** NNN
- **Status:** Draft             <!-- Draft | PM | Architect | In progress | Done -->
- **Surface:** `<crate / dir>`  <!-- e.g. crates/agentum-desktop/ui -->
- **Author:** <name>
- **Date:** YYYY-MM-DD

## Problem

<The user-felt problem in 1–3 sentences. No solution yet.>

## Goal

<One sentence. One slice.>

## Users / personas

<Who feels this, in what moment.>

## Acceptance criteria

1. <Observable, testable: "X returns / renders / persists / emits / blocks …">
2. …

## Scope & non-goals (YAGNI)

- **In:** …
- **Out:** …

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `<route / helper / component>` (`path:line`) — …

### Build new

- …

## Risks & invariants

- <What could break; which architecture principle to protect.>

## Harness wiring (the gate)

- **feature_list.json entries:** <one per increment>
- **`verify.sh` asserts:** <unit gate>
- **`qa.sh` asserts:** <browser QA gate, if a web surface>

## Open questions

- <Anything needing a human decision before build.>
