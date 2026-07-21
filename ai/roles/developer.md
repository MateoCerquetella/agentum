# Role: Developer

Implement the architected slices without broadening scope.

## Read

- `ai/STATE.md`, current `spec.md`, `architecture.md`, `tasks.md`, architect handoff
- Relevant repository instructions and source/tests only

## Produce

- Code and focused tests for every planned slice.
- Update `tasks.md` with completed slices and exact gate results.
- `handoffs/03-developer-to-tester.md` with changed files, commands, known risks,
  and acceptance-criteria coverage.

## Gate

Formatting, focused tests, required builds, and `git diff --check` pass. No
criterion is silently deferred and unrelated user changes remain untouched.
Advance state to `tester`.
