//! Editor: open and edit a file from the thread's checkout (a tab in its side panel).
//!
//! The text is a plain `<textarea>` over a highlighted copy of it. The highlighting is done on
//! the hub (syntect, `open` and `highlight`), one HTML string per line, so the web app carries no
//! grammar: while you type, unchanged lines keep their colours and edited ones show plain until
//! the hub re-highlights them. Saving (⌘S) writes through agentd with the sha the file was read
//! at, so a file changed meanwhile isn't overwritten without asking.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "editor",
    name: "Editor",
    description: "Open and edit a file from the thread's checkout.",
    icon: "edit",
    group: Group::Code,
    default_on: true,
    settings: &[],
};

/// The side panel tab's id.
pub const TAB: &str = "editor";

/// Files bigger than this are shown without colours.
pub const MAX_HIGHLIGHT: usize = 512 * 1024;

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            c => o.push(c),
        }
    }
    o
}

/// The highlighted copy of `text`: lines unchanged since `hl` was made (by position from the
/// start and from the end) keep their HTML, the rest are plain. Ends with a newline so the last
/// line has its height.
pub fn render(text: &str, hl: &[(String, String)]) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let (n, m) = (lines.len(), hl.len());
    let mut p = 0;
    while p < n && p < m && hl[p].0 == lines[p] {
        p += 1;
    }
    let mut s = 0;
    while s < n - p && s < m - p && hl[m - 1 - s].0 == lines[n - 1 - s] {
        s += 1;
    }
    let mut out = String::with_capacity(text.len() * 2 + 16);
    for (i, l) in lines.iter().enumerate() {
        if i < p {
            out.push_str(&hl[i].1);
        } else if i >= n - s {
            out.push_str(&hl[m - (n - i)].1);
        } else {
            out.push_str(&esc(l));
        }
        out.push('\n');
    }
    out
}

/// What Tab inserts: a tab if the file indents with tabs, else its smallest space indent (2 or 4).
pub fn indent_unit(text: &str) -> String {
    let mut min = usize::MAX;
    for l in text.lines().take(400) {
        if l.starts_with('\t') {
            return "\t".into();
        }
        let n = l.len() - l.trim_start_matches(' ').len();
        if n > 0 && l.len() > n && n < min {
            min = n;
        }
    }
    " ".repeat(if min == 2 { 2 } else { 4 })
}

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;
#[cfg(feature = "hub")]
pub use hub::highlight_lines;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::{active, open, PLUGIN as WEB};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_keeps_unchanged_lines() {
        let hl: Vec<(String, String)> = ["a", "b", "c"].iter().map(|l| (l.to_string(), format!("<i>{l}</i>"))).collect();
        assert_eq!(render("a\nb\nc", &hl), "<i>a</i>\n<i>b</i>\n<i>c</i>\n");
        // A line inserted in the middle: the ones after it keep their colours too.
        assert_eq!(render("a\nx<\nb\nc", &hl), "<i>a</i>\nx&lt;\n<i>b</i>\n<i>c</i>\n");
        assert_eq!(render("a\nB\nc", &hl), "<i>a</i>\nB\n<i>c</i>\n");
        assert_eq!(render("", &[]), "\n");
        assert_eq!(render("c", &hl), "<i>c</i>\n");
    }

    #[test]
    fn indents() {
        assert_eq!(indent_unit("fn a() {\n\tx\n}"), "\t");
        assert_eq!(indent_unit("a:\n  b:\n    c"), "  ");
        assert_eq!(indent_unit("x"), "    ");
    }
}
