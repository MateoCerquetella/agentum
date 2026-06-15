//! `agentum terminal <verb>` + `agentum exec` — drive agentum-managed terminal
//! sessions over the server's existing `/api/sessions` routes (list, `/pane`
//! read, `/send`). Reaches the desktop's embedded server when run inside a pane
//! via `$AGENTUM_API_URL`, else the configured profile or `127.0.0.1:8822`.
//!
//! These are the most-used commands in the agentum-cli skill: an agent inspects
//! and drives sibling terminals here rather than spawning ad-hoc PTYs.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::http::ApiClient;

fn field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

async fn list_sessions(client: &ApiClient) -> Result<Vec<Value>> {
    Ok(client
        .get_json("/api/sessions")
        .await?
        .as_array()
        .cloned()
        .unwrap_or_default())
}

/// Resolve a session by exact name first, then by id prefix (so a uuid or its
/// short form both work). Pure, for unit testing.
pub fn find_session<'a>(sessions: &'a [Value], needle: &str) -> Option<&'a Value> {
    sessions
        .iter()
        .find(|s| field(s, "name") == needle)
        .or_else(|| sessions.iter().find(|s| field(s, "id").starts_with(needle)))
}

fn require_session<'a>(sessions: &'a [Value], needle: &str) -> Result<&'a Value> {
    find_session(sessions, needle).ok_or_else(|| {
        anyhow!("no session named or id-prefixed `{needle}` (try `agentum terminal list`)")
    })
}

async fn pane_lines(client: &ApiClient, id: &str, lines: usize) -> Result<Vec<String>> {
    let resp = client
        .get_json(&format!("/api/sessions/{id}/pane?lines={lines}"))
        .await?;
    Ok(resp
        .get("lines")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default())
}

pub async fn list(json: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let sessions = list_sessions(&client).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }
    if sessions.is_empty() {
        println!("(no sessions)");
        return Ok(());
    }
    let name_w = sessions
        .iter()
        .map(|s| field(s, "name").len())
        .max()
        .unwrap_or(4)
        .max(4);
    println!("{:<nw$}  {:<8}  tool", "NAME", "STATUS", nw = name_w);
    for s in &sessions {
        println!(
            "{:<nw$}  {:<8}  {}",
            field(s, "name"),
            field(s, "status"),
            field(s, "tool"),
            nw = name_w
        );
    }
    Ok(())
}

pub async fn read(name: String, lines: usize, json: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let sessions = list_sessions(&client).await?;
    let id = field(require_session(&sessions, &name)?, "id").to_string();
    // Capture the whole visible pane, then show the last `lines` MEANINGFUL rows.
    // A freshly-started shell draws its prompt at the top with the rest of the
    // grid blank; naively taking the last N rows would return only blanks.
    let captured = pane_lines(&client, &id, 200).await?;
    let out = last_meaningful_lines(&captured, lines);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "lines": out }))?
        );
    } else {
        for l in out {
            println!("{l}");
        }
    }
    Ok(())
}

/// The last `n` rows of a captured pane after dropping trailing blank rows — so
/// a mostly-empty pane shows its actual content, not the blank tail. Pure.
pub fn last_meaningful_lines(captured: &[String], n: usize) -> Vec<String> {
    let end = captured
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    let trimmed = &captured[..end];
    let start = trimmed.len().saturating_sub(n);
    trimmed[start..].to_vec()
}

pub async fn send(name: String, text: Vec<String>, no_enter: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let sessions = list_sessions(&client).await?;
    let id = field(require_session(&sessions, &name)?, "id").to_string();
    let body = json!({ "text": text.join(" "), "append_enter": !no_enter });
    client
        .post_json(&format!("/api/sessions/{id}/send"), &body)
        .await?;
    Ok(())
}

/// Poll a session's pane until it contains `needle`, or `timeout_secs` elapse.
pub async fn wait(name: String, needle: String, timeout_secs: u64) -> Result<()> {
    let client = ApiClient::from_env();
    let sessions = list_sessions(&client).await?;
    let id = field(require_session(&sessions, &name)?, "id").to_string();
    let mut waited = 0u64;
    loop {
        let pane = pane_lines(&client, &id, 200).await?.join("\n");
        if pane.contains(&needle) {
            println!("matched: {needle:?}");
            return Ok(());
        }
        if waited >= timeout_secs {
            bail!("timed out after {timeout_secs}s waiting for {needle:?} in `{name}`");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        waited += 1;
    }
}

/// Run a shell command in a session and capture its output. Sends
/// `<command>` followed by a unique done-marker that also records `$?`, waits
/// for the marker to appear, then returns the pane text the command produced.
/// Best-effort (it reads a tmux pane, not a real exec channel): output is the
/// captured lines between the sent command and the marker.
pub async fn exec(name: String, command: String, timeout_secs: u64) -> Result<()> {
    let client = ApiClient::from_env();
    let sessions = list_sessions(&client).await?;
    let id = field(require_session(&sessions, &name)?, "id").to_string();

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let marker = format!("__AGENTUM_EXEC_DONE_{nonce}__");
    // Emit the marker WITH the exit code on its own line after the command runs.
    let wrapped = format!("{command}; printf '%s:%d\\n' '{marker}' \"$?\"");
    client
        .post_json(
            &format!("/api/sessions/{id}/send"),
            &json!({ "text": wrapped, "append_enter": true }),
        )
        .await?;

    let mut waited = 0u64;
    loop {
        let lines = pane_lines(&client, &id, 400).await?;
        if let Some((output, code)) = extract_exec_output(&lines, &marker) {
            for l in output {
                println!("{l}");
            }
            if code != 0 {
                bail!("command exited with status {code}");
            }
            return Ok(());
        }
        if waited >= timeout_secs {
            bail!("timed out after {timeout_secs}s waiting for `{command}` to finish in `{name}`");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        waited += 1;
    }
}

/// Given captured pane lines, find the done-marker line (`<marker>:<code>`) and
/// return the command's output (the lines AFTER the line that launched the
/// command — which contains the marker text verbatim — and BEFORE the marker
/// result line) plus the parsed exit code. `None` until the marker appears.
/// Pure, for unit testing.
pub fn extract_exec_output(lines: &[String], marker: &str) -> Option<(Vec<String>, i32)> {
    // The result line looks like `<marker>:0`. The command-echo line contains
    // the marker as part of the typed command (`...printf ... <marker> ...`).
    let result_idx = lines
        .iter()
        .rposition(|l| l.contains(&format!("{marker}:")))?;
    let code = lines[result_idx]
        .rsplit(':')
        .next()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(0);
    // The echo of the typed command also contains the bare marker; find the last
    // such line BEFORE the result line — output sits between them.
    let echo_idx = lines[..result_idx]
        .iter()
        .rposition(|l| l.contains(marker))
        .map(|i| i + 1)
        .unwrap_or(0);
    let output = lines[echo_idx..result_idx].to_vec();
    Some((output, code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn find_session_by_name_then_id_prefix() {
        let sessions = vec![
            json!({"name": "build", "id": "abc123-def"}),
            json!({"name": "test", "id": "999000-aaa"}),
        ];
        assert_eq!(
            field(find_session(&sessions, "test").unwrap(), "id"),
            "999000-aaa"
        );
        // id prefix also resolves
        assert_eq!(
            field(find_session(&sessions, "abc123").unwrap(), "name"),
            "build"
        );
        assert!(find_session(&sessions, "nope").is_none());
    }

    #[test]
    fn extract_exec_output_pulls_lines_between_echo_and_marker() {
        let marker = "__AGENTUM_EXEC_DONE_42__";
        let lines = vec![
            format!("$ echo hi; printf '%s:%d' '{marker}' \"$?\""), // command echo (contains marker)
            "hi".to_string(),                                       // the actual output
            format!("{marker}:0"),                                  // result line
            "$ ".to_string(),                                       // next prompt
        ];
        let (out, code) = extract_exec_output(&lines, marker).unwrap();
        assert_eq!(out, vec!["hi".to_string()]);
        assert_eq!(code, 0);
    }

    #[test]
    fn extract_exec_output_none_until_marker_present() {
        let lines = vec!["still running".to_string()];
        assert!(extract_exec_output(&lines, "__AGENTUM_EXEC_DONE_1__").is_none());
    }

    #[test]
    fn last_meaningful_lines_drops_trailing_blanks() {
        let pane = vec![
            "$ echo hi".to_string(),
            "hi".to_string(),
            "$ ".to_string(),
            "".to_string(),
            "   ".to_string(),
        ];
        // Trailing blank/whitespace rows are dropped; the last 2 real rows show.
        assert_eq!(
            last_meaningful_lines(&pane, 2),
            vec!["hi".to_string(), "$ ".to_string()]
        );
        // Asking for more than exist returns all meaningful rows.
        assert_eq!(last_meaningful_lines(&pane, 10).len(), 3);
        // An all-blank pane yields nothing rather than rows of whitespace.
        assert!(last_meaningful_lines(&vec!["".to_string(), "  ".to_string()], 5).is_empty());
    }

    #[test]
    fn extract_exec_output_reports_nonzero_exit() {
        let marker = "__AGENTUM_EXEC_DONE_7__";
        let lines = vec![
            format!("cmd '{marker}'"),
            "boom".to_string(),
            format!("{marker}:2"),
        ];
        let (_, code) = extract_exec_output(&lines, marker).unwrap();
        assert_eq!(code, 2);
    }
}
