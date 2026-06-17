# Harness demo project

A tiny, self-contained project for driving the **Agentum Harness Engine**
end-to-end. Open Agentum Desktop → **Harness** in the sidebar, point it at this
directory, and hit **Run**.

## What the harness does here

1. Runs `.harness/init.sh` (environment smoke-test).
2. Picks the first `pending` feature from `.harness/feature_list.json`.
3. Spawns a **real agent** (Claude Code by default) scoped to that one feature.
4. Lets the agent work, then runs `.harness/verify.sh` (the **gate**).
   - **Green** → the feature is marked `done`, `handoff.md` is written, and the
     harness advances to the next feature.
   - **Red** → advancement is **blocked**; the agent is handed the failing
     output and retries until it passes or hits `max_retries`.

## Features in this demo

| id                | what the agent must do                                  | gate check                                          |
| ----------------- | ------------------------------------------------------- | --------------------------------------------------- |
| `hello-file`      | create `GREETING.md` with a specific first line         | file exists + exact greeting line                   |
| `add-build-stamp` | append a line starting with `Verified` to `GREETING.md` | greeting intact + a `Verified…` line present        |

Everything the gate touches is a plain file at the project root, so you can
watch the board go **backlog → coding → verifying → done** for real.

> The harness writes feature state back into `feature_list.json` as it runs. To
> re-run from scratch, reset every feature's `"state"` back to `"pending"` and
> delete `GREETING.md`.
