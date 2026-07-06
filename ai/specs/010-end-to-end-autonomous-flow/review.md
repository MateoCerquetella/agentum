# Review — spec 010 (reviewer)

> Final sign-off review by the sdd-reviewer (autonomous /sdd-loop), written
> verbatim by the orchestrator. HEAD `8aa8a2d2`. Verdict: **SIGN-OFF —
> SHIP-READY** (release + AC-11 demo stay HUMAN-GATED).

Repo root: `.claude/worktrees/prd-agentum-end-to-end-autonomous`. All paths
relative; all quotes read from the working tree at HEAD (clean).

## Focus items (1–20)

**1. The three accepted duplication-drift risks — comments sufficient?**
Verdict: sufficient for now; consolidate in ONE follow-up ticket after the
boundary freeze lifts (see Should-fix).
- `gh_bin()`: `task_sink.rs:588` (private) vs `github_projects.rs:416`
  (pub(crate)), the copy carrying "same knob as task_sink::gh_bin (kept
  local: F1 adds nothing to task_sink)" (:415–417). Three lines, one env var;
  the new copy is now load-bearing for BOTH new route files
  (`routes/github_projects.rs:249`, `routes/provision.rs:249/:308`), so
  post-freeze the right end-state is task_sink delegating to the shared one.
  Cross-link is one-way (task_sink couldn't be edited) — acceptable.
- `BLOCKED_LABEL`: `provision.rs:24–28` + "Keep in sync with task_sink.rs"
  vs `task_sink.rs:279` (identical value). Drift consequence is cosmetic AND
  self-healing: task_sink's `--force` ensure re-canonicalizes the color on
  the next transition, and task_sink's side is test-pinned
  (task_sink.rs:2174–2177). Comment suffices.
- `resolve_slug`: `routes/provision.rs:39–61` copies
  `routes/github_projects.rs:45–67` with "keep the two in sync" (:38). Both
  are thin wrappers over the same `board_goals::resolve_github_slug` +
  identical 422 envelope, so real divergence surface is small — but this one
  mildly rubs against the repo convention (CLAUDE.md: shared route helpers
  live in `routes/util.rs`). Strongest candidate for the follow-up; not
  blocking, since `routes/util.rs` was outside F3's may-touch list.

**2. D2 residual honesty (two-embedded-servers race).** Verdict: honest.
`WRITE_LOCK` is `static std::sync::Mutex<()>` (`github_projects.rs:138`) —
process-local by construction, so two embedded servers doing the RMW
(`upsert_binding_at` :190–199) can lose one write. Architecture §6.5 states
exactly this. The out-of-profile claim holds on this repo's evidence: every
write path is server-side (PUT/DELETE routes :370/:384, provision :514–517,
knob-toggle rides the same PUT), the TUI gained no bind surface in this
spec, and a lost write is re-bindable. Nothing more to do. Tiny nit: the
code comment at :135–137 ("single writer by construction") is true
per-process but doesn't point at §6.5's residual — leave-as-is.

**3. The `Skipped`-with-label-applied bend — self-describing?** Verdict:
yes. Docstring widened exactly as designed (`task_sink.rs:247–251`). The
fold strings name both halves plus the remedy:
`"status label applied; Projects board write failed: {reason}"` (:681–683)
and `"{why}; Projects board write failed: {reason}"` (:684–686); the AC-7
test pins the scope remedy riding into the run log (:2095–2098). A run-log
reader sees what landed, what didn't, and the fix. One wording nit
(leave-as-is): a close-act failure reads "…Projects board write failed:
issue close failed: …" even though the card move succeeded — the inner
clause names the true failure, so it stays legible.

**4. D1.** Honored. Close/reopen gated on the binding knob only
(`github_projects.rs:896–900`), probe-then-act both directions (:910–947),
knob-OFF never probes (test :1816–1847). The default materializes at ONE
server site: `default_true` (:27–29) used by the serde attribute (:110) and
BOTH wire-absent cases (`routes/github_projects.rs:355–357`,
`routes/provision.rs:314–316`). Unbound byte-identity:
`github_transition_with_board` returns the label result untouched on `None`
(`task_sink.rs:675`), pinned by the exact-5-invocation + no-GraphQL test
(:2018–2056); the tester byte-compared the label fn bodies against base.
PR-`Closes #N` untouched (arm comment :831–836 records the supersession for
bound repos only). The editor's fresh-bind toggle carries its own `true`
literal (`ProjectBindingEditor.tsx:74`, `:98 ?? true`) — inherent (a UI
can't read a serde default) and it always writes the knob explicitly on PUT,
so the server default governs only absent-field reads. Consistent with §7.7.

**5. D2 single-writer / no `github.json` writes.** Honored. The only writers
of the bindings file are `upsert_binding{,_at}`/`remove_binding{,_at}` under
`WRITE_LOCK`; new code touches `github.json` read-only
(`GithubStateMap::from_env()`). Desktop `github_labels.rs` diff-empty
(tester-proven).

**6. D3 zero echo/polling.** Honored. Grep for `setInterval|poll` across all
new UI files: no matches; the Rust side writes only inside `board_write_with`
(transition-driven) — no timers, no board reads except discovery at bind
time.

**7. D4.** Honored. `DEFAULT_TEMPLATE_REPO` is a UI constant
(`workspace-provision-step.ts:18`), the wizard field is editable state
seeded from it (`NewWorkspaceGoalStep.tsx:70`), owner is an explicit input
(:67); the server takes whatever the wire says (`validate_template`, no
hardcoding).

**8. D5.** Honored. Board create ships (`gh_project_create_argv`
`provision.rs:71–75`, `ProjectChoice::Create` path :418–454, wizard create
form `NewWorkspaceProvisionStep.tsx:159–203`). NO Status-field option
mutation anywhere: the only GraphQL mutations in the new code are
`ADD_ITEM_MUTATION` and `UPDATE_STATUS_MUTATION` (`github_projects.rs:671–675`).
FellBack visible: `fallbackHints` (`github-projects-binding.ts:122–126`),
rendered per-phase (`ProjectBindingEditor.tsx:376–380`), plus the
create-mode copy warning (`NewWorkspaceProvisionStep.tsx:197–201`).

**9. D6.** Honored. `grep provision` in `github_projects.rs` = 0 matches; in
`task_sink.rs` = 2 comment-only hits. F2 has no code dependency on F3;
dependency direction is provision → {github_projects, task_sink} only.

**10. D7.** Honored. ONE component (`ProjectBindingEditor.tsx`), two mounts:
Settings (`IntegrationsPane.tsx:266`, rendered :590) and the wizard
(`NewWorkspaceProvisionStep.tsx:158`). Refusal → manual completion:
`selectionFromResolved(null)` yields all-empty
(`github-projects-binding.ts:68–79`), the editor sets the "pick each column
below to finish binding" prompt (`ProjectBindingEditor.tsx:160–166`), Save
gated on `mappingComplete` — never a dead end.

**11. D8.** Honored. Consent default ON and declinable
(`NewWorkspaceProvisionStep.tsx:46`, toggle :220–235); names the branch
(:216–218; authoritative post-run via `CommitReport.branch` from `rev-parse`
`provision.rs:635–638`); exact five-path list rendered when ON (:236–244).
Push is `["push", "origin", "HEAD"]` — plain, never `--force`
(`provision.rs:678`); red push → `pushed:false` + surfaced error, commit
kept (:685–691; test :1022–1046). Commit message is the single
`-m "chore: provision agentum harness scaffold"` (:669) — no AI-attribution
trailer.

**12. No shell injection.** Verified. Every invocation is argv-exec:
`tokio::process::Command::new(program).args(…)` (`github_projects.rs:522–525`
GraphQL runner, :743–746 capture; `task_sink.rs:597–600`;
`provision.rs:121–124` `run_in` for gh AND git). User-controlled strings
ride single argv tokens (`gh_graphql_argv` builds `format!("{key}={value}")`
as one token with hardcoded keys, :424–444); the only string interpolated
into a query is `owner_node`, a closed two-literal set (:561–567); the login
is always a `$owner` var (pinned :1265–1274). The UI never constructs
commands.

**13. Path safety.** Verified. `validate_repo_name`
(`routes/provision.rs:67–82`) restricts to `[A-Za-z0-9._-]`, rejects
`.`/`..` — separators/traversal unrepresentable, test-swept (:371–373). Both
provision routes guard `expand_workdir` + `is_dir` (:241–247, :291–297). The
bindings file path is daemon-owned data-dir + a BTreeMap key; the env
override is process config, not wire-reachable.

**14. No token/secret leakage.** Verified. `SCOPE_MISSING_MESSAGE` is a
constructed constant carrying only the remedy (`github_projects.rs:411–412`);
other kinds carry gh's own message / first stderr line — gh does not print
auth material, and no new code reads token env vars. Truncation bounds:
`run_gh_capture` ~240 chars (:759–766), `run_in` 400 (:140–147). The wire
envelope exposes exactly `{error:{code,message}}`
(`routes/github_projects.rs:73–83`).

**15. No new `is_public`; no new task_sink pub API.** Verified.
`auth.rs::is_public` still lists only the six pre-existing public paths
(:74–85); both new routers merge under the authed top-level router
(`lib.rs:333–335`). task_sink's only visibility changes are the two
documented `pub(crate)` widenings (:311, :459); both seam fns are private
(:666, :734).

**16. Option IDs, never names, at write time.** Verified. The write's option
value is `binding.status_mapping.option_id(phase)` (`github_projects.rs:855`),
riding `-f option=<id>` into `updateProjectV2ItemFieldValue` (test
:1450–1468). Names appear only in bind-time fuzzy matching and display
metadata (`StatusNames`, doc :84). The UI selects submit `value={o.id}`
(`ProjectBindingEditor.tsx:371`).

**17. Best-effort contract + cache soundness.** Verified. The github arms
return `Ok(<seam fn>.await)` where the seam fns return `TransitionResult`,
never `Result` (`task_sink.rs:856–864`, :914–925) — an `Err` cannot escape
either arm. `board_write_with` returns `Result<(), String>` folded into
`Skipped` (:676–689). Cache: populated on success only (:819–823), cold-path
failure returns without retry, cached-path failure invalidates + retries
ONCE cold (:866–895) — correctness never depends on it (stale-heal test
:1668–1721).

**18. Could-not-verify list handled honestly?** Yes, per house precedent.
The F3 test-first narrative is session-internal — the tests' present
strength (run-twice with real git, real `check-ignore`) stands
independently. AC 3 visual + AC 11 ride the human demo with a named runner
(Mateo) and a concrete evidence contract — the 008 AC-12 shape. Full-vitest
not re-run: the 37/37 targeted suites + the exact-1642 tsc delta are the
established regression stand-in. All three disclosed, not buried.

**19. tasks.md count-swap.** Leave as-is. Docs-only inaccuracy in a
committed historical artifact, totals and substance correct; the correction
is durably recorded in `verification.md` and the 04 handoff. Rewriting now
is churn without behavioral value.

**20. Overall.** SHIP-READY pending the human-gated release + AC-11 demo.
Six gates independently reproduced at HEAD, ACs 1–10 pass on read evidence,
sacred surfaces byte-proven, all eight decisions honored in code, and every
cross-cutting safety item verified against the source. Nothing found blocks.

## Blockers

None.

## Should-fix (non-blocking)

1. **One consolidation follow-up ticket** (post-release, when the spec-010
   boundary freeze lifts): (a) move `resolve_slug` into `routes/util.rs` and
   import it from both new route files — the repo's own documented
   convention for shared route helpers; (b) make `task_sink::gh_bin`
   delegate to `github_projects::gh_bin` (or vice versa) so the knob has one
   owner; (c) either widen `task_sink::GITHUB_BLOCKED_LABEL` to `pub(crate)`
   for provision to import, or add a one-line pinning test in `provision.rs`
   asserting `BLOCKED_LABEL == ("status/blocked", "b60205")` so a drift reds
   a test instead of relying on the comment. Suggested framing: `type/chore`
   + `area/server`, "spec 010 follow-up: consolidate the three keep-in-sync
   duplications (resolve_slug, gh_bin, BLOCKED_LABEL)".

## Leave-as-is nits

1. `workspace-goal-step.ts:110` — the `skippable` field doc still says
   "Every one of the three is skippable" while the list (correctly retitled
   at :97) now has four entries. Stale word only.
2. `routes/provision.rs:85–94` — `validate_owner` accepts a leading `-`. No
   injection possible (argv-exec; gh errors verbatim), and GitHub logins
   can't start with `-`; optional polish for the consolidation ticket.
3. `github_projects.rs:135–137` — the `WRITE_LOCK` comment could
   cross-reference architecture §6.5's two-process residual.
4. Close-act failures fold under "Projects board write failed:" even when
   the card move landed — the inner clause keeps it legible; cosmetic.
5. tasks.md F3 vitest per-file counts swapped — already recorded in
   verification.md; no edit needed.

## Verdict

Zero defects found on an independent code-level pass. The tester's three
focus items rule clean; all eight decisions and all cross-cutting invariants
verified with quoted evidence. AC 11 remains the human-gated live demo
(runner Mateo; evidence = issue timeline + `ai/STATE.md` line), and release
stays human-gated regardless.

**SIGN-OFF — SHIP-READY**
