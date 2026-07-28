---
schema: 1
id: SPC-0DDJ7677HRYV0YC1Q1REEDDSZK
revision: 1
title: <Name>
source: legacy-import:ai/specs/_template/spec.md@sha256:fdad1a7797307ad4426304bf00eabb5c0302a3d32c79277d0502b3a653917b88
---

# <Name>

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

> # Spec NNN — <Name>
>
> - **Number:** NNN
> - **Status:** Draft             <!-- Draft | PM | Architect | In progress | Done -->
> - **Surface:** `<crate / dir>`  <!-- e.g. crates/agentum-desktop/ui -->
> - **Author:** <name>
> - **Date:** YYYY-MM-DD
>
> ## Problem
>
> <The user-felt problem in 1–3 sentences. No solution yet.>
>
> ## Goal
>
> <One sentence. One slice.>
>
> ## Users / personas
>
> <Who feels this, in what moment.>
>
> ## Acceptance criteria
>
> 1. <Observable, testable: "X returns / renders / persists / emits / blocks …">
> 2. …
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** …
> - **Out:** …
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - `<route / helper / component>` (`path:line`) — …
>
> ### Build new
>
> - …
>
> ## Risks & invariants
>
> - <What could break; which architecture principle to protect.>
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries:** <one per increment>
> - **`verify.sh` asserts:** <unit gate>
> - **`qa.sh` asserts:** <browser QA gate, if a web surface>
>
> ## Open questions
>
> - <Anything needing a human decision before build.>
