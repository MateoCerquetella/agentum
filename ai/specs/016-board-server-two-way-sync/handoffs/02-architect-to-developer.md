# Handoff: Architect → Developer — Spec 016a

## 1. Summary
Architecture for **016a** complete; Architect gate **PASSED (6/6)**. The design is a
faithful re-port of `feat/014d`'s **PULL half** onto a **fresh branch off `origin/develop`**,
with the three PR-killing collisions removed by construction. Build **016a only**
(server-side GitHub pull + durable binding + migration) — no push-back, Linear, or desktop.
Full design in `architecture.md`.

## 2. Completed Work
- `architecture.md` written: components/files (NEW vs MODIFIED), API table, data flow,
  the key tradeoff, a risk/mitigation table, explicit non-goals, and a **10-step build order**.
- Decisions locked: reconcile by `(provider, external_id)`; **extend** #58's columns
  (new migration adds ONLY `external_id` + `external_synced_at` + the bindings table +
  index); pull on `POST /api/board/bindings/{id}/sync` (NOT `/api/board/sync`); reuse
  `forge.rs`; `linear.rs` untouched.

## 3. Pending Work (yours)
- Implement per the 10-step build order in `architecture.md`.
- **Create a fresh branch off `origin/develop`** — NOT this `feat/014d` checkout.
- Port the pure functions **with their existing reference unit tests**.
- Add the **2 AC integration tests**: (a) #58 `POST /api/board/sync {items}` regression;
  (b) fails-loud → **zero board mutation** with a stubbed-unreachable tracker.
- Update `tasks.md` (honest checkboxes).
- Verify: `cargo test -p agentum-core -p agentum-store -p agentum-server --lib` green on macOS.

## 4. Important Decisions (constraints to honor)
- Build **ON** #58 (develop), not parallel. Migration adds ONLY the 2 missing columns
  under the **next-free** number (≥ `0023`; verify with `ls crates/agentum-store/migrations`).
- #58's `POST /api/board/sync {items}` stays **byte-for-byte**; server-pull lives on
  `/api/board/bindings/{id}/sync`.
- **All network I/O before any store write** (fails-loud ⇒ zero mutation; short-circuit with `?`).
- Omit `set_card_external_ref` / `push_card` / all Linear arms (→ 016b/016c).

## 5. Risks (carried forward)
- **#58 regression** → separate route + the regression test (an AC).
- **Migration numbering** → verify next-free at build time; never reuse `0022`.
- **main-checkout WIP hazard** → fresh branch off `origin/develop`; **NEVER `git add -A`**;
  stage only your own hunks; no `checkout`/`reset`/`stash` of the shared tree.
- **Porting drift** → port the reference's unit tests alongside the functions (green = faithful).

## 6. Questions
- None blocking.

## 7. Recommended Next Step
Developer implements 016a on a fresh branch off `origin/develop` per the build order,
reaches green tests (including the 2 AC integration tests), updates `tasks.md`, and stages
only own hunks. Then hand off to **Tester**. (Releases stay human-gated — do not push
develop→staging→main.)
