//! Files attached in chat. The page pulls the text out (PDF, Word, Excel,
//! PowerPoint, plain text) and stores it here. The model gets the first part
//! with the message and reads the rest with file_read, so long files aren't cut.
use crate::{state::App, util::*};
use axum::{extract::{DefaultBodyLimit, State}, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

/// Characters per part (~2-3k tokens).
pub const PART: usize = 8000;
/// How much of the attached files goes in with the message itself.
const INLINE: usize = 16_000;
const MAX_CHARS: usize = 2_000_000;

pub fn routes() -> Router<App> {
    Router::new().route("/api/chat/files", post(upload)).layer(DefaultBodyLimit::max(12 * 1024 * 1024))
}

#[derive(Deserialize)]
struct Upload { name: String, #[serde(default)] kind: String, #[serde(default)] size: i64, text: String }

async fn upload(State(app): State<App>, Json(u): Json<Upload>) -> R<Json<Value>> {
    let text = u.text.trim().replace("\r\n", "\n");
    if text.is_empty() { return Err(bad("No text found in this file. A scanned PDF or a picture has none to read.")); }
    let text: String = text.chars().take(MAX_CHARS).collect();
    let (id, name) = (uuid::Uuid::new_v4().simple().to_string()[..12].to_string(), u.name.chars().take(200).collect::<String>());
    let info = json!({"id": id, "name": name, "size": u.size, "chars": text.chars().count(), "parts": parts(&text).len()});
    app.db.call(move |c| c.execute("INSERT INTO chat_files(id,name,kind,size,text,created_at) VALUES(?1,?2,?3,?4,?5,datetime('now'))",
        rusqlite::params![id, name, u.kind, u.size, text])).await?;
    Ok(Json(info))
}

/// Split into parts of about PART characters, at a line break when there is one.
pub fn parts(t: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut rest = t;
    while !rest.is_empty() {
        let end = rest.char_indices().nth(PART).map(|(i, _)| i).unwrap_or(rest.len());
        let cut = if end < rest.len() { rest[..end].rfind('\n').filter(|&i| i > end / 2).map(|i| i + 1).unwrap_or(end) } else { end };
        out.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    if out.is_empty() { out.push(""); }
    out
}

pub struct File { pub id: String, pub name: String, pub text: String }

pub async fn load(app: &App, ids: &[String]) -> Vec<File> {
    let ids: Vec<String> = ids.iter().take(10).cloned().collect();
    app.db.call(move |c| {
        let mut out = vec![];
        for id in ids {
            if let Ok(f) = c.query_row("SELECT id, coalesce(name,''), coalesce(text,'') FROM chat_files WHERE id=?1", [&id], |r| Ok(File { id: r.get(0)?, name: r.get(1)?, text: r.get(2)? })) { out.push(f); }
        }
        Ok(out)
    }).await.unwrap_or_default()
}

/// What the page shows on the message, and what later turns are told.
pub fn meta(files: &[File]) -> Value {
    json!(files.iter().map(|f| json!({"id": f.id, "name": f.name, "chars": f.text.chars().count(), "parts": parts(&f.text).len()})).collect::<Vec<_>>())
}

/// A line telling the model a file is there and how to read it.
pub fn marker(id: &str, name: &str, parts: u64) -> String {
    format!("[Attached file “{name}” (file id={id}, {parts} part{}). Read it with file_read.]", if parts == 1 { "" } else { "s" })
}

/// The files as text to send along with the user's message: whole first
/// parts while they fit, the rest as markers the model can follow up on.
pub fn inline(files: &[File]) -> String {
    let mut used = 0;
    files.iter().map(|f| {
        let ps = parts(&f.text);
        let first = ps[0];
        if used + first.len() > INLINE { return marker(&f.id, &f.name, ps.len() as u64); }
        used += first.len();
        let more = if ps.len() > 1 { format!("\n[That was part 1 of {}. Use file_read with id={} and part 2–{} for the rest, or a query to find lines.]", ps.len(), f.id, ps.len()) } else { String::new() };
        format!("[Attached file “{}” (file id={}){}:]\n{first}{more}", f.name, f.id, if ps.len() > 1 { ", part 1" } else { "" })
    }).collect::<Vec<_>>().join("\n\n")
}

/// The file_read tool: one part, or the lines that match a query.
pub async fn read(app: &App, a: &Value) -> anyhow::Result<String> {
    let id = s(a, "id");
    let f = load(app, std::slice::from_ref(&id)).await.into_iter().next().ok_or_else(|| anyhow::anyhow!("no file with id {id}"))?;
    let ps = parts(&f.text);
    let q = s(a, "query").trim().to_lowercase();
    if !q.is_empty() {
        let words: Vec<&str> = q.split_whitespace().filter(|w| w.chars().count() >= 3).collect();
        let mut hits = vec![];
        for (n, p) in ps.iter().enumerate() {
            for line in p.lines() {
                let l = line.to_lowercase();
                if l.contains(&q) || (!words.is_empty() && words.iter().all(|w| l.contains(w))) {
                    hits.push(format!("part {}: {}", n + 1, line.trim().chars().take(300).collect::<String>()));
                }
            }
        }
        if hits.is_empty() { return Ok(format!("No lines in “{}” match “{q}”. It has {} parts; read them with part.", f.name, ps.len())); }
        let more = if hits.len() > 40 { format!("\n({} more matches)", hits.len() - 40) } else { String::new() };
        return Ok(format!("Lines in “{}” matching “{q}”:\n{}{more}", f.name, hits.into_iter().take(40).collect::<Vec<_>>().join("\n")));
    }
    let n = a["part"].as_u64().unwrap_or(1).max(1) as usize;
    let p = ps.get(n - 1).ok_or_else(|| anyhow::anyhow!("“{}” has {} parts", f.name, ps.len()))?;
    Ok(format!("“{}”, part {n} of {}:\n{p}", f.name, ps.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parts_cover_the_text_at_line_breaks() {
        let line = "a".repeat(99) + "\n";
        let t = line.repeat(250); // 25 000 chars
        let ps = parts(&t);
        assert_eq!(ps.concat(), t);
        assert_eq!(ps.len(), 4);
        assert!(ps.iter().all(|p| p.ends_with('\n') && p.chars().count() <= PART));
        assert_eq!(parts("").len(), 1);
        let long = "é".repeat(PART + 5);
        assert_eq!(parts(&long).concat(), long);
    }
}
