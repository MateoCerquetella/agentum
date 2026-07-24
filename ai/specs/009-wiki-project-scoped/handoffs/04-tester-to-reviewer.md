# Handoff 04 — Tester → Reviewer (spec 009-wiki-project-scoped)

- **Date:** 2026-07-06
- **From:** sdd-tester (fresh subagent, autonomous run)
- **To:** sdd-reviewer
- **Verdict:** **PASS-WITH-DEFERRALS** — full report in `verification.md`
- **HEAD at verification:** `2c3dc89d` (F1 `b325c176`, F2 `8f1b663c`, F3 `fdfec986`)

## What the reviewer receives

- All gates independently re-run and green: cargo 571/0/5 (AC-9 loud-failure
  tests proven unmodified via diff-hunk analysis), fmt clean, clippy zero
  warnings, vite green, vitest 610 passing with the 31-fail baseline
  corroborated against a pristine origin/develop extract (exact same 7 files).
- All 9 ACs PASS (4 with browser-visible aspects deferred to qa.sh/staging
  with concrete repro steps — spec-008 precedent).
- **AC-4 ruled PASS-with-note** (verification.md §C): worst case TWO same-repo
  GETs on mount (probe + events-bus onOpen refetch). Intent (one-repo-only,
  no sweep, no git subprocess — the cache absorbs the second read) fully
  holds; a dedupe was rejected because it would risk the reconnect-heal
  refetch that justifies having no fallback poll. The orchestrator has
  APPLIED the required qa.sh wording amendment to spec.md; the PR body must
  carry the deviation note.
- All four developer deviations audited ACCURATE; D4 not violated (the event
  payload stays snake_case; the camelCase fix is the GET response, whose TS
  type always declared camelCase — latent drift fixed, wire-shape pinned).
- Sacred surfaces confirmed untouched (one launch path, YOLO, hub embed,
  projectHubTab union, is_public/route list byte-identical, .status.json).

## Reviewer focus (suggested)

1. The **discriminator honesty** chain end-to-end: reducer test ("ready event
   ⇒ refetch, reference-equal state") + `applyIndex` as the single transition
   owner + Rust Running-arm — is there ANY path that constructs Ready from an
   event?
2. The **cache correctness** knife-edge: `should_cache_wiki_key` positive-only
   + composite key — can any sequence serve another repo's wiki? (Tester found
   none; adversarial review welcome.)
3. **Scanner lifetime**: `scan_pages_loop` inside `tokio::select!` with
   `wait_for_settle` — confirm no leak path if inject fails before the select.
4. The **AC-4 ruling** itself — reviewer may overrule to send-back if they
   judge the two-GET mount a real defect (tester's reasoning in §C).
5. Cosmetic, D2-locked: double-"Projects" heading potential when
   `groupBy === 'repo'` — leave-as-is vs. a one-line label tweak.

## Deferred (NOT reviewer-blocking; qa.sh/staging + human release gate)

Six live probes with repro steps in verification.md §F: progressive growth,
loud failure, reconnect heal, two-hub scoping, the amended AC-4 network
assertion, and the TCC quiet-open check (the spec's raison d'être).

## Expected artifact

`review.md` — sign-off (SHIP-READY) or a send-back with the failed item,
quoted evidence, and the shallowest fixing role.
