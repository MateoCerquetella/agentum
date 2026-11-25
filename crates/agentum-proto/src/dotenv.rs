//! A small dotenv reader and editor, for the hub's and agentd's own env files and for projects'
//! `.env` (the Secrets tab).
//!
//! `KEY=value` per line; `export KEY=value` too; `#` starts a comment (a whole line, or after a
//! space in an unquoted value). `'single'` quotes are literal; `"double"` quotes know `\n \r \t \" \\`
//! and may span lines. No `${VAR}` expansion. A line without `=` is skipped.
use std::ops::Range;

/// One assignment: its key, its value, and the bytes it spans in the text (its whole line or
/// lines, newline included).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub key: String,
    pub value: String,
    pub span: Range<usize>,
}

/// Every assignment, in order (a key may repeat; the first one wins when loading).
pub fn entries(text: &str) -> Vec<Entry> {
    let b = text.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let eol = |from: usize| text[from..].find('\n').map(|n| from + n + 1).unwrap_or(b.len());
        // Leading blanks, then `export `.
        let mut j = i;
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        if text[j..].starts_with("export ") || text[j..].starts_with("export\t") {
            j += 7;
            while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
        }
        let line_end = eol(j);
        let line = &text[j..line_end];
        let Some(eq) = line.find('=') else {
            i = line_end;
            continue;
        };
        let key = line[..eq].trim();
        if key.is_empty() || key.starts_with('#') || key.contains(char::is_whitespace) {
            i = line_end;
            continue;
        }
        // The value: skip blanks after `=`.
        let mut k = j + eq + 1;
        while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
            k += 1;
        }
        let blank_before = k > j + eq + 1;
        let (value, end) = match b.get(k) {
            Some(&q @ (b'"' | b'\'')) => match quoted(text, k + 1, q) {
                Some((v, close)) => (v, eol(close)),
                // No closing quote: take the rest of the line as it is.
                None => (text[k..line_end].trim_end().to_string(), line_end),
            },
            _ => {
                let raw = text[k..line_end].trim_end_matches(['\n', '\r']);
                // ` #` starts a comment.
                let cut = raw.find(" #").or_else(|| raw.find("\t#")).unwrap_or(raw.len());
                let raw = if blank_before && raw.starts_with('#') { "" } else { &raw[..cut] };
                (raw.trim().to_string(), line_end)
            }
        };
        out.push(Entry { key: key.to_string(), value, span: start..end });
        i = end;
    }
    out
}

/// A quoted value from just after its opening quote: (the value, the index after the closing one).
fn quoted(text: &str, from: usize, q: u8) -> Option<(String, usize)> {
    let mut v = String::new();
    let mut it = text[from..].char_indices();
    while let Some((n, c)) = it.next() {
        if c as u32 == q as u32 {
            return Some((v, from + n + 1));
        }
        if c == '\\' && q == b'"' {
            match it.next() {
                Some((_, 'n')) => v.push('\n'),
                Some((_, 'r')) => v.push('\r'),
                Some((_, 't')) => v.push('\t'),
                Some((_, c @ ('"' | '\\' | '$'))) => v.push(c),
                Some((_, c)) => {
                    v.push('\\');
                    v.push(c);
                }
                None => return None,
            }
            continue;
        }
        v.push(c);
    }
    None
}

/// The assignments as (key, value), the first of each key.
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for e in entries(text) {
        if !out.iter().any(|(k, _)| *k == e.key) {
            out.push((e.key, e.value));
        }
    }
    out
}

/// The keys, each once, in order.
pub fn keys(text: &str) -> Vec<String> {
    parse(text).into_iter().map(|(k, _)| k).collect()
}

/// A key the Secrets tab may write: a shell variable name.
pub fn valid_key(k: &str) -> bool {
    let mut cs = k.chars();
    matches!(cs.next(), Some(c) if c == '_' || c.is_ascii_alphabetic()) && cs.all(|c| c == '_' || c.is_ascii_alphanumeric()) && k.len() <= 200
}

/// `value` as it should be written: bare when that is unambiguous, else quoted.
pub fn quote(value: &str) -> String {
    let bare = !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || "_-.,:/@+=%~^".contains(c));
    if bare {
        value.to_string()
    } else if !value.contains(['\'', '\n', '\r']) {
        format!("'{value}'")
    } else {
        let mut s = String::from("\"");
        for c in value.chars() {
            match c {
                '\n' => s.push_str("\\n"),
                '\r' => s.push_str("\\r"),
                '\t' => s.push_str("\\t"),
                '"' => s.push_str("\\\""),
                '\\' => s.push_str("\\\\"),
                '$' => s.push_str("\\$"),
                c => s.push(c),
            }
        }
        s.push('"');
        s
    }
}

/// The text with `key` set to `value`: its first assignment replaced in place (any later ones
/// dropped), or a new line at the end. Everything else is kept as it was.
pub fn set(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key}={}\n", quote(value));
    let mine: Vec<Entry> = entries(text).into_iter().filter(|e| e.key == key).collect();
    if mine.is_empty() {
        let mut s = text.to_string();
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&line);
        return s;
    }
    let mut s = String::with_capacity(text.len() + line.len());
    let mut at = 0;
    for (n, e) in mine.iter().enumerate() {
        s.push_str(&text[at..e.span.start]);
        if n == 0 {
            s.push_str(&line);
        }
        at = e.span.end;
    }
    s.push_str(&text[at..]);
    s
}

/// The text without any assignment of `key`.
pub fn remove(text: &str, key: &str) -> String {
    let mut s = String::with_capacity(text.len());
    let mut at = 0;
    for e in entries(text).into_iter().filter(|e| e.key == key) {
        s.push_str(&text[at..e.span.start]);
        at = e.span.end;
    }
    s.push_str(&text[at..]);
    s
}

/// Read a dotenv file into the process environment, leaving variables already set alone.
/// Returns the keys it set; a missing file sets none.
///
/// # Safety
/// It calls `std::env::set_var`: call it at the start of `main`, before any other thread exists.
pub unsafe fn load(path: &std::path::Path) -> std::io::Result<Vec<String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut set = vec![];
    for (k, v) in parse(&text) {
        if std::env::var_os(&k).is_none() {
            // SAFETY: the caller promises no other thread is running.
            unsafe { std::env::set_var(&k, v) };
            set.push(k);
        }
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let t = "# comment\n\nA=1\nexport B = two words \nC='single # not a comment'\nD=\"line\\nnext \\\"q\\\" \\\\ $x\"\nE=bare # comment\nF=\nG=\"multi\nline\"\nnot a line\n=nokey\nA=second\n  H=indented\r\nI='unterminated\n";
        let p = parse(t);
        let get = |k: &str| p.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
        assert_eq!(get("A"), Some("1"));
        assert_eq!(get("B"), Some("two words"));
        assert_eq!(get("C"), Some("single # not a comment"));
        assert_eq!(get("D"), Some("line\nnext \"q\" \\ $x"));
        assert_eq!(get("E"), Some("bare"));
        assert_eq!(get("F"), Some(""));
        assert_eq!(get("G"), Some("multi\nline"));
        assert_eq!(get("H"), Some("indented"));
        assert_eq!(get("I"), Some("'unterminated"));
        assert_eq!(keys(t), vec!["A", "B", "C", "D", "E", "F", "G", "H", "I"]);
        // Values with = and URLs stay whole.
        assert_eq!(parse("URL=postgres://u:p@h/db?x=1"), vec![("URL".into(), "postgres://u:p@h/db?x=1".into())]);
        assert_eq!(parse("X=a#b"), vec![("X".into(), "a#b".into())]);
    }

    #[test]
    fn quotes_round_trip() {
        for v in ["plain", "with space", "it's", "a\nb", "q\"uote", "back\\slash $HOME", "#hash", "", "tab\there"] {
            let t = format!("K={}\n", quote(v));
            assert_eq!(parse(&t), vec![("K".to_string(), v.to_string())], "{v:?} written as {t:?}");
        }
        assert_eq!(quote("sk-abc_123"), "sk-abc_123");
        assert_eq!(quote("two words"), "'two words'");
    }

    #[test]
    fn edits_keep_the_rest() {
        let t = "# keys\nA=1\nB=\"x\ny\"\nC=3 # note\nB=dup\n";
        assert_eq!(set(t, "B", "new"), "# keys\nA=1\nB=new\nC=3 # note\n");
        assert_eq!(set(t, "D", "four"), format!("{t}D=four\n"));
        assert_eq!(set("A=1", "B", "2"), "A=1\nB=2\n");
        assert_eq!(set("", "B", "2"), "B=2\n");
        assert_eq!(remove(t, "B"), "# keys\nA=1\nC=3 # note\n");
        assert_eq!(remove(t, "Z"), t);
        assert_eq!(set("export A=1\n", "A", "2"), "A=2\n");
    }

    #[test]
    fn keys_are_checked() {
        assert!(valid_key("OPENAI_API_KEY"));
        assert!(valid_key("_x1"));
        assert!(!valid_key("1X"));
        assert!(!valid_key("A-B"));
        assert!(!valid_key(""));
        assert!(!valid_key("A B"));
    }

    #[test]
    fn loads_without_overriding() {
        let p = std::env::temp_dir().join(format!("agentum-dotenv-{}", std::process::id()));
        std::fs::write(&p, "AGENTUM_DOTENV_TEST_NEW=1\nAGENTUM_DOTENV_TEST_SET=file\n").unwrap();
        // SAFETY: the test harness may run other tests in parallel, but none reads these names.
        unsafe {
            std::env::set_var("AGENTUM_DOTENV_TEST_SET", "env");
            assert_eq!(load(&p).unwrap(), vec!["AGENTUM_DOTENV_TEST_NEW"]);
            assert!(load(&p.with_extension("missing")).unwrap().is_empty());
        }
        assert_eq!(std::env::var("AGENTUM_DOTENV_TEST_NEW").as_deref(), Ok("1"));
        assert_eq!(std::env::var("AGENTUM_DOTENV_TEST_SET").as_deref(), Ok("env"));
        let _ = std::fs::remove_file(p);
    }
}
