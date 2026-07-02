//! `GET .../git/history` — commit-history listing and its response DTOs.
use super::*;

#[derive(Debug, Deserialize)]
pub(crate) struct HistoryQuery {
    #[serde(default)]
    limit: Option<u32>,
    // Accepted for parity with the desktop call (its `GitHistoryOptions`), but
    // unused: this panel is scoped to HEAD's history.
    #[serde(default, rename = "baseRef")]
    #[allow(dead_code)]
    base_ref: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryItem {
    id: String,
    parent_ids: Vec<String>,
    subject: String,
    /// Full commit body (`%B`), for the expanded-commit view.
    message: String,
    /// First 8 chars of the oid, for compact display.
    display_id: String,
    author: String,
    author_email: String,
    /// Author date as a unix timestamp (`%at`); null if unparseable.
    timestamp: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryCurrentRef {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryResp {
    items: Vec<HistoryItem>,
    has_incoming_changes: bool,
    has_outgoing_changes: bool,
    has_more: bool,
    limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_ref: Option<HistoryCurrentRef>,
}

/// Parse `git log --pretty=format:%H␟%P␟%s␟%B␟%an␟%ae␟%at␞` (`␟` = unit sep,
/// `␞` = record sep — control chars that don't appear in commit metadata).
/// Records with fewer than 7 fields are skipped. Mirrors the desktop's
/// `git_history` parsing exactly so the history shape is unchanged.
fn parse_history_records(raw: &str) -> Vec<HistoryItem> {
    let mut out = Vec::new();
    for record in raw.split('\u{1e}') {
        let record = record.trim_matches(['\n', '\r']);
        if record.is_empty() {
            continue;
        }
        let fields: Vec<&str> = record.split('\u{1f}').collect();
        if fields.len() < 7 {
            continue;
        }
        let parent_ids = fields[1].split_whitespace().map(str::to_string).collect();
        out.push(HistoryItem {
            id: fields[0].to_string(),
            parent_ids,
            subject: fields[2].to_string(),
            message: fields[3].to_string(),
            display_id: fields[0].chars().take(8).collect(),
            author: fields[4].to_string(),
            author_email: fields[5].to_string(),
            timestamp: fields[6].trim().parse::<i64>().ok(),
        });
    }
    out
}

/// `GET /api/sessions/{id}/git/history?limit=N` — recent commits plus
/// incoming/outgoing-vs-upstream flags and the current ref. Returns the same
/// shape the desktop's native `git_history` produced.
pub(crate) async fn history(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<HistoryResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 1000);
    // \x1f field sep, \x1e record sep; fetch one extra to detect `hasMore`.
    let fmt = "%H\u{1f}%P\u{1f}%s\u{1f}%B\u{1f}%an\u{1f}%ae\u{1f}%at\u{1e}";
    let max_count = format!("--max-count={}", limit + 1);
    let pretty = format!("--pretty=format:{fmt}");
    // An unborn HEAD makes `git log` exit non-zero — treat as no history.
    let raw = run_git(&host, &cwd, &["log", &max_count, &pretty])
        .await
        .unwrap_or_default();
    let mut items = parse_history_records(&raw);
    let has_more = items.len() as u32 > limit;
    items.truncate(limit as usize);

    // Upstream ahead/behind → incoming/outgoing. No upstream → false/false.
    let (incoming, outgoing) = match run_git(
        &host,
        &cwd,
        &["rev-list", "--left-right", "--count", "@{u}...HEAD"],
    )
    .await
    {
        Ok(out) => {
            let mut it = out.split_whitespace();
            let behind: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let ahead: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            (behind > 0, ahead > 0)
        }
        Err(_) => (false, false),
    };

    // currentRef = head oid + ref name (`HEAD` when detached); omitted on unborn.
    let head_oid = run_git(&host, &cwd, &["rev-parse", "HEAD"])
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let name = run_git(&host, &cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let current_ref = match (head_oid, name) {
        (Some(id), Some(name)) => Some(HistoryCurrentRef { id, name }),
        _ => None,
    };

    Ok(Json(HistoryResp {
        items,
        has_incoming_changes: incoming,
        has_outgoing_changes: outgoing,
        has_more,
        limit,
        current_ref,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_history_records_splits_fields_and_short_id() {
        // Two records: one with two parents (a merge), one root commit.
        let raw = "abcdef1234567890\u{1f}p1 p2\u{1f}fix: thing\u{1f}fix: thing\n\nbody\u{1f}Jane\u{1f}jane@x.dev\u{1f}1700000000\u{1e}\
                   0011223344556677\u{1f}\u{1f}init\u{1f}init\u{1f}Bob\u{1f}bob@x.dev\u{1f}1690000000\u{1e}";
        let items = parse_history_records(raw);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "abcdef1234567890");
        assert_eq!(items[0].display_id, "abcdef12");
        assert_eq!(items[0].parent_ids, vec!["p1", "p2"]);
        assert_eq!(items[0].subject, "fix: thing");
        assert!(items[0].message.contains("body"));
        assert_eq!(items[0].author_email, "jane@x.dev");
        assert_eq!(items[0].timestamp, Some(1700000000));
        // Root commit: no parents.
        assert!(items[1].parent_ids.is_empty());
        assert_eq!(items[1].subject, "init");
    }

    #[test]
    fn parse_history_records_skips_short_records() {
        // A record missing fields (e.g. truncated) is dropped, not panicked on.
        assert!(parse_history_records("only\u{1f}two\u{1e}").is_empty());
        assert!(parse_history_records("").is_empty());
    }
}
