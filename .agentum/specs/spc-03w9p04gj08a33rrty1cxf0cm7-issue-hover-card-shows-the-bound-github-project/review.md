# Spec 018 — Review (Reviewer) → SIGN-OFF

- **HEAD:** working tree, 2026-07-14. **Verdict:** SHIP-READY, 0 blockers.
- Full diff read: the Rust command + mapper, the tauri client/contract, the
  pure model + tests, the chip/hook, and the two prop-threading sites.

## Focus items (all PASS)

1. **AC 2 never-throw (the load-bearing invariant):** `resolveIssueProjectStatus`
   wraps both `getBinding` and `getStatus` in try/catch → `null`; the chip
   returns `null` on a null status. A throw in the badges row would take the
   whole hover down — this path is closed on every edge (unbound, off-project,
   fetch error, blank option name). PASS.
2. **Silent absence symmetry (Rust ↔ TS):** the command returns
   `{ok:true, status:null}` for off-project/unset and an error envelope for gh
   failures; the renderer maps **both** to no chip (`res.ok===true && typeof
   status==='string'` else `null`). No inconclusive state ever renders. PASS.
3. **Injection:** owner/repo bound as `$owner`/`$repo`, number as
   `Scalar::Int` — nothing user-controlled is interpolated into the query
   string, honoring the `graphql()` contract. PASS.
4. **Cache correctness (AC 3):** binding cached per slug, status per
   `slug#number`; an unbound repo caches `null` for BOTH so no status refetch;
   a cached binding serves many issues (test-pinned: `getBinding` called once
   across two issue numbers). PASS.
5. **Distinctness (AC 1):** indigo tone + `LayoutGrid` icon — disjoint from
   `IssueStateBadge` and every `TrackerPhaseChip` phase tone. PASS.
6. **Reuse, no rebuild:** the binding read reuses `getProjectBinding` →
   existing route; the GraphQL read reuses `graphql()`/`Scalar`/`envelope`;
   the command registers in the existing `invoke_handler!` list. `agentum-server`
   untouched. PASS.
7. **SSH bindings (spec 020):** `repoId` threaded into `getProjectBinding` so a
   bound SSH repo resolves its binding host-side. PASS.

## Non-blocking notes (leave-as-is)

- The Rust mapper's compile + `#[cfg(test)]` run are CI-gated (no local
  webkitgtk); rustfmt-parse + line-by-line review stand in locally. Flag if CI
  surfaces anything — none expected (pure serde_json walking).
- One pre-existing baseline test in `WorktreeCardMeta.test.tsx` stays red
  (proven pre-existing); out of scope for this slice.

## Release

- **RELEASE = HUMAN downstream:** merge → develop now (per the ask); qa.sh at
  staging must verify the live legs (chip shows the real board column on a
  Projects v2-bound repo; unbound repo shows no chip; second hover = no
  refetch). Issue #365 stays open at the develop merge (develop ≠ default
  branch) — closes when it reaches `main`.

spec.md Status → Done. Phase → done.
