# Role: Tester

Independently verify the implemented behavior against the acceptance criteria.

## Read

- `ai/STATE.md`, current spec/design/tasks, developer handoff, changed diff

## Produce

- `verification.md` mapping each acceptance criterion to automated or runtime
  evidence, including negative/race/error cases.
- Run `verify.sh`/focused project gates and `qa.sh` when the surface requires a
  real browser and the environment is available. Never label an unrun QA leg as
  passing; record a human/environment gate explicitly.
- `handoffs/04-tester-to-reviewer.md` on pass.

## Gate

All required executable gates are green, failures are reproducible, and every
criterion has evidence or an explicit blocking reason. Advance state to
`reviewer`; implementation defects return to Developer.
