# Review — Spec 014: Per-project persistent browser profiles

- **Spec:** 014-per-project-browser-profiles
- **Date:** 2026-07-09
- **Reviewer:** sdd-reviewer (autonomous /sdd-loop, final phase)
- **Code under review:** commits `8dbe0d88` (F1) · `a69538b5` (F2) · `18ca3376` (F3, HEAD), branch `hagfish`, base `d957eefd`
- **Inputs:** spec.md (D1–D4 locked) · architecture.md (A–H) · tasks.md · verification.md (PASS-WITH-DEFERRALS) · handoffs/04-tester-to-reviewer.md · ai/context/architecture_principles.md
- **Method:** independent code read of every cited surface — no prior role's claim accepted without checking the lines. I cannot execute gates (no shell in this role); gate NUMBERS are tester-attested, test EXISTENCE and content are independently verified.
- **Verdict: SIGN-OFF — spec 014 is SHIP-READY on this base** (release caveats below are mandatory reading for the human release step).

---

## Focus 1 — AC conformance (7/7 at unit level; live halves qa.sh-deferred by the spec itself)

| AC | Verdict | Evidence (independently read) |
|---|---|---|
| 1 | **PASS** | `BrowserScope::profile_token()` (`cdp_browser.rs:262-269`): `Project` ⇒ `format!("project-{}", sanitize_worktree_token(repo_id))` — prefix applied AFTER sanitization, so the 48-char tail bound (`:510`) can never truncate it; a UUID passes sanitization unchanged. `profile_dir_for_token` (`:525-529`) joins `state_dir()/cdp-browser/<token>`. Different repos ⇒ different tokens ⇒ different dirs. Pinned by `project_profile_token_is_prefixed_fs_safe_and_uncollidable` (`:1356-1377`). |
| 2 | **PASS (unit); live login-survival = qa.sh** | See Focus 2 (the teardown split, verified arm by arm). Tests `stop_project_scope_never_deletes_profile_dir` (`:1447`), `release_is_noop_while_project_attached` (`:1492`), `stop_adhoc_scope_deletes_profile_dir` (`:1473`) all read and sound. The spec's own Harness wiring assigns "state survives tab close / relaunch" to qa.sh — deferral is per-spec, not a gap. |
| 3 | **PASS (unit); live routing = qa.sh** | Bare registered UUID ⇒ `Project` (`resolve_scope_from_tables` `:298-303`); the safety property that makes any missed UI surface non-catastrophic is server-side by construction: an unknown path returns `None` ⇒ git probe ⇒ **`Adhoc`, never `Shared`** (`:324`, `resolve_scope_with` `:374-379`). Test `scope_miss_is_adhoc_never_shared` (`:1380`). UI passes `worktreeId` on the WS (`cdp-screencast-client.ts:88-90`). The server-side Adhoc fallback bounds any missed UI site to pre-014 isolation, never a leak. |
| 4 | **PASS** | `project_store_token` (`browser_native.rs:50-59`) ⇒ `project-<repoId>` for `::`-ids and bare UUIDs, raw fallback otherwise; `worktree_data_store_id` (`:67-73`) hashes the TOKEN (same SHA-256→16-byte scheme); wired at `data_store_identifier(data_store_id)` (`:205`). Legacy disjointness pinned by `project_store_id_never_equals_legacy_worktree_id`. |
| 5 | **PASS (unit); end-to-end UI = qa.sh** | See Focus 4. Only-mine test `clear_project_browser_data_deletes_only_that_project` includes a LIVE attach on the cleared project (clear ignores counts — explicit intent). |
| 6 | **PASS** | Reap (`:656-663`) still pkills by the `cdp-browser` root signature — which matches shared/, project-*, and adhoc alike since all now nest under it — clears the registry, deletes **nothing**. Self-test profile is `std::env::temp_dir().join("agentum-cdp-test-…")` (`cdp_driver.rs:1302`), structurally unreachable by a sweep that only reads `state_dir()/cdp-browser` (`cdp_browser.rs:678`). |
| 7 | **PASS (tester-attested execution)** | All mandated new tests exist and were read: 13 Rust server tests, 3 Rust desktop tests, `browser-project.test.ts` (5 vitest cases — the AC-mandated TS derivation test). Gate numbers (574/0/5 server, 78/0/4 desktop, 86/2 vitest with both fails proven pre-existing, vite ✓) are tester-attested; this role cannot rerun them. |

## Focus 2 — The teardown split (riskiest change): VERIFIED

**Three arms of `stop_local_cdp_browser_for` (`cdp_browser.rs:704-735`):**
- *Shared*: `let Some(token) = scope.profile_token() else { return Ok(()); }` (`:706-708`) — no-op; also removes the pre-014 latent oddity (empty key sanitizing to `"wt"` and deleting `cdp-browser/wt`).
- *Project*: attach-gated (`if is_project && project_attach_count(&token) > 0 { return Ok(()); }` `:712-714`), then tmux kill + `pkill_by_signature(profile)` + registry removal — and the delete is guarded: `if !is_project { remove_dir_all }` (`:730-732`). **The `:466`-era delete is gone for the project arm.**
- *Adhoc*: same path with `is_project == false` ⇒ kill + delete, today's contract verbatim.

**Guard genuinely rides inside `run()`'s future:** created in the handler (`routes/cdp_screencast.rs:126-129`), moved at `:131` (`ws.on_upgrade(move |socket| run(socket, base, opts, attach_guard))`), pinned for the stream's whole life at `:144-148` (`let _attach_guard = attach_guard;` with the why-comment). Exactly the placement architecture §5 demanded.

**No std mutex held across `.await`** — audited every lock site in the new code: `registered_listening_port` (`:537`), port reuse in ensure (`:581-585`), stop (`:715-718`, entry extracted as owned value before the tmux `.await`), clear (`:752-755`), `register_browser_attach` (`:452-464` — resolve `.await` happens BEFORE the lock), guard `Drop` (sync), reap (`:660-662`). Desktop `store_tokens` is used only in sync `#[tauri::command] fn`s.

**Remove/prune pass full ids:** `routes/worktrees.rs:456` (`&body.worktree_id`, with the load-bearing comment) and prune `:639-643` (`format!("{repo_id}::{}", wt.path)`).

## Focus 3 — Sweep + shared relocation: VERIFIED (one new Nit, see #4)

- Sweep (`cdp_browser.rs:674-693`): name check **precedes** every deletion (`if name == "shared" || name.starts_with("project-") { continue; }`); top-level `read_dir` only, never recurses. Test seeds the mixed fixture and asserts survivors + idempotency.
- Ordering: `agentum-desktop/src/lib.rs:129-136` — one spawned task, reap `.await` then sweep `.await`, sequential. Verified.
- Shared relocation: `user_data_dir()` = `state_dir()/cdp-browser/shared` (`:1077-1085`); `stop_local_cdp_browser` (`:209-223`) unchanged and now structurally cannot touch `project-*` siblings. Pin test present.

## Focus 4 — The clear action: VERIFIED

- **Only-mine deletion:** `clear_project_browser_data` (`cdp_browser.rs:743-767`) bails on empty id, builds the exact `project-<sanitized id>` token, kills tmux + pkills by that ONE profile path, `remove_dir_all` of that one dir with errors propagating. Test proves the sibling project survives even with a live attach on the cleared one.
- **Route behind auth:** `POST /api/cdp-browser/clear-project-data` (`routes/cdp_browser.rs:36-39`, `:110-121`); `auth.rs::is_public` (`:74-98`) has **no cdp-browser entry** — independently confirmed.
- **Native degradation observable:** `browser_clear_project_data` (`browser_native.rs:372-406`) — flat named params; no live webview ⇒ `{cleared:false, warning:…}`; a `clear_all_browsing_data()` error is returned, never swallowed. UI surfaces warnings verbatim (`AgentBrowserScreencastPane.tsx:460-478`, incl. a catch-arm warning if the invoke itself fails).
- **Stub flip honest:** `browser.rs:37-43` returns `false` with the why-comment.
- **Wire verified end-to-end:** `api.browser.clearProjectData({repoId})` resolves via `defineNamespace`'s Proxy fallback (`tauri/core.ts:55-68`) → `browser_clear_project_data`; command registered in `generate_handler!` (`lib.rs:395`). Server call is NOT fire-and-forget (`clearProjectCdpData`, `cdp-screencast-client.ts:314-334`).

## Focus 5 — Nit dispositions (tester's 3 + 1 new from this review)

1. **Nit 1 (attach guard on explicit-`cdpPort` branch) — CONCUR, keep as Nit.** A tunneled-browser pane with a `worktreeId` inflates the local project count; worst case a local Chromium lingers until the quit/boot reap — inside AC 2's tolerance (lingering allowed, wrong kills not). One-line fix; follow-up ticket.
2. **Nit 2 (native/UI accept any bare UUID; server requires registration) — CONCUR, keep as Nit.** Unreachable via current surfaces; same accepted class as D2's repo-re-add caveat.
3. **Nit 3 (adhoc reserved-name collision incl. the `shared` variant) — CONCUR; FOLLOW-UP TICKET, not Should-fix.** No current caller can produce reserved-name raws (pane ids contain `::`, agent paths start with `/`, pseudo-keys are `github-pr:*`, repo ids are UUIDs); no privilege boundary crossed (equally-authed routes; shared profile is delete-on-stop by contract). The ~5-line hardening (prefix adhoc tokens `adhoc-`, or reject reserved names in `profile_token`) lands in the rebase/port PR where it gets a free re-test.
4. **NEW Nit 4: the boot sweep deletes `cdp-browser/last-screenshot.png` every boot.** `cdp_driver.rs::screenshot_path()` (`:942-948`) writes a top-level file the sweep doesn't spare. Harmless (per-capture scratch artifact, consumed immediately; pre-014 the shared stop's root `remove_dir_all` deleted it far more often — strictly no worse), but it makes architecture Decision E's rationale factually incomplete. Fold into the follow-up ticket: relocate under `cdp-browser/shared/` or spare it by name.

## Focus 6 — PM decisions D1–D4 and architecture A–H: HONORED

D1/D2/D3/D4 all verified against code (resolution chain, canonicalize-both-sides git fallback, tolerant table accessors, clear-only deletion, every-boot-idempotent sweep as a documented safer superset). All 6 developer deviations acceptable (opt-out check placement — identical effect; `cargo check` gate on a 2-line boot change; mildly-unhygienic-but-deterministic real-registry test reads; F2 verify-only; trash-button-not-menu; no unit test for imperative toolbar glue).

## Focus 7 — Repo invariants: HELD

One launch path / YOLO translation / per-session UUID: untouched (no executor/provision/session files in the change set). Push-based, never poll: attach count is WS-lifecycle event-driven; no interval/poll anywhere. API-only server. pkill safety: every profile path still nests under `state_dir()/cdp-browser`. Commit messages (no AI attribution): orchestrator-attested (this role has no shell).

## Focus 8 — D1 behavior-inversion changelog obligation: RECORDED DURABLY

In spec.md Risks, spec.md D1, architecture.md §5, and restated in the release notes below.

## Focus 9 — Security (path traversal): INDEPENDENTLY CONFIRMED NOT POSSIBLE

`sanitize_worktree_token` (`cdp_browser.rs:496-520`) maps every char outside `[A-Za-z0-9_-]` to `-` and bounds to a 48-char tail. Every fs join of caller-influenced input (`:528-529`, `:726`, `:760`) joins a sanitized TOKEN from `profile_token()`, never the raw. The raw is used only for table matching and an arg-passed (not shell-interpolated) `git -C` probe. Residual weakness is namespace collision (Nit 3), not traversal. The background scan's flag is a false positive; the tester's acknowledgment is accurate.

## Focus 10 — Release-readiness scope: SHIP-READY **ON THIS BASE ONLY**

Branch base `d957eefd` (v0.57.0-era); `origin/develop` is at ~v0.67.0 and v0.64.0 touched these exact surfaces (screencast clicks/viewport/paste). This sign-off certifies the spec work on base `d957eefd`; it does NOT certify a clean merge. The human release step MUST rebase/port onto fresh `origin/develop` (precedent: spec 011) and re-run all gates there; expect conflicts concentrated in `AgentBrowserScreencastPane.tsx` and possibly `routes/cdp_screencast.rs` / `cdp-screencast-client.ts`. A property of the loop's starting point, not a defect of the spec work.

---

## Feedback Contract summary

**What worked well:** `BrowserScope` makes the riskiest policy an exhaustively-tested pure function; the RAII attach guard is the right shape (decrement on any exit incl. panic); miss→Adhoc-never-Shared turns unknown-surface bugs into pre-014 isolation instead of cookie leaks; comments encode *why*; the token-keyed registry quietly fixed the latent sanitize-collision double-Chromium bug.

**Risks:** four bounded Nits; mild test-hygiene debt (real-registry reads); the qa.sh live halves of AC 2/3/5 are genuinely unexecuted until the human runs the checklist.

**Should-fix (blocking):** *none.*

**Follow-up ticket (one ticket, post-rebase):**
1. Reserve the token namespace: prefix adhoc tokens (`adhoc-`) or reject `shared`/`project-*` raws in `profile_token()` (Nit 3).
2. Skip attach registration when `cdpPort` is explicit (Nit 1, one line).
3. Native/UI bare-UUID project-check parity, or document the divergence at `project_store_token` (Nit 2).
4. Spare `last-screenshot.png` from the sweep or relocate it under `cdp-browser/shared/` (Nit 4).

## Release notes for the human (mandatory)

1. **Rebase/port first.** Ship-ready ON BASE `d957eefd` only. Rebase/port onto fresh `origin/develop` (~v0.67.0), expect screencast-surface conflicts (v0.64.0), re-run all gates there (server+desktop lib tests, clippy, fmt, vitest, UI build). Precedent: spec 011.
2. **Changelog/PR must state the D1 inversion:** worktrees of one repo now SHARE browser logins (inverts v0.27-era per-worktree isolation); different repos never share. Also note: legacy WKWebView stores orphaned on disk; repo remove+re-add mints a new profile (D2); the legacy profiles UI's delete now shows an honest failure where it faked success (stub flip).
3. **One-time effect at first relaunch:** old shared-profile state at the `cdp-browser/` root is swept (nothing durable lost — it was already deleted on every shared stop).
4. **qa.sh live checklist (deferred halves of AC 2/3/5):** log in on project P → close tab → reopen → survives; relaunch app → survives; project Q absent; Clear on P → P gone, Q intact; plain-workspace browsing lands on the project profile; clear toast/warning surfaces in the installed app.
5. **File the follow-up ticket** (Nits 1–4) when the port PR opens.

## Final verdict

**SIGN-OFF.** All 7 ACs delivered at the unit level with the spec-defined qa.sh deferrals; the riskiest change implemented exactly per architecture (guard in the right future, no lock-across-await); D1–D4 and A–H honored with 6 acceptable documented deviations; repo invariants held; security acknowledgment independently confirmed. Zero Should-fix; four Nits → one follow-up ticket. Spec 014 is SHIP-READY on this base; rebase/port + live QA are owned by the human release step.
