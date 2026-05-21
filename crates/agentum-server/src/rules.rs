//! Composition layer: resolves the per-status required-field set by
//! looking in the `board_column_rules` table first, then falling back
//! to the slice-1 const matrix in `agentum-core`. Lives in
//! `agentum-server` rather than core because it touches the store —
//! core stays DB-free.

use std::borrow::Cow;
use std::collections::BTreeMap;

use agentum_core::{RequiredField, required_fields_for};
use agentum_store::Store;

use crate::error::ApiError;

/// Column names always present in `GET /api/board/rules` even when the
/// DB is empty — synthesised from the const for the spec's "defaults
/// come back" AC.
const DEFAULT_COLUMNS: &[&str] = &["todo", "doing", "done"];

/// Resolve the required-field set for one column. DB override wins; an
/// absent row falls back to the const (which itself returns an empty
/// slice for custom columns — that's the passthrough path).
///
/// `Cow` keeps the const path zero-alloc; only DB overrides allocate.
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

/// Build the merged matrix the `GET /api/board/rules` handler returns.
/// Defaults from the const are pre-populated; DB rows overwrite the
/// const for matching keys and add custom columns on top.
pub async fn merged_rule_matrix(
    store: &Store,
) -> Result<BTreeMap<String, Vec<RequiredField>>, ApiError> {
    let mut out = BTreeMap::new();
    for col in DEFAULT_COLUMNS {
        out.insert((*col).to_string(), required_fields_for(col).to_vec());
    }
    for (col, fields) in store.list_board_column_rules().await? {
        // Overrides win — including when the override is `[]` for a
        // default column, which is the explicit "drop the gate" signal.
        out.insert(col, fields);
    }
    Ok(out)
}
