//! `agentum tab/snapshot/click/fill/navigate` — drive the desktop's browser
//! pane over `/api/browser/*`. Only works against a running desktop (the
//! standalone daemon 501s these); reaches it via `$AGENTUM_API_URL`.

use anyhow::Result;
use serde_json::{Value, json};

use crate::http::ApiClient;

fn tab_arg(tab: Option<String>) -> Value {
    match tab {
        Some(t) => json!({ "tab": t }),
        None => json!({}),
    }
}

pub async fn tab_list(json_out: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let resp = client.post_json("/api/browser/tabs", &json!({})).await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else {
        let tabs = resp
            .get("tabs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if tabs.is_empty() {
            println!("(no browser tabs open)");
        }
        for t in tabs {
            println!(
                "{}  {}",
                t.get("tab").and_then(Value::as_str).unwrap_or("?"),
                t.get("url").and_then(Value::as_str).unwrap_or(""),
            );
        }
    }
    Ok(())
}

pub async fn tab_open(url: String, json_out: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let resp = client
        .post_json("/api/browser/open", &json!({ "url": url }))
        .await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else {
        // The new tab id is what the other ops take as `--tab`; print it plainly.
        match resp.get("tab").and_then(Value::as_str) {
            Some(tab) => println!("{tab}"),
            None => println!("{resp}"),
        }
    }
    Ok(())
}

pub async fn snapshot(tab: Option<String>, json_out: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let resp = client
        .post_json("/api/browser/snapshot", &tab_arg(tab))
        .await?;
    println!("{}", serde_json::to_string_pretty(&resp)?);
    let _ = json_out;
    Ok(())
}

pub async fn navigate(url: String, tab: Option<String>) -> Result<()> {
    let client = ApiClient::from_env();
    let mut body = tab_arg(tab);
    body["url"] = Value::String(url);
    client.post_json("/api/browser/navigate", &body).await?;
    println!("ok");
    Ok(())
}

pub async fn click(selector: String, tab: Option<String>) -> Result<()> {
    let client = ApiClient::from_env();
    let mut body = tab_arg(tab);
    body["selector"] = Value::String(selector);
    client.post_json("/api/browser/click", &body).await?;
    println!("ok");
    Ok(())
}

pub async fn fill(selector: String, text: String, tab: Option<String>) -> Result<()> {
    let client = ApiClient::from_env();
    let mut body = tab_arg(tab);
    body["selector"] = Value::String(selector);
    body["text"] = Value::String(text);
    client.post_json("/api/browser/fill", &body).await?;
    println!("ok");
    Ok(())
}
