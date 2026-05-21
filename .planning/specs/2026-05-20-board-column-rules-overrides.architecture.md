---
created: 2026-05-20
title: Architecture notes — Board column rules (per-server overrides)
spec: ./2026-05-20-board-column-rules-overrides.md
stage: architecture
---

# Architecture Notes — Board Column Rules (slice 2)

Decisions 1–4 in the spec are settled and not reopened. This file fixes the structural shape so the developer doesn't re-litigate placement.

## 1. Lookup function location — Option (a) with a twist

Keep `required_fields_for` pure-const in `agentum-core`. **Do not** introduce a trait (option b — over-engineered for two callsites and zero alternative impls) and **do not** move the lookup into `agentum-store` (option c — `Store` would have to depend on `agentum-core::RequiredField`, which it currently doesn't, and the const default becomes data instead of code).

Put the resolved-lookup helper in a new module **`crates/agentum-server/src/rules.rs`** (not inside `routes/board.rs`). It owns:

```rust
// agentum-server::rules
pub async fn resolve_required_fields(
    store: &Store,
    status: &str,
) -> Result<Cow<'static, [RequiredField]>, ApiError> {
    if let Some(override_) = store.get_board_column_rule(status).await? {
        Ok(Cow::Owned(override_))
    } else {
        Ok(Cow::Borrowed(required_fields_for(status)))
    }
}

pub async fn merged_rule_matrix(
    store: &Store,
) -> Result<BTreeMap<String, Vec<RequiredField>>, ApiError> { /* §4 */ }
```

`Cow<'static, [RequiredField]>` lets the const path stay zero-alloc; only the DB-hit path allocates. The validator in `agentum-core::validate_transition` already takes a `&[RequiredField]` conceptually (it iterates `required_fields_for` internally) — but it doesn't accept one as a parameter today. **Required edit to slice-1 code**: split `validate_transition` into:

```rust
// agentum-core
pub fn validate_against(
    required: &[RequiredField],
    ctx: &TransitionCtx<'_>,
) -> Result<(), Vec<&'static str>>;

pub fn validate_transition(target_status: &str, ctx: &TransitionCtx<'_>) -> Result<(), Vec<&'static str>> {
    validate_against(required_fields_for(target_status), ctx)
}
```

The existing function stays as a thin shim — slice-1 tests keep passing, and slice 2 calls `validate_against` directly from `enforce_transition` after `resolve_required_fields`. This is the smallest change that lets `agentum-core` stay DB-free while the server composes the lookup.

Re: "wire types dumb DTOs only" — that constraint applies to `NewBoardItem` / `BoardPatch`, not to `required_fields_for`. The const matrix is domain logic (per-status invariants), not a wire type. It legitimately belongs in `agentum-core`. The new module `agentum-server::rules` is the *composition* layer — it knows about both the const and the store, which is exactly the server's job.

## 2. `Store` method signatures

Four methods, JSON deserialized **inside** the store. Rationale: both callsites (rules CRUD + gate lookup) want `Vec<RequiredField>`, not `String`. Pushing the deser to two callsites means two places to get it wrong, and `Store` already does JSON work for other tables. The store becomes the parsing boundary.

```rust
impl Store {
    /// Single-column lookup. None = no override row (caller falls back to const).
    pub async fn get_board_column_rule(
        &self,
        column: &str,
    ) -> Result<Option<Vec<RequiredField>>>;

    /// All overrides. Empty map when the table is empty. The merge with
    /// the const happens in `agentum-server::rules` (§4).
    pub async fn list_board_column_rules(
        &self,
    ) -> Result<BTreeMap<String, Vec<RequiredField>>>;

    /// Upsert. Caller has already validated field names (§5).
    pub async fn upsert_board_column_rule(
        &self,
        column: &str,
        fields: &[RequiredField],
    ) -> Result<()>;

    /// Returns Ok(true) iff a row was actually deleted (so the handler
    /// can choose 200 vs 404 — see §6 on routing).
    pub async fn delete_board_column_rule(&self, column: &str) -> Result<bool>;
}
```

`agentum-store` gains a dep on `agentum-core` for `RequiredField` — already true (`BoardItem` lives in core). Serde impls on `RequiredField`: explicit `#[serde(rename = "...")]` per variant matching `as_missing_key()`. Avoid `rename_all = "snake_case"` because it won't auto-produce `session_id_or_comment` from `SessionOrComment`. Explicit per-variant renames make the wire format canonical and let the typed enum flow through the whole stack.

## 3. `enforce_transition` integration

**Pass the store explicitly.** `enforce_transition` already receives `&Store` as its first arg today — slice 2 changes nothing about its signature. It picks up an extra `.await` inside.

**Lookup lives inside `enforce_transition`, not in `TransitionCtx`.** `TransitionCtx` is a *value snapshot* of the card state — adding the rules lookup to it conflates two concerns. The caller (POST/PATCH handler) shouldn't have to know whether it needs to pre-fetch rules; that's the gate's responsibility.

Concrete shape inside `enforce_transition`:

```rust
// Resolve rules once. Cow keeps the const path allocation-free.
let required = rules::resolve_required_fields(store, target_status).await?;

if target_status == "done" {
    if let Some(real_id) = id {
        ctx.has_comment = store.has_board_comments(real_id).await?;
    }
}

match agentum_core::validate_against(&required, ctx) { /* … existing … */ }
```

**"At most one store call per gate evaluation" as a structural invariant**: today's code calls `has_board_comments` only on `done`. Slice 2 adds exactly one more: `get_board_column_rule(target_status)`. The invariant is "rules-lookup is unconditional, comments-check is `done`-only" — and it's expressed by the linear shape of the function body: one `resolve_required_fields` call, no loop, no conditional skip. Enforce in tests with a counter (or just a unit test on `resolve_required_fields` asserting one query). Don't try to express it in the type system — too much ceremony for a 15-line function.

## 4. `GET /api/board/rules` merging — shared helper, not duplicated

Put the merge in `agentum-server::rules::merged_rule_matrix`. Returns `BTreeMap<String, Vec<RequiredField>>` containing:
- All three default columns (`todo`, `doing`, `done`) populated from const **unless** the DB has an override for them, in which case the override replaces the entry.
- Any extra columns present in `board_column_rules` get added.
- Custom columns with no row are **not** present (spec AC).

```rust
pub async fn merged_rule_matrix(store: &Store) -> Result<BTreeMap<String, Vec<RequiredField>>, ApiError> {
    let mut out = BTreeMap::new();
    for col in ["todo", "doing", "done"] {
        out.insert(col.to_string(), required_fields_for(col).to_vec());
    }
    for (col, fields) in store.list_board_column_rules().await? {
        out.insert(col, fields); // overrides win
    }
    Ok(out)
}
```

The route handler serializes `BTreeMap<String, Vec<RequiredField>>` as `{"todo": ["title","lbl"], …}` — the `Serialize` impl from §2 maps `RequiredField` → `"title"` etc. directly.

## 5. PUT field-name validation — add `RequiredField::from_missing_key`

Add to `agentum-core::board_schema`:

```rust
impl RequiredField {
    pub fn from_missing_key(s: &str) -> Option<Self> {
        match s {
            "title" => Some(Self::Title),
            "lbl" => Some(Self::Lbl),
            "workdir" => Some(Self::Workdir),
            "tool" => Some(Self::Tool),
            "claimed_by" => Some(Self::ClaimedBy),
            "session_id_or_comment" => Some(Self::SessionOrComment),
            _ => None,
        }
    }
}
```

The PUT handler parses `Vec<String>` from the body, maps each through `from_missing_key`, and returns `ApiError::BadRequest(format!("unknown field: {name}"))` on the first miss. Use `ApiError::BadRequest`, **not** `ApiError::Custom` — the spec AC pins the body to `{"error": "unknown field: wat"}`, exactly what `BadRequest` produces. `Custom` stays reserved for the gate's `{missing, status}` shape.

Why `from_missing_key` over a `validate_field_name(&str) -> bool`: the parser is the reusable primitive. It gives the handler the typed value it needs to call `store.upsert_board_column_rule(&fields)` without re-parsing. The boolean version forces a second pass.

## 6. API routing — new module `routes/board_rules.rs`

`routes/board.rs` is already long (gate + 7 handlers + Pass B's tests). The rules CRUD is structurally a separate resource — `/api/board/rules` operates on a *different table*, has *different auth shape* (admin-leaning even if slice 2 keeps the same token gate), and doesn't share code with the items handlers beyond `AppState`.

Plan:

```
crates/agentum-server/src/routes/board_rules.rs   // new file
   pub fn router() -> Router<AppState> { … }      // GET / PUT / DELETE
```

Register in `crates/agentum-server/src/lib.rs` alongside `routes::board::router()`. The helper module (`agentum-server::rules`) and the routes module (`agentum-server::routes::board_rules`) coexist via distinct paths — two names, no collision.

DELETE returns 200 with empty body when a row was deleted, 404 when no row existed (the `bool` from §2 drives this). Spec doesn't require the distinction but it's the conventional REST shape and costs nothing.

## 7. Event emission on rule changes — yes, emit two events

`board.rules.updated` (payload: `{column, required_fields}`) on PUT, `board.rules.deleted` (payload: `{column}`) on DELETE. Reasons:

1. The dashboard's WS event bus filters on `board.*` already.
2. Slice 3 ships dashboard rule fetching; emitting now means slice 3 is a listener-only change.
3. Cost is one `bus.send(...)` per CRUD call — same pattern as `board.created` / `board.updated`.

Don't emit on GET. Don't emit on PUT-with-no-change — the row's `updated_at` changes but the rule state doesn't. Two-line equality check before the emit.

## 8. Risks the spec hasn't flagged

1. **Rule-change ⇄ in-flight transition race.** Client A PUTs `["title","lbl","workdir"]` for `doing`; Client B PATCHes a card into `doing` simultaneously. Both reads, no locking. B's transition is validated against whatever rule it happens to read. Outcome: B's PATCH either gets rejected by the new rule or sneaks through under the old one. **Not a real bug** — there's no "correct" winner; the user issuing the PATCH at the moment of a rule change has to accept either result. SQLite's serializable default makes the *individual* transaction atomic, which is enough. Don't add CAS. Flag in the dev brief.

2. **Empty-array PUT is idempotent — but `updated_at` bumps anyway.** Spec AC says "issuing the same PUT twice produces the same state and the second call still returns 200." `updated_at` will change on the second PUT. If a future test asserts deep-equality of two GETs across a re-PUT, it'll fail on `updated_at`. Trivial, but worth noting: `updated_at` is **not** exposed in `GET /api/board/rules` (the spec only shows `column → required_fields`). Keep it that way until a dedicated audit endpoint lands.

3. **`from_missing_key` and the JSON column will drift if `RequiredField` grows/shrinks variants.** A removed variant means old rows fail to deserialize. **Mitigation:** deserialize per-element with `from_missing_key`, **skip unknown strings with a `warn!` log** rather than failing the whole row. User sees a column with fewer requirements than they configured; server stays up. Document this as the forward-compat policy. (Spec Decision 2 deferred versioning — this is the cheap version of that.)

4. **`board.rules.updated` on a column with no items.** Firing an event for a column nobody uses is fine. The spec's "dashboard reactively refreshes" assumes the dashboard has a rules store; in slice 2 it doesn't. The events are dead-letter until slice 3. Acceptable — emit early so slice 3 is a listener add, not a server retrofit.

5. **Migration `0014` must be idempotent across an upgrade.** Slice 1 shipped no rules table; slice 2's `CREATE TABLE` runs on daemons that already have a full `board_items` table. Pure add — no concern, but the dev should smoke-test `agentum serve` against a slice-1-era SQLite file before merging.

## Implementation order (suggestion to dev)

1. `agentum-core::board_schema`: split `validate_transition` → `validate_against` + thin shim; add `RequiredField::from_missing_key`; add `Serialize`/`Deserialize` derives with `#[serde(rename)]` per variant.
2. `agentum-store`: migration `0014` + four methods (`get/list/upsert/delete_board_column_rule`). Per-element `from_missing_key` deser with `warn!` on unknown.
3. `agentum-server::rules` (helper module): `resolve_required_fields`, `merged_rule_matrix`.
4. `agentum-server::routes::board.rs`: swap `enforce_transition`'s internals to call `resolve_required_fields` + `validate_against`. Verify all slice-1 handler tests still pass.
5. `agentum-server::routes::board_rules.rs` (new): three handlers + `board.rules.updated/deleted` events with the no-op-skip check.
6. Wire the new router in `agentum-server::lib.rs`.
7. Tests in `agentum-server` per spec AC.
