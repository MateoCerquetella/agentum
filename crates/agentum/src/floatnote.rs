//! Notes as floatnote's files (github.com/MateoCerquetella/linux-tools,
//! floatnote/): plain Markdown files in CERQ_NOTES_DIR (default
//! <data>/notes), kept in step with the desktop's ~/Documents/Notes by
//! Syncthing. Same rules as floatnote: the id is the file name without .md,
//! a new note is named after the local time, the title is the first line with
//! text, deleting moves to .deleted/ (empty notes are just removed), writes
//! are atomic. A write that carries the mtime it started from is refused
//! (409, with the note as it is now) when the file changed elsewhere since.
use crate::{state::App, util::*};
use axum::{extract::{Path, Query, State}, http::StatusCode, response::{IntoResponse, Response}, routing::get, Json, Router};
use serde_json::{json, Value};
use std::{fs, path::{Path as FsPath, PathBuf}, sync::LazyLock, time::UNIX_EPOCH};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/floatnotes", get(list).post(create))
        .route("/api/floatnotes/{id}", get(read).put(write).delete(remove))
        .route("/api/floatnotes/{id}/suggest", axum::routing::post(suggest))
        .route("/api/floatnotes/{id}/tidy", axum::routing::post(tidy))
}

pub fn dir(app: &App) -> PathBuf {
    std::env::var("CERQ_NOTES_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(&app.cfg.data_dir).join("notes"))
}

/// floatnote's id rule: letters, digits, dot, underscore, space, dash; not hidden.
fn valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && !id.starts_with('.') && id.chars().all(|c| c.is_ascii_alphanumeric() || "._ -".contains(c))
}
fn path(d: &FsPath, id: &str) -> Option<PathBuf> { valid(id).then(|| d.join(format!("{id}.md"))) }
fn mtime(p: &FsPath) -> f64 { fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0) }
fn changed(p: &FsPath, base: f64) -> bool { (mtime(p) - base).abs() > 0.000001 }

/// The first line with text, without Markdown marks (floatnote's title_of).
pub fn title_of(text: &str) -> String {
    static LEAD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\s*(#{1,6}\s+|[-*+]\s+(\[[ xX]\]\s+)?|\d+[.)]\s+|>\s*)").unwrap());
    static MARKS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"[*_~`]").unwrap());
    for line in text.lines() {
        let l = MARKS.replace_all(LEAD.replace(line, "").trim(), "").trim().to_string();
        if !l.is_empty() { return l.chars().take(120).collect(); }
    }
    String::new()
}

pub fn list_in(d: &FsPath) -> Vec<Value> {
    let mut out: Vec<Value> = fs::read_dir(d).into_iter().flatten().flatten().filter_map(|e| {
        let p = e.path();
        let id = p.file_name()?.to_str()?.strip_suffix(".md")?.to_string();
        if !valid(&id) { return None; }
        let text = fs::read_to_string(&p).ok()?;
        let body: String = text.chars().take(4000).collect();
        Some(json!({"id": id, "title": title_of(&body), "body": body, "mtime": mtime(&p)}))
    }).collect();
    out.sort_by(|a, b| b["mtime"].as_f64().partial_cmp(&a["mtime"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Notes matching every word of `q` (title counts triple), best first, with the line that matched.
pub fn search_in(d: &FsPath, q: &str, limit: usize) -> Vec<Value> {
    let words: Vec<String> = q.to_lowercase().split_whitespace().filter(|w| w.len() > 1).map(String::from).collect();
    let mut hits: Vec<(usize, Value)> = list_in(d).into_iter().filter_map(|n| {
        let (title, body) = (s(&n, "title").to_lowercase(), s(&n, "body").to_lowercase());
        if !words.iter().all(|w| title.contains(w.as_str()) || body.contains(w.as_str())) { return None; }
        let score = words.iter().map(|w| title.matches(w.as_str()).count() * 3 + body.matches(w.as_str()).count()).sum::<usize>();
        let line = s(&n, "body").lines().find(|l| words.iter().any(|w| l.to_lowercase().contains(w.as_str()))).unwrap_or("").chars().take(160).collect::<String>();
        Some((score, json!({"id": n["id"], "title": n["title"], "mtime": n["mtime"], "line": line})))
    }).collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter().take(limit).map(|(_, v)| v).collect()
}

pub fn read_in(d: &FsPath, id: &str) -> Option<Value> {
    let p = path(d, id)?;
    Some(match fs::read_to_string(&p) { Ok(md) => json!({"id": id, "md": md, "mtime": mtime(&p)}), Err(_) => json!({"id": id, "md": "", "mtime": 0.0}) })
}

/// Ok(new mtime), or Err(the note as it is now) when it changed since `base`.
pub fn write_in(d: &FsPath, id: &str, md: &str, base: Option<f64>) -> anyhow::Result<Result<f64, Value>> {
    let p = path(d, id).ok_or_else(|| anyhow::anyhow!("bad note id"))?;
    if base.is_some_and(|base| changed(&p, base)) { return Ok(Err(read_in(d, id).unwrap_or(Value::Null))); }
    fs::create_dir_all(d)?;
    let tmp = d.join(format!(".{id}.md.tmp"));
    fs::write(&tmp, md)?;
    fs::rename(&tmp, &p)?;
    Ok(Ok(mtime(&p)))
}

/// A new note named after the local time ("2026-11-20 15.04.05", then " 2", " 3"…).
pub fn create_in(d: &FsPath, stamp: &str, md: &str) -> anyhow::Result<String> {
    fs::create_dir_all(d)?;
    let (mut id, mut n) = (stamp.to_string(), 1);
    while d.join(format!("{id}.md")).exists() { n += 1; id = format!("{stamp} {n}"); }
    write_in(d, &id, md, None)?.map_err(|_| anyhow::anyhow!("note changed"))?;
    Ok(id)
}

/// To Recently Deleted (.deleted/, floatnote keeps it 30 days); empty notes are just removed.
pub fn delete_in(d: &FsPath, id: &str) -> anyhow::Result<()> {
    let p = path(d, id).ok_or_else(|| anyhow::anyhow!("bad note id"))?;
    let Ok(text) = fs::read_to_string(&p) else { return Ok(()) };
    if text.trim().is_empty() { fs::remove_file(&p)?; return Ok(()); }
    let del = d.join(".deleted");
    fs::create_dir_all(&del)?;
    let (mut dest, mut n) = (del.join(format!("{id}.md")), 1);
    while dest.exists() { n += 1; dest = del.join(format!("{id} ({n}).md")); }
    fs::rename(&p, &dest)?;
    fs::File::options().append(true).open(&dest)?.set_modified(std::time::SystemTime::now())?; // mtime = deletion time
    Ok(())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> anyhow::Result<T> { Ok(tokio::task::spawn_blocking(f).await?) }

async fn list(State(app): State<App>) -> R<Json<Value>> {
    let d = dir(&app);
    Ok(Json(json!(blocking(move || list_in(&d)).await?)))
}
async fn read(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> {
    let d = dir(&app);
    blocking(move || read_in(&d, &id)).await?.map(Json).ok_or_else(|| bad("bad note id"))
}
async fn create(State(app): State<App>, Json(b): Json<Value>) -> R<Json<Value>> {
    let (d, md) = (dir(&app), s(&b, "md"));
    let stamp = crate::today::now(&app).format("%Y-%m-%d %H.%M.%S").to_string();
    let id = blocking(move || create_in(&d, &stamp, &md)).await??;
    app.emit(json!({"kind": "notes"}));
    let d = dir(&app);
    Ok(Json(blocking(move || read_in(&d, &id)).await?.unwrap_or_default()))
}
async fn write(State(app): State<App>, Path(id): Path<String>, Json(b): Json<Value>) -> R<Response> {
    if !valid(&id) { return Err(bad("bad note id")); }
    let (d, md, base) = (dir(&app), s(&b, "md"), b["mtime"].as_f64());
    match blocking(move || write_in(&d, &id, &md, base)).await?? {
        Ok(m) => { app.emit(json!({"kind": "notes"})); Ok(Json(json!({"mtime": m})).into_response()) }
        Err(now) => Ok((StatusCode::CONFLICT, Json(json!({"error": "This note changed on another device.", "note": now}))).into_response()),
    }
}
#[derive(serde::Deserialize)]
struct DeleteQuery { mtime: Option<f64> }

async fn remove(State(app): State<App>, Path(id): Path<String>, Query(query): Query<DeleteQuery>) -> R<Json<Value>> {
    let d = dir(&app);
    let p = path(&d, &id).ok_or_else(|| bad("bad note id"))?;
    let removed = blocking(move || {
        if query.mtime.is_some_and(|base| changed(&p, base)) { return Ok::<_, anyhow::Error>(false); }
        delete_in(&d, &id)?;
        Ok(true)
    }).await??;
    if !removed { return Err(AppError(StatusCode::CONFLICT, "This note changed on another device.".into())); }
    app.emit(json!({"kind": "notes"}));
    Ok(Json(json!({"ok": true})))
}

/// The note as the model should see it: floatnote's Markdown escapes removed, at most `max` characters.
fn note_text(app: &App, id: &str, max: usize) -> R<(String, f64)> {
    let n = read_in(&dir(app), id).ok_or_else(|| bad("bad note id"))?;
    let md = n["md"].as_str().unwrap_or("").to_string();
    if md.trim().is_empty() { return Err(bad("This note is empty.")); }
    if md.chars().count() > max { return Err(bad(format!("This note is too long for your model (over {max} characters)."))); }
    Ok((md, n["mtime"].as_f64().unwrap_or(0.0)))
}

/// To-dos and events written in a note, found by your model: [{kind, title, date, time, end, line}].
/// Only what's still to do (not ticked, not past); you add each one yourself.
async fn suggest(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> {
    let (md, _) = note_text(&app, &id, 8000)?;
    let today = crate::today::now(&app);
    // Small models get weekdays wrong: spell out the coming week.
    let days = (1..=7).map(|i| (today.date_naive() + chrono::Duration::days(i)).format("%A %Y-%m-%d").to_string()).collect::<Vec<_>>().join(", ");
    let sys = format!("You find what the user still has to do in one of their notes. Today is {} ({}). A weekday name means the nearest one of these: {days} (\"next\" before it adds a week). \
Return only a JSON array; each item: {{\"kind\": \"todo\" or \"event\", \"title\": short imperative in the note's language, \"date\": \"YYYY-MM-DD\" or null, \"time\": \"HH:MM\" or null, \"end\": \"HH:MM\" or null, \"line\": the note's line it comes from}}. \
An event happens at a time with other people or at a place (a meeting, an appointment, a trip); everything else to do is a todo. \
Resolve relative dates (friday, mañana, next week) from today. Skip ticked items ([x]), things already done or past, plain facts, ideas and secrets (keys, tokens, passwords). At most 12 items. Nothing to do: [].",
        today.format("%Y-%m-%d"), today.format("%A"));
    let out = crate::llm::complete_long(&app, vec![json!({"role": "system", "content": sys}), json!({"role": "user", "content": md})], 900, crate::llm::Prio::Fg).await;
    let Some(out) = out else {
        // No model: the unticked checklist items, as plain to-dos.
        let items: Vec<Value> = md.lines().filter_map(|l| l.trim_start().strip_prefix("- [ ] ").or_else(|| l.trim_start().strip_prefix("* [ ] ")))
            .map(|t| json!({"kind": "todo", "title": t.replace('\\', "").trim(), "date": null, "time": null, "end": null, "line": t})).take(12).collect();
        return Ok(Json(json!({"items": items, "by_model": false})));
    };
    let raw = crate::llm::extract_json(&out).or_else(|| crate::llm::repair_json(&out)).unwrap_or(json!([]));
    let date = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap();
    let time = regex::Regex::new(r"^\d{2}:\d{2}$").unwrap();
    let pick = |v: &Value, re: &regex::Regex| v.as_str().filter(|x| re.is_match(x)).map(|x| json!(x)).unwrap_or(Value::Null);
    let items: Vec<Value> = raw.as_array().cloned().unwrap_or_default().iter().filter_map(|x| {
        let title: String = s(x, "title").trim().chars().take(120).collect();
        if title.is_empty() { return None; }
        let d = pick(&x["date"], &date);
        let kind = if s(x, "kind") == "event" && !d.is_null() { "event" } else { "todo" };
        Some(json!({"kind": kind, "title": title, "date": d, "time": pick(&x["time"], &time), "end": pick(&x["end"], &time), "line": s(x, "line").chars().take(200).collect::<String>()}))
    }).take(12).collect();
    Ok(Json(json!({"items": items, "by_model": true})))
}

/// Your model's tidier version of the note (structure, typos; same facts, links and ticks). Nothing is saved: you accept it.
async fn tidy(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> {
    let (md, mtime) = note_text(&app, &id, 6000)?;
    let sys = "You tidy one Markdown note. Keep every fact, number, name, link and checklist state ([ ] / [x]) exactly; keep its language; add nothing new. \
Make the first line a short title, group related lines under headings when it helps, turn lists of things to do into checklists (- [ ] ), fix typos and broken formatting, drop duplicate lines. \
Return only the note's Markdown, no explanations, no code fences.";
    let max = ((md.len() / 3) as u32 + 200).clamp(300, 2400);
    let out = crate::llm::complete_long(&app, vec![json!({"role": "system", "content": sys}), json!({"role": "user", "content": md.clone()})], max, crate::llm::Prio::Fg).await
        .ok_or_else(|| AppError(StatusCode::SERVICE_UNAVAILABLE, "Your model didn't answer.".into()))?;
    let new = out.trim().trim_start_matches("```markdown").trim_start_matches("```md").trim_start_matches("```").trim_end_matches("```").trim().to_string();
    // A tidy that lost a lot is a broken answer, not a tidier note.
    if new.len() * 10 < md.trim().len() * 6 { return Err(AppError(StatusCode::UNPROCESSABLE_ENTITY, "Your model's version dropped too much of the note; nothing changed.".into())); }
    Ok(Json(json!({"md": new, "mtime": mtime})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floatnote_rules() {
        let d = std::env::temp_dir().join(format!("cerq-notes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        assert_eq!(title_of("\n## Groceries\n- [ ] milk"), "Groceries");
        assert_eq!(title_of("- [x] **done** thing"), "done thing");
        assert!(!valid("../etc/passwd") && !valid(".deleted") && !valid("a/b") && valid("2026-11-20 15.04.05 2"));

        let a = create_in(&d, "2026-11-20 15.04.05", "# One\nbody").unwrap();
        let b = create_in(&d, "2026-11-20 15.04.05", "Two").unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("2026-11-20 15.04.05", "2026-11-20 15.04.05 2"));
        let l = list_in(&d);
        assert_eq!(l.len(), 2);
        assert_eq!(l.iter().find(|n| n["id"] == a.as_str()).unwrap()["title"], "One");

        // A write from a stale copy is refused and gets the note as it is now.
        let base = read_in(&d, &a).unwrap()["mtime"].as_f64().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let m2 = write_in(&d, &a, "# One\nedited elsewhere", None).unwrap().unwrap();
        let stale = write_in(&d, &a, "# One\nmine", Some(base)).unwrap();
        assert_eq!(stale.unwrap_err()["md"], "# One\nedited elsewhere");
        assert!(write_in(&d, &a, "# One\nmine", Some(m2)).unwrap().is_ok());
        assert_eq!(read_in(&d, &a).unwrap()["md"], "# One\nmine");

        // Deleting keeps a copy in .deleted/; a second one with the same name gets " (2)".
        delete_in(&d, &a).unwrap();
        assert!(read_in(&d, &a).unwrap()["mtime"] == 0.0 && d.join(".deleted").join(format!("{a}.md")).exists());
        assert!(write_in(&d, &a, "stale draft", Some(m2)).unwrap().is_err());
        create_in(&d, "2026-11-20 15.04.05", "again").unwrap();
        delete_in(&d, &a).unwrap();
        assert!(d.join(".deleted").join(format!("{a} (2).md")).exists());
        // An empty note is just removed; hidden files never list.
        write_in(&d, &b, "  \n", None).unwrap().unwrap();
        delete_in(&d, &b).unwrap();
        assert_eq!(list_in(&d).len(), 0);
        assert!(read_in(&d, "../x").is_none());
        // Search: every word must match; the title weighs more.
        create_in(&d, "2026-11-21 10.00.00", "Trip to Mendoza\nbook the bus").unwrap();
        create_in(&d, "2026-11-21 10.00.01", "Groceries\nmendoza wine for the trip").unwrap();
        let hits = search_in(&d, "Mendoza trip", 5);
        assert_eq!(hits.iter().map(|h| s(h, "title")).collect::<Vec<_>>(), ["Trip to Mendoza", "Groceries"]);
        assert_eq!(s(&hits[1], "line"), "mendoza wine for the trip");
        assert!(search_in(&d, "mendoza dentist", 5).is_empty());
        fs::remove_dir_all(&d).unwrap();
    }
}
