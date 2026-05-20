---
created: 2026-05-19
title: Architecture notes — Typed kanban card schemas
spec: ./2026-05-19-typed-kanban-card-schemas.md
stage: architecture
---

# Architecture Notes

Decisions 1–5 in the spec are settled and not reopened here. This file fixes
the *structural shape* of slice 1 so the developer can implement without
re-litigating placement.

## Spec issues

1. **AC mismatch on status code.** Decision 5 settles on `400`, but the
   dialog AC line says "On 422 from the server, map `missing[]` to field
   highlights". Change to `400`. The dialog should branch on payload shape
   (`{missing, status}`), not the code.
2. **AC wording asks for too much from the patch path.** `Transitions out
   of a gated column never re-validate` is correct, but the more general
   invariant is *only validate when the PATCH body sets a `status` that
   differs from the row's current status*. A PATCH that doesn't touch
   `status` (e.g. user just edits `title`) must not trigger gate logic
   even if the row's current state would fail it — that's the
   grandfathering rule. State this explicitly in the dev brief.
3. **No drag-drop snap-back contract on the wire.** The frontend AC
   describes snap-back, but the server contract doesn't change — the 400
   response is the same. Worth flagging so the dev doesn't invent a new
   field.

## 1. Matrix location and shape (`agentum-core`)

**New module:** `crates/agentum-core/src/board_schema.rs`, re-exported
from `lib.rs`.

**Shape:** plain `match` function over a `&str`, returning a
`&'static [RequiredField]`. **Not** a phf map, not a typed enum. Rationale:

- Three columns × <6 fields each. A map or enum buys nothing over `match`.
- Custom columns must passthrough (spec: "any status not in the matrix
  bypasses"). A `match` with a `_ => &[]` arm expresses this in one line;
  an enum forces a `TryFrom<&str>` plus a fallback path on every callsite.
- The "extensibility for the next spec (custom rules)" pressure is a red
  herring — the next spec replaces the const with a DB table read; either
  representation gets rewritten then. `const → table` is the same refactor
  regardless. Pick the form that's smallest *today*.

```rust
// crates/agentum-core/src/board_schema.rs

/// Fields a card must satisfy to *enter* the column. The `done` OR-clause
/// (session_id OR ≥1 comment) is encoded as a single synthetic field
/// `SessionOrComment`; the validator resolves the disjunction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredField {
    Title,
    Lbl,
    Workdir,
    Tool,
    ClaimedBy,
    SessionOrComment,
}

impl RequiredField {
    pub fn as_missing_key(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Lbl => "lbl",
            Self::Workdir => "workdir",
            Self::Tool => "tool",
            Self::ClaimedBy => "claimed_by",
            Self::SessionOrComment => "session_id_or_comment",
        }
    }
}

/// Empty slice ⇒ no gate (custom columns passthrough).
pub fn required_fields_for(status: &str) -> &'static [RequiredField] {
    use RequiredField::*;
    match status {
        "todo"  => &[Title, Lbl],
        "doing" => &[Title, Lbl, Workdir, Tool, ClaimedBy],
        "done"  => &[Title, Lbl, SessionOrComment],
        _       => &[],
    }
}
```

That's the entire module surface besides the validator (next section).

## 2. Validation API in `agentum-core`

**Do not** hang validation off `NewBoardItem` / `BoardPatch` as inherent
methods. Those are wire types — keeping them dumb DTOs preserves the
existing serde-only pattern and avoids dragging the validator into every
consumer of the type (the store, the CLI, future TUI). Free function
instead, in `board_schema.rs`:

```rust
/// Snapshot the validator needs. Built by the handler from the
/// existing row (PATCH) or zeroed (POST), then merged with the patch.
#[derive(Debug, Clone, Default)]
pub struct TransitionCtx<'a> {
    pub title:       Option<&'a str>,
    pub lbl:         Option<&'a str>,
    pub workdir:     Option<&'a str>,
    pub tool:        Option<&'a str>,
    pub claimed_by:  Option<&'a str>,
    pub session_id:  Option<&'a str>,
    pub has_comment: bool,
}

/// Returns `Err(missing_keys)` on gate failure, `Ok(())` on pass or
/// custom-column passthrough. `&'static str` keys map directly into
/// the JSON `missing` array — no allocation.
pub fn validate_transition(
    target_status: &str,
    ctx: &TransitionCtx<'_>,
) -> Result<(), Vec<&'static str>> { … }
```

**Why a free function with a `TransitionCtx`**, not a method on
`BoardPatch`:

- The handler already has to fetch the current row and (sometimes) the
  comment-existence flag. Building a `TransitionCtx` from those + the
  patch is one place where the "merge patch over current" logic lives.
  Putting it on `BoardPatch` would force `BoardPatch` to know about
  `BoardItem` and comments — leakage.
- Tests live in `board_schema.rs` and don't need to construct full
  `BoardItem` / `BoardPatch` instances; just literal `TransitionCtx`.

**Handler stays thin** — three lines: build ctx, call validator, map err.

## 3. Comments-existence check (`agentum-store`)

`Store` is a concrete struct, not a trait — no abstraction to widen.
Add a single new method:

```rust
/// True iff at least one row in `board_comments` references this id.
pub async fn has_board_comments(&self, board_id: i64) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM board_comments WHERE board_id = ? LIMIT 1",
    )
    .bind(board_id)
    .fetch_optional(&self.pool)
    .await?;
    Ok(row.is_some())
}
```

**Do not** fold this into a `fetch_for_validation` combo method. Reasons:

- POST has no id yet → the comments check is structurally not applicable;
  forcing it through a combo method means the POST path either passes a
  sentinel `-1` or skips the call awkwardly. Two callsites with different
  shapes ⇒ two simple methods beats one polymorphic one.
- `get_board_item(id)` already exists and is the right primitive for
  fetching the current row. The handler calls `get_board_item` + (only
  for `done` transitions) `has_board_comments`. Both are indexed lookups;
  total cost is two cheap queries on the gated path.
- The `LIMIT 1` form is intentional — cheaper than `COUNT(*)` and the
  store already has `count_board_comments` for the bulk-counts callsite,
  so don't repurpose it.

**Cost ceiling:** the comments check only runs when `target_status ==
"done"`. Other transitions never touch `board_comments`.

## 4. Where validation runs

**Inline at the top of `create` and `patch` in `routes/board.rs`.** No
extractor, no middleware.

- Two callsites, both already need access to `state.store` (for the row
  fetch on PATCH and the comment check on `done`). An extractor would
  need to re-issue those calls; sharing the result across extractor and
  handler is awkward in Axum without a request-extension dance.
- The only thing both callsites genuinely share is "given a target
  status + ctx, return missing fields, else 400". That's exactly the
  free function in §2. Pulling it into middleware adds a layer for one
  line of reuse.
- If a third gated endpoint shows up, lift it then. YAGNI.

**Suggested helper inside `routes/board.rs`** (private, ~15 lines) so
the two handlers don't duplicate the ctx-building boilerplate:

```rust
async fn enforce_transition(
    store: &Store,
    bus: &EventBus,
    id: Option<i64>,                 // None for POST
    target_status: &str,
    incoming: &TransitionCtx<'_>,    // built from patch/payload
    base_row: Option<&BoardItem>,    // current row on PATCH; None on POST
) -> Result<(), ApiError> { … }
```

This is the only piece of glue. Returns `Err(ApiError::BadRequest)` with
the JSON shape `{"missing": [...], "status": target}`. **Emit the
`board.transition.rejected` event here**, before returning Err — single
source of truth.

## 5. Event emission

Spec proposes `board.transition.rejected` with payload `{id,
target_status, missing}`. Confirmed consistent with the existing dotted
namespace (`board.created`, `board.updated`, `board.claimed`,
`board.commented`, `board.deleted`, `board.reordered`, `board.released`).

**One tweak:** on POST, there's no `id` yet (validation runs before
insert). Use `null` for `id` rather than omitting the key — keeps the
downstream consumer's parsing branch-free. Payload shape:

```json
{ "id": null, "target_status": "doing", "missing": ["workdir","tool","claimed_by"] }
```

Document this in the event payload schema; the dashboard watchdog feed
filters on `board.*` already so no client change required for visibility.

## 6. Frontend symmetry

**Pick (a) hardcode twice. Ship the duplication.**

- Slice 1 has no API consumer beyond the dashboard. Adding
  `GET /api/board/schema` is a new endpoint with auth+router+test
  overhead for *one* client whose source we control.
- Baking metadata into `GET /api/board` (option c) bloats the hot list
  payload that already carries items + comment counts; the schema is
  static, doesn't belong in a per-request response.
- The matrix is three rows × <6 fields. The drift risk is real but
  bounded: the server is the authority (it returns 400 on mismatch), so
  drift degrades UX (server rejects something the form let through) but
  never corrupts data.

**Mitigation that costs nothing:**

- Put the TS matrix in a single file (`dashboard/src/lib/board-schema.ts`)
  with a top-of-file comment: `// SOURCE OF TRUTH: crates/agentum-core/src/board_schema.rs::required_fields_for. Keep in sync.`
- Have one Svelte-side unit test that asserts the matrix shape against
  the values in the spec table (the same table the Rust test asserts
  against). If either drifts from the spec, both fail.
- When the next spec lands (custom per-project rules), the schema *will*
  go over the wire — that's the right time to introduce the endpoint,
  not now.

## 7. Risks the spec hasn't flagged

1. **Optimistic UI ⇄ server rejection race in `board.ts`.** `moveLocal`
   mutates the store synchronously before the PATCH lands. On a 400, the
   card has already visually moved. Snap-back needs to revert that
   optimistic move *and* re-open the dialog. The natural fix: have the
   drag handler capture the origin column, call `moveLocal(id, target)`,
   fire the PATCH, and on rejection call `moveLocal(id, origin)` *then*
   open the dialog. Without that explicit revert the next `loadBoard()`
   reconciles eventually but the UI flashes inconsistent in the
   meantime. Worth one line in the dev brief.

2. **Concurrent transitions are not serialized.** Two clients can
   simultaneously PATCH the same id into different gated columns. Both
   read the same row state, both pass validation, both UPDATE. The
   second write wins, but the *event log* records two transitions
   neither of which was rejected. Not a regression (today's PATCH has
   the same race), and the spec doesn't list it, but the gate now
   makes the inconsistency more visible. Mitigation = compare-and-swap
   on `status` in `patch_board_item` — explicitly *out of scope* for
   this slice; flag in `.planning/todos/pending/` for a follow-up.

3. **`claim` does not transition status (confirmed).** `routes/board.rs:121`
   only touches `claimed_by`. The gate doesn't apply. **However**: a
   common UX pattern would be "claim implies move to doing"; today
   nothing implements that, but if it ever does, the gate must apply
   to the implied transition. Out of scope here, but document the
   invariant: *anywhere status mutates, the gate runs*. If a future PR
   adds a "claim & start" endpoint, it owes a validation call.

4. **Grandfathered rows can be edited without re-validation.** Spec
   explicitly says this is fine (silent grandfathering). The subtle
   case: a PATCH that mutates `claimed_by` on a card stuck in `doing`
   without a `workdir` passes through. Correct per the decision — but
   reviewers will ask. Worth one line in the PR description.

5. **Test fixture churn in `agentum-store`'s lib tests.** `CLAUDE.md`
   notes pre-existing breakage from `NewBoardItem` field churn. New
   tests should live in `agentum-server` (handler-level) and
   `agentum-core` (validator-level), not `agentum-store`. The store
   changes are mechanical (one new method) and don't need new store
   tests beyond a smoke `has_board_comments` test.

## Implementation order (suggestion to dev)

1. `agentum-core`: new `board_schema` module + tests (matrix +
   validator, no I/O).
2. `agentum-store`: `has_board_comments` method.
3. `agentum-server`: `enforce_transition` helper + wire it into
   `create` and `patch` + event emission + handler tests covering the
   five AC test cases.
4. `dashboard`: mirror TS matrix + dialog hints/disable + drag snap-back
   in `board.ts` (revert via `moveLocal`).
5. Add `.planning/todos/pending/board-transition-cas.md` for the
   serialization-race follow-up (risk #2).
