//! Writing tools for Mail's composer (cerq-mail's writer.py): your model
//! works on the text you wrote, or the part you selected, and the answer
//! streams back as NDJSON frames. Nothing is sent or saved here.
//!   write: notes → a complete email ("Subject: …" first when there is none)
//!   proofread, rewrite, friendly, professional, concise
//!   english / spanish: translate (spanish = the second language in Settings)
use crate::{llm, state::App};
use futures::{Stream, StreamExt};
use regex::Regex;
use serde_json::{json, Value};
use std::{sync::LazyLock, time::Duration};

const RULES: &str = "Output ONLY the resulting text: no preface, no explanation, no quotes around it, no markdown. \
Never invent facts, experience, numbers, links, dates or promises that aren't in the text.";

pub const FAILED: &str = "Writing failed before completion. Your draft was kept.";

fn mode_task(mode: &str, lang2_desc: &str) -> Option<String> {
    Some(match mode {
        "write" => "Turn the user's notes into a complete, ready-to-send email. Write it in the same language as the notes \
(Spanish notes → a Spanish email), unless the notes ask for another language. \
Keep every fact, name, number, date, question and request from the notes, in a natural order. \
Open with a greeting to the recipient and close with a short sign-off{sign}. \
Keep it as short as the notes allow.".into(),
        "proofread" => "Fix spelling, grammar, accents and punctuation only. Keep the wording, tone, language and line breaks.".into(),
        "rewrite" => "Rewrite it so it reads clearly and naturally. Same meaning, same language, about the same length.".into(),
        "friendly" => "Rewrite it warmer and friendlier. Same meaning and language.".into(),
        "professional" => "Rewrite it more professional and polished. Same meaning and language.".into(),
        "concise" => "Make it shorter. Keep every request, question, fact and date; drop the rest. Same language.".into(),
        "english" => "Translate it to natural English. Keep names, numbers and line breaks.".into(),
        "spanish" => format!("Translate it to {lang2_desc}. Keep names, numbers and line breaks."),
        _ => return None,
    })
}

static ES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)[ñ¿¡áéíóú]|\b(que|qué|por|para|pero|como|cómo|cuanto|cuánto|gracias|hola|avisame|avísame|vos|está|estoy|una|del|los|las|el|la|me|te|mi|adjunto|saludos|abrazo|quedo)\b").unwrap());
static EN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(the|and|to|you|your|is|are|for|with|thanks|hi|hello|please|would|could|attached|regards|best|my|me|i)\b").unwrap());
static ASK_EN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(in english|en ingl[eé]s)\b").unwrap());
static ASK_ES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(in spanish|en espa[nñ]ol|en castellano)\b").unwrap());

/// "Spanish (Argentina)" → ("Spanish", "natural Spanish as spoken in Argentina").
fn lang2(setting: &str) -> (String, String) {
    let l = if setting.trim().is_empty() { "Spanish" } else { setting.trim() };
    let (base, region) = l.split_once(" (").map(|(b, r)| (b.to_string(), r.trim_end_matches(')').to_string())).unwrap_or((l.to_string(), String::new()));
    let desc = if region.is_empty() { format!("natural {base}") } else { format!("natural {base} as spoken in {region}") };
    (base, desc)
}

/// Spanish or English, from the words (the model alone drifts to the
/// recipient's language). Asked for in the notes wins. "" = leave it to the model.
pub fn language(text: &str, base2: &str) -> &'static str {
    if ASK_EN.is_match(text) { return "English"; }
    if ASK_ES.is_match(text) { return "Spanish"; }
    let (es, en) = (ES.find_iter(text).count(), EN.find_iter(text).count());
    if base2 == "Spanish" { return if es >= en { "Spanish" } else { "English" }; }
    if en > es && en >= 2 { "English" } else { "" }
}

/// The chat messages and token budget for a request, or why it can't run.
pub fn messages(d: &Value, me: &str, lang2_setting: &str) -> Result<(Vec<Value>, u32), String> {
    let mode = d["mode"].as_str().unwrap_or("");
    let (base2, desc2) = lang2(lang2_setting);
    let mut task = mode_task(mode, &desc2).ok_or("unknown mode")?;
    let text: String = d["text"].as_str().unwrap_or("").trim().chars().take(8000).collect();
    if text.is_empty() { return Err("nothing to work on".into()); }
    let me = me.trim();
    task = task.replace("{sign}", &if me.is_empty() { String::new() } else { format!(", signed {me}") });
    if mode != "english" && mode != "spanish" {
        let lang = language(&text, &base2);
        task += &if lang.is_empty() { " Write your answer in the same language as the text, whatever the recipient's language.".to_string() }
            else { format!(" The text is in {lang}: write your answer in {lang}, whatever the recipient's language.") };
    }
    let field = |k: &str| d[k].as_str().map(str::trim).unwrap_or("").to_string();
    if mode == "write" && field("subject").is_empty() {
        task += " The first line is \"Subject: \" and a short subject, in the email's language, that says what the email is about (no names of who referred them), then an empty line, then the email.";
    }
    let mut facts = vec![];
    let to = d["to"].as_str().unwrap_or("");
    if !to.is_empty() { facts.push(format!("To: {}", to.chars().take(200).collect::<String>())); }
    let subject = d["subject"].as_str().unwrap_or("");
    if !subject.is_empty() { facts.push(format!("Subject: {}", subject.chars().take(200).collect::<String>())); }
    let ctx = d["context"].as_str().unwrap_or("");
    if !ctx.is_empty() { facts.push(format!("The email being answered:\n{}", ctx.chars().take(2500).collect::<String>())); }
    let head = if facts.is_empty() { String::new() } else { facts.join("\n") + "\n\n" };
    let label = if mode == "write" { "Notes" } else { "Text" };
    let budget = if mode == "write" { 900 } else { (200 + text.chars().count() / 2).min(1600) as u32 };
    Ok((vec![json!({"role": "system", "content": format!("You help the user write emails. {task} {RULES}")}),
             json!({"role": "user", "content": format!("{head}{label}:\n{text}")})], budget))
}

static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(```\w*\s*)?").unwrap());
fn clean_start(s: &str) -> String { FENCE.replace(s, "").into_owned() }

fn frame(v: Value) -> String { format!("{v}\n") }

/// NDJSON frames: chunk… then done, or error (a stream that stopped early is
/// an error, never a silently short text).
pub fn frames(app: App, msgs: Vec<Value>, budget: u32) -> impl Stream<Item = String> + Send + 'static {
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);
    tokio::spawn(async move {
        let ok = write(&app, msgs, budget, &tx).await;
        let _ = tx.send(frame(if ok { json!({"type": "done"}) } else { json!({"type": "error", "error": FAILED}) })).await;
    });
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

/// Stream from your model; if nothing started, one answer in one go. True
/// when text was written to the end. A closed client stops the model too.
async fn write(app: &App, msgs: Vec<Value>, budget: u32, tx: &tokio::sync::mpsc::Sender<String>) -> bool {
    let body = json!({"messages": msgs.clone(), "options": {"temperature": 0.2, "num_predict": budget}});
    let mut started = false;
    if let Ok(mut st) = llm::stream(app, body, llm::Prio::Fg).await {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        let mut finished = false;
        loop {
            let next = match tokio::time::timeout_at(deadline, st.next()).await { Ok(n) => n, Err(_) => break };
            let Some(Ok(chunk)) = next else { break };
            if chunk.get("error").is_some() { break; }
            finished |= chunk["done"] == true;
            let mut piece = chunk["message"]["content"].as_str().unwrap_or("").to_string();
            if !started {
                piece = clean_start(&piece);
                if piece.is_empty() { if finished { break } else { continue } }
                started = true;
            }
            if !piece.is_empty() && tx.send(frame(json!({"type": "chunk", "text": piece}))).await.is_err() { return false; }
            if finished { break; }
        }
        if started { return finished; }
    }
    let Some(out) = llm::complete(app, msgs, budget, llm::Prio::Fg).await else { return false };
    let text = clean_start(&out).trim_end_matches(['`', '\n', ' ']).to_string();
    if text.trim().is_empty() { return false; }
    tx.send(frame(json!({"type": "chunk", "text": text}))).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn language_detection() {
        assert_eq!(language("hola, te mando el adjunto", "Spanish"), "Spanish");
        assert_eq!(language("hi, please find the file attached", "Spanish"), "English");
        assert_eq!(language("hi, write it en español please", "Spanish"), "Spanish");
        assert_eq!(language("hola, in english please", "Spanish"), "English");
        assert_eq!(language("bonjour", "French"), "");
        assert_eq!(language("hi, the file is attached", "French"), "English");
    }

    #[test]
    fn prompts() {
        let (m, budget) = messages(&json!({"mode": "write", "text": "decile a Ana que llego tarde"}), "Mateo", "Spanish (Argentina)").unwrap();
        let sys = m[0]["content"].as_str().unwrap();
        assert!(sys.contains("short sign-off, signed Mateo."));
        assert!(sys.contains("The text is in Spanish"));
        assert!(sys.contains("The first line is \"Subject: \""));
        assert_eq!(m[1]["content"], "Notes:\ndecile a Ana que llego tarde");
        assert_eq!(budget, 900);
        let (m, budget) = messages(&json!({"mode": "spanish", "text": "See you", "subject": "Hi", "to": "a@b.c", "context": "Ctx"}), "", "Spanish (Argentina)").unwrap();
        assert!(m[0]["content"].as_str().unwrap().contains("Translate it to natural Spanish as spoken in Argentina."));
        assert!(!m[0]["content"].as_str().unwrap().contains("The text is in"));
        assert_eq!(m[1]["content"], "To: a@b.c\nSubject: Hi\nThe email being answered:\nCtx\n\nText:\nSee you");
        assert_eq!(budget, 203);
        assert!(messages(&json!({"mode": "poem", "text": "x"}), "", "").is_err());
        assert!(messages(&json!({"mode": "rewrite", "text": "  "}), "", "").is_err());
        assert_eq!(clean_start("```text\nHello"), "Hello");
    }
}
