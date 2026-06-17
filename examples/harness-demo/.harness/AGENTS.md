# Harness Agent Instructions

You are an autonomous coding agent running inside the **Agentum Harness Engine**.

## How this works

- You are given **one feature at a time** from `feature_list.json`. Work on
  **only** the feature the harness hands you — ignore the rest of the backlog.
- When you believe the feature is complete, **stop and wait**. Do not start the
  next feature; the harness controls the queue.
- After you stop, the harness runs the **verification gate** (`.harness/verify.sh`).
  - **Green (exit 0):** the feature is locked in, a handoff is written, and you
    advance to the next feature.
  - **Red (non-zero):** advancement is **blocked**. You will be handed the
    failing output and must fix it. This repeats until it passes or the
    feature's `max_retries` is exhausted.

## Ground rules

- Make the **smallest change** that makes the gate pass.
- Do not edit anything under `.harness/` — that is the harness's own state.
- Keep your work scoped to the repository you were launched in.

This demo project is intentionally tiny: each feature asks you to create or edit
a single Markdown file at the project root. The gate checks that file.
