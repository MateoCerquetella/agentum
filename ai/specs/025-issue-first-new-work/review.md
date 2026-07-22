# Review — Spec 025: Issue-first New Work

- **Role:** Reviewer
- **Date:** 2026-07-22
- **Verdict:** **SIGN-OFF**
- **Disposition:** 0 blockers, 0 should-fixes

## Final finding

Spec 025 is correct, scoped, compatible, and supported by green executable
gates. The prior keyboard-submit blocker is closed: one pure
`canLaunchNewWork` predicate now controls both the button disabled state and the
first guard in `handlePrimary`, before issue resolution or any irreversible
side effect. The empty-agent copy now matches the fail-closed contract.

The installed-app GitHub/fault-injection/layout scenarios remain explicitly
**UNRUN** and are a mandatory staging/release gate. They are not represented as
passing and do not block code sign-off because the spec and Tester handoff
declare that environment boundary precisely.

## Prior send-back closure

### B1 — keyboard bypass of launch eligibility: CLOSED

- `canLaunchNewWork` rejects a missing selected agent, `agent-unavailable`, and
  `setup-blocked` before considering source or execution mode.
- It permits Manual only after those hard blockers, while preserving the
  intended explicit remote/non-GitHub/non-git Manual compatibility path.
- A checkpointed issue bypasses only New/Existing draft-selection prerequisites,
  so Retry can continue without weakening agent/setup requirements.
- `primaryDisabled` and `handlePrimary` consume the same `launchAllowed` value.
  Enter and click therefore have identical eligibility semantics.
- The same-frame `launchInFlightRef` remains set synchronously before the first
  await and released in `finally`; the fix does not reopen the double-submit
  race.
- The focused suite adds a gate regression covering unavailable-agent,
  setup-blocked, and compatible remote Manual outcomes.

### S1 — contradictory empty-agent copy: CLOSED

The wizard now directs the operator to install or detect an agent before
starting work; it no longer promises a post-open picker that the launch contract
forbids.

## Acceptance-criteria disposition

| AC | Disposition | Reviewer evidence |
|---|---|---|
| 1 | PASS | New/Existing is structural and mutually exclusive. New stages title, description, AI draft, and labels; its deferred variant has no early file button and title Enter cannot launch. Existing retains the project-scoped picker. |
| 2 | PASS | CTA labels are contextual. `resolveLaunchIssue` files only New, checkpoints the confirmed identity, reuses it on Retry, and never creates for Existing. The explicit summary drives worktree binding/name derivation. |
| 3 | PASS | Autopilot/Manual is explicit and mutually exclusive. Eligible work defaults to Autopilot, whose visible copy names PM → Architect → Build → Verify → Review without primary-workflow Harness jargon. |
| 4 | PASS | Both eligible local-GitHub modes prepare the issue-derived spec after worktree creation. Manual uses `plan:false, converge:true`; Autopilot delegates convergence to `start-work`. The old scaffold choice is absent from this wizard. |
| 5 | PASS | Explicit Autopilot directly calls `startGatedWork`, requires confirmed ownership, and opens with `gatedRun:true`; its errors cannot fall through to the legacy plain-session behavior. |
| 6 | PASS | Manual converges the spec without planning or starting the driver, then uses the existing single plain-agent activation. Existing spec bytes are retained and the legacy route default remains unchanged. |
| 7 | PASS | Issue and full worktree results checkpoint immediately at durability boundaries. Retry skips completed irreversible operations, inputs lock by boundary, progress is ordered, and the synchronous in-flight guard closes click/Enter duplication. |
| 8 | PASS | Eligibility reports precise reasons. The shared launch predicate blocks mouse and keyboard before side effects for unavailable agent/setup, requires explicit Manual for compatible ineligible paths, and Autopilot never silently degrades. |

## Invariant, compatibility, and safety review

- **One launch owner:** PASS. Autopilot suppresses plain activation only after
  Harness ownership; Manual never starts Harness.
- **Green/fail-closed gate:** PASS. No verification semantics changed, and
  explicit Autopilot failure remains visible rather than falling back.
- **Irreversible durability:** PASS. Confirmed issue and worktree results are
  retained and reused; no destructive compensation was introduced.
- **Human-edited specs:** PASS. `converge` retains existing content and is
  opt-in; absent/false preserves the established 400-on-existing behavior.
- **Tracker/registry compatibility:** PASS. Existing canonical bind coordinates
  are reused and no registry wire aliases/schema widening were introduced.
- **Legacy composer compatibility:** PASS. New orchestration is selected only
  by optional submit options; unoptioned scaffold/gated-run behavior remains.
- **Security/privacy:** PASS. No credential flow, authorization boundary,
  command-construction path, destructive action, or sensitive-data handling was
  added.
- **Scope:** PASS. Chat retirement, repo-native Loop redesign, SSH/Linear
  Autopilot, and durable cross-reopen recovery remain out of this slice.

## Gate evidence

- Focused desktop UI: **106/106 PASS** across 6 files.
- Vite production build: **PASS**, 7,239 modules transformed.
- Focused Harness server: **10/10 PASS**.
- Rust formatting: **PASS**.
- Diff hygiene: **PASS**.

The absence of a dedicated fake-dependency hook test for full coordinator call
ordering remains a coverage limitation, not a correctness defect: issue call
counts are unit-tested, checkpoint/worktree reuse and ownership ordering are
directly inspectable at the authoritative seam, constituent ownership/opening
tests are green, and installed-app forced failures remain release-gated.

## Verdict

**SIGN-OFF.** All eight acceptance criteria are supported, no blocker or
should-fix remains, architecture invariants hold, and required executable gates
are green. The orchestrator may mark Spec 025 Done; merge and release remain
human-authorized actions, with the declared installed-app QA required first.
