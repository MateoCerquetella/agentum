//! The editor's hub part: reads a file through agentd and highlights it with syntect, one HTML
//! string per line (each line reopens the scopes it starts inside, so any line can be replaced
//! alone). Classes are `sy-…`, styled by /api/code/syntax.css.
//!
//! - `open {machine, path}` → `{path, content, sha, size, lang, lines}` (`lines` null: plain)
//! - `highlight {path, content}` → `{lang, lines}`
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::machine::{FsReq, Req};
use serde_json::{json, Value};
use std::sync::OnceLock;
use syntect::html::{line_tokens_to_classed_spans, ClassStyle};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

const STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "sy-" };

fn syntaxes() -> &'static SyntaxSet {
    static S: OnceLock<SyntaxSet> = OnceLock::new();
    S.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// The grammar for a path (by name, extension or shebang). The default set has no TypeScript,
/// TOML…: close relatives stand in.
fn syntax_for<'a>(ss: &'a SyntaxSet, path: &str, first_line: &str) -> Option<&'a SyntaxReference> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    let by_name = match lower.as_str() {
        "makefile" | "gnumakefile" => Some("make"),
        "dockerfile" | "containerfile" | ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" | ".envrc" | "pkgbuild" => Some("sh"),
        "cmakelists.txt" => None,
        _ => None,
    };
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let ext = by_name.unwrap_or(match ext {
        "ts" | "tsx" | "mts" | "cts" | "jsx" | "mjs" | "cjs" => "js",
        "zsh" | "fish" | "bash" | "ksh" => "sh",
        "scss" | "sass" | "less" => "css",
        "vue" | "svelte" | "astro" | "htm" | "xhtml" => "html",
        "jsonc" | "json5" | "webmanifest" => "json",
        "hpp" | "hh" | "hxx" | "cc" | "cxx" | "ino" => "cpp",
        "kt" | "kts" => "java",
        "yml" => "yaml",
        "mdx" | "markdown" => "md",
        "svg" | "plist" | "xsd" | "csproj" => "xml",
        e => e,
    });
    let found = if ext.is_empty() { None } else { ss.find_syntax_by_extension(ext).or_else(|| ss.find_syntax_by_token(ext)) };
    found.or_else(|| ss.find_syntax_by_first_line(first_line)).filter(|s| s.name != "Plain Text")
}

fn open_span(out: &mut String, scope: Scope) {
    out.push_str("<span class=\"");
    for (i, atom) in scope.build_string().split('.').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str("sy-");
        out.push_str(atom);
    }
    out.push_str("\">");
}

/// Each line of `content` as HTML (without its newline), and the grammar's name. None: no
/// grammar for it, or too big.
pub fn highlight_lines(path: &str, content: &str) -> Option<(String, Vec<String>)> {
    if content.len() > super::MAX_HIGHLIGHT {
        return None;
    }
    let ss = syntaxes();
    let syn = syntax_for(ss, path, content.lines().next().unwrap_or(""))?;
    let mut state = ParseState::new(syn);
    let mut stack = ScopeStack::new();
    let mut out = Vec::with_capacity(content.len() / 32 + 1);
    for line in LinesWithEndings::from(content) {
        let mut html = String::with_capacity(line.len() * 2);
        for s in stack.as_slice() {
            open_span(&mut html, *s);
        }
        let ops = state.parse_line(line, ss).ok()?;
        let (body, _) = line_tokens_to_classed_spans(line, &ops, STYLE, &mut stack).ok()?;
        html.push_str(&body);
        // The line's own newline is the only one in it.
        if let Some(i) = html.rfind('\n') {
            html.remove(i);
            if i > 0 && html.as_bytes()[i - 1] == b'\r' {
                html.remove(i - 1);
            }
        }
        for _ in 0..stack.len() {
            html.push_str("</span>");
        }
        out.push(html);
    }
    Some((syn.name.clone(), out))
}

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    async fn rpc(&self, cx: &Ctx, method: &str, input: Value) -> Result<Value, String> {
        match method {
            "open" => {
                let machine = input["machine"].as_str().ok_or("machine?")?;
                let path = input["path"].as_str().ok_or("path?")?.to_string();
                let mut f = cx.machine(machine, Req::Fs(FsReq::Read { path: path.clone() })).await?;
                let content = f["content"].as_str().unwrap_or("").to_string();
                let p2 = path.clone();
                let hl = tokio::task::spawn_blocking(move || highlight_lines(&p2, &content)).await.ok().flatten();
                f["lang"] = json!(hl.as_ref().map(|h| h.0.clone()));
                f["lines"] = json!(hl.map(|h| h.1));
                Ok(f)
            }
            "highlight" => {
                let path = input["path"].as_str().ok_or("path?")?.to_string();
                let content = input["content"].as_str().ok_or("content?")?.to_string();
                let hl = tokio::task::spawn_blocking(move || highlight_lines(&path, &content)).await.ok().flatten();
                Ok(json!({"lang": hl.as_ref().map(|h| h.0.clone()), "lines": hl.map(|h| h.1)}))
            }
            _ => Err(format!("no method {method}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_stand_alone() {
        let src = "fn main() {\n    /* a\n    comment */ let x = \"<s>\";\r\n}\n";
        let (lang, lines) = highlight_lines("src/main.rs", src).unwrap();
        assert_eq!(lang, "Rust");
        assert_eq!(lines.len(), 4);
        for l in &lines {
            assert_eq!(l.matches("<span").count(), l.matches("</span>").count(), "{l}");
            assert!(!l.contains('\n') && !l.contains('\r'), "{l:?}");
        }
        // Line 3 starts inside the comment, so it reopens it.
        assert!(lines[2].starts_with("<span class=\"sy-source sy-rust\">"), "{}", lines[2]);
        assert!(lines[2].contains("sy-comment"), "{}", lines[2]);
        assert!(lines[2].contains("&lt;s&gt;"));
    }

    #[test]
    fn grammars_by_name() {
        let ss = syntaxes();
        assert_eq!(syntax_for(ss, "/x/app.tsx", "").map(|s| s.name.as_str()), Some("JavaScript"));
        assert_eq!(syntax_for(ss, "/x/Makefile", "").map(|s| s.name.as_str()), Some("Makefile"));
        assert_eq!(syntax_for(ss, "/x/run", "#!/bin/bash").map(|s| s.name.as_str()), Some("Bourne Again Shell (bash)"));
        assert!(syntax_for(ss, "/x/notes.txt", "hello").is_none());
        assert!(highlight_lines("/x/notes.txt", "hello").is_none());
    }
}
