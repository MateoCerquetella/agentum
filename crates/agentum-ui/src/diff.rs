//! Unified diffs (`git diff`, `git show`): parsed into files, hunks and lines, and drawn
//! unified or side by side. A file's body is drawn as one HTML string (big diffs stay fast and
//! the wasm small); styles are `.dv*` in style/code/diff.css.
use crate::icons::Icon;
use leptos::prelude::*;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Modified,
    Added,
    Deleted,
    Renamed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Ctx,
    Add,
    Del,
    /// "\ No newline at end of file"
    NoNewline,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub kind: LineKind,
    /// Line numbers in the old and new file (0: none).
    pub old: u32,
    pub new: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hunk {
    /// The whole `@@ -a,b +c,d @@ context` line.
    pub header: String,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileDiff {
    /// The new path (the old one for a deleted file).
    pub path: String,
    /// The old path, when renamed.
    pub old_path: Option<String>,
    pub kind: FileKind,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    pub add: u32,
    pub del: u32,
}

impl FileDiff {
    fn new(path: String) -> FileDiff {
        FileDiff { path, old_path: None, kind: FileKind::Modified, binary: false, hunks: vec![], add: 0, del: 0 }
    }
    pub fn lines(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len() + 1).sum()
    }
    /// "M", "A", "D", "R".
    pub fn letter(&self) -> &'static str {
        match self.kind {
            FileKind::Modified => "M",
            FileKind::Added => "A",
            FileKind::Deleted => "D",
            FileKind::Renamed => "R",
        }
    }
}

/// A path from a `---`/`+++`/`diff --git` line: unquoted, without its `a/` `b/` prefix.
fn clean_path(p: &str) -> String {
    let p = p.trim_end_matches('\t').trim();
    let p = if p.len() >= 2 && p.starts_with('"') && p.ends_with('"') { unquote(&p[1..p.len() - 1]) } else { p.to_string() };
    p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).map(str::to_string).unwrap_or(p)
}

/// git's C-style quoting (`\t`, `\"`, `\\`, octal bytes for non-ASCII).
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' || i + 1 >= b.len() {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let oct = b.get(i + 1..i + 4).filter(|o| o.iter().all(|c| (b'0'..=b'7').contains(c)));
        if let Some(v) = oct.and_then(|o| u8::from_str_radix(std::str::from_utf8(o).ok()?, 8).ok()) {
            out.push(v);
            i += 4;
            continue;
        }
        out.push(match b[i + 1] {
            b'n' => b'\n',
            b't' => b'\t',
            b'r' => b'\r',
            other => other,
        });
        i += 2;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `@@ -a,b +c,d @@`: (a, b, c, d); a missing count is 1.
fn hunk_range(h: &str) -> Option<(u32, u32, u32, u32)> {
    let inner = h.strip_prefix("@@ ")?;
    let end = inner.find(" @@")?;
    let mut it = inner[..end].split(' ');
    let one = |s: &str| -> Option<(u32, u32)> {
        let (a, b) = match s.split_once(',') {
            Some((a, b)) => (a, b.parse().ok()?),
            None => (s, 1),
        };
        Some((a.parse().ok()?, b))
    };
    let (a, b) = one(it.next()?.strip_prefix('-')?)?;
    let (c, d) = one(it.next()?.strip_prefix('+')?)?;
    Some((a, b, c, d))
}

/// Parse a unified diff (several files).
pub fn parse(diff: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = vec![];
    let mut cur: Option<FileDiff> = None;
    // Lines still expected in the current hunk (old side, new side).
    let (mut left_old, mut left_new) = (0u32, 0u32);
    let (mut ln_old, mut ln_new) = (0u32, 0u32);
    for line in diff.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if left_old > 0 || left_new > 0 {
            let f = cur.as_mut().unwrap();
            let h = f.hunks.last_mut().unwrap();
            match line.as_bytes().first() {
                Some(b'+') => {
                    h.lines.push(Line { kind: LineKind::Add, old: 0, new: ln_new, text: line[1..].to_string() });
                    ln_new += 1;
                    left_new = left_new.saturating_sub(1);
                    f.add += 1;
                    continue;
                }
                Some(b'-') => {
                    h.lines.push(Line { kind: LineKind::Del, old: ln_old, new: 0, text: line[1..].to_string() });
                    ln_old += 1;
                    left_old = left_old.saturating_sub(1);
                    f.del += 1;
                    continue;
                }
                // Some tools drop the space of an empty context line.
                Some(b' ') | None => {
                    h.lines.push(Line { kind: LineKind::Ctx, old: ln_old, new: ln_new, text: line.get(1..).unwrap_or("").to_string() });
                    ln_old += 1;
                    ln_new += 1;
                    left_old = left_old.saturating_sub(1);
                    left_new = left_new.saturating_sub(1);
                    continue;
                }
                Some(b'\\') => {
                    h.lines.push(Line { kind: LineKind::NoNewline, old: 0, new: 0, text: line.to_string() });
                    continue;
                }
                // The hunk was shorter than it said: read this line as a header.
                _ => (left_old, left_new) = (0, 0),
            }
        }
        // "\ No newline" right after a hunk's last line.
        if line.starts_with("\\ ") {
            if let Some(h) = cur.as_mut().and_then(|f| f.hunks.last_mut()) {
                h.lines.push(Line { kind: LineKind::NoNewline, old: 0, new: 0, text: line.to_string() });
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            files.extend(cur.take());
            // "a/x b/x": both halves are the same path unless renamed (then ---/+++ or rename lines tell).
            let path = if rest.len() % 2 == 1 && rest[..rest.len() / 2] == rest[rest.len() / 2 + 1..].replacen("b/", "a/", 1) {
                clean_path(&rest[..rest.len() / 2])
            } else {
                rest.split_once(" b/").map(|(_, b)| b.to_string()).unwrap_or_else(|| clean_path(rest))
            };
            cur = Some(FileDiff::new(path));
            continue;
        }
        if let Some(h) = line.strip_prefix("@@ ").map(|_| line) {
            let Some((a, b, c, d)) = hunk_range(h) else { continue };
            let f = cur.get_or_insert_with(|| FileDiff::new(String::new()));
            f.hunks.push(Hunk { header: h.to_string(), lines: vec![] });
            (ln_old, left_old, ln_new, left_new) = (a, b, c, d);
            continue;
        }
        if let Some(p) = line.strip_prefix("--- ") {
            // A plain unified diff (no "diff --git"): each "---" after a hunk starts a file.
            if cur.as_ref().is_none_or(|f| !f.hunks.is_empty()) {
                files.extend(cur.take());
                cur = Some(FileDiff::new(String::new()));
            }
            let f = cur.as_mut().unwrap();
            if p.trim() == "/dev/null" {
                f.kind = FileKind::Added;
            } else if f.path.is_empty() {
                f.path = clean_path(p);
            }
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };
        if let Some(p) = line.strip_prefix("+++ ") {
            if p.trim() == "/dev/null" {
                f.kind = FileKind::Deleted;
            } else {
                let new = clean_path(p);
                if f.kind == FileKind::Modified && !f.path.is_empty() && f.path != new && f.old_path.is_none() {
                    f.old_path = Some(f.path.clone());
                    f.kind = FileKind::Renamed;
                }
                f.path = new;
            }
        } else if line.starts_with("new file mode") {
            f.kind = FileKind::Added;
        } else if line.starts_with("deleted file mode") {
            f.kind = FileKind::Deleted;
        } else if let Some(p) = line.strip_prefix("rename from ") {
            f.old_path = Some(clean_path(p));
            f.kind = FileKind::Renamed;
        } else if let Some(p) = line.strip_prefix("rename to ") {
            f.path = clean_path(p);
            f.kind = FileKind::Renamed;
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            f.binary = true;
        }
    }
    files.extend(cur);
    files.retain(|f| !f.path.is_empty() || !f.hunks.is_empty());
    files
}

/// Lines added and removed in all files.
pub fn totals(files: &[FileDiff]) -> (u32, u32) {
    files.iter().fold((0, 0), |(a, d), f| (a + f.add, d + f.del))
}

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c => o.push(c),
        }
    }
    o
}

fn num(n: u32) -> String {
    if n == 0 { String::new() } else { n.to_string() }
}

fn class(k: LineKind) -> &'static str {
    match k {
        LineKind::Ctx => "",
        LineKind::Add => "dv-add",
        LineKind::Del => "dv-del",
        LineKind::NoNewline => "dv-nonl",
    }
}

fn sign(k: LineKind) -> &'static str {
    match k {
        LineKind::Add => "+",
        LineKind::Del => "-",
        _ => " ",
    }
}

/// The body of one file as a `<table>`: unified (old no, new no, line) or split (old no, old
/// line, new no, new line; removed lines paired with the added lines that replace them).
pub fn body_html(f: &FileDiff, split: bool) -> String {
    let mut s = String::with_capacity(f.lines() * 80);
    s.push_str(if split { "<table class=\"dv dv-split\"><colgroup><col class=\"dv-nc\"><col><col class=\"dv-nc\"><col></colgroup>" } else { "<table class=\"dv\"><colgroup><col class=\"dv-nc\"><col class=\"dv-nc\"><col></colgroup>" });
    for h in &f.hunks {
        let cols = if split { 4 } else { 3 };
        s.push_str(&format!("<tr class=\"dv-hunk\"><td colspan=\"{cols}\">{}</td></tr>", esc(&h.header)));
        if !split {
            for l in &h.lines {
                if l.kind == LineKind::NoNewline {
                    s.push_str(&format!("<tr class=\"dv-nonl\"><td></td><td></td><td>{}</td></tr>", esc(&l.text)));
                    continue;
                }
                s.push_str(&format!(
                    "<tr class=\"{}\"><td class=\"dv-n\">{}</td><td class=\"dv-n\">{}</td><td class=\"dv-c\"><span class=\"dv-s\">{}</span>{}</td></tr>",
                    class(l.kind),
                    num(l.old),
                    num(l.new),
                    sign(l.kind),
                    esc(&l.text)
                ));
            }
            continue;
        }
        let side = |l: Option<&Line>, old: bool| -> String {
            match l {
                Some(l) => format!("<td class=\"dv-n {c}\">{}</td><td class=\"dv-c {c}\">{}</td>", num(if old { l.old } else { l.new }), esc(&l.text), c = class(l.kind)),
                None => "<td class=\"dv-n dv-none\"></td><td class=\"dv-c dv-none\"></td>".to_string(),
            }
        };
        let lines: Vec<&Line> = h.lines.iter().filter(|l| l.kind != LineKind::NoNewline).collect();
        let mut i = 0;
        while i < lines.len() {
            if lines[i].kind == LineKind::Ctx {
                s.push_str(&format!("<tr>{}{}</tr>", side(Some(lines[i]), true), side(Some(lines[i]), false)));
                i += 1;
                continue;
            }
            let dels: Vec<&Line> = lines[i..].iter().take_while(|l| l.kind == LineKind::Del).copied().collect();
            i += dels.len();
            let adds: Vec<&Line> = lines[i..].iter().take_while(|l| l.kind == LineKind::Add).copied().collect();
            i += adds.len();
            for r in 0..dels.len().max(adds.len()) {
                s.push_str(&format!("<tr>{}{}</tr>", side(dels.get(r).copied(), true), side(adds.get(r).copied(), false)));
            }
        }
    }
    s.push_str("</table>");
    s
}

/// Files of a diff, each with a header (status, path, +/−) that folds its body. Big files start
/// folded. `on_open`: a click on the path (open it in the editor).
#[component]
pub fn DiffView(#[prop(into)] files: Signal<Vec<FileDiff>>, #[prop(into)] split: Signal<bool>, #[prop(optional)] on_open: Option<Callback<String>>) -> impl IntoView {
    // Folded files, by path; files past the size budget start folded.
    let folded = RwSignal::new(HashSet::<String>::new());
    let seen = StoredValue::new(HashSet::<String>::new());
    Effect::new(move |_| {
        files.with(|fs| {
            let mut budget = 3000usize;
            let mut fold = vec![];
            seen.update_value(|s| {
                for f in fs {
                    let n = f.lines();
                    if s.insert(f.path.clone()) && (n > budget || n > 1500) {
                        fold.push(f.path.clone());
                    }
                    budget = budget.saturating_sub(n);
                }
            });
            if !fold.is_empty() {
                folded.update(|x| x.extend(fold));
            }
        })
    });
    view! {
        <div class="dv-files">
            {move || files.get().into_iter().map(|f| {
                let path = f.path.clone();
                let p1 = path.clone();
                let p2 = path.clone();
                let is_folded = Memo::new(move |_| folded.with(|x| x.contains(&p1)));
                let toggle = move |_| folded.update(|x| { if !x.remove(&p2) { x.insert(p2.clone()); } });
                let shown = if let Some(o) = &f.old_path { format!("{o} → {}", f.path) } else { f.path.clone() };
                let (dir, base) = match shown.rfind('/') { Some(i) if f.old_path.is_none() => (shown[..=i].to_string(), shown[i + 1..].to_string()), _ => (String::new(), shown.clone()) };
                let letter = f.letter();
                let open = on_open.map(|cb| { let p = path.clone(); view! {
                    <button class="icon-btn dv-open" title="Open in the editor" on:click=move |e| { e.stop_propagation(); cb.run(p.clone()) }><Icon name="edit" size=13/></button>
                }});
                let f2 = f.clone();
                view! {
                    <div class="dv-file">
                        <div class="dv-head" on:click=toggle>
                            <span class="dv-chev" class:folded=move || is_folded.get()><Icon name="chevron" size=13/></span>
                            <span class=format!("dv-k dv-k-{letter}")>{letter}</span>
                            <span class="dv-path truncate" title=shown.clone()><span class="subdue">{dir}</span>{base}</span>
                            <span class="grow"></span>
                            {(f.add > 0).then(|| view! { <span class="dv-plus">{format!("+{}", f.add)}</span> })}
                            {(f.del > 0).then(|| view! { <span class="dv-minus">{format!("−{}", f.del)}</span> })}
                            {open}
                        </div>
                        <Show when=move || !is_folded.get()>
                            {
                                let f3 = f2.clone();
                                move || if f3.binary {
                                    view! { <div class="dv-empty small subdue">"Binary file"</div> }.into_any()
                                } else if f3.hunks.is_empty() {
                                    view! { <div class="dv-empty small subdue">{if f3.kind == FileKind::Renamed { "Renamed, no changes" } else { "No changes to show" }}</div> }.into_any()
                                } else {
                                    let f4 = f3.clone();
                                    view! { <div class="dv-body" inner_html=move || body_html(&f4, split.get())></div> }.into_any()
                                }
                            }
                        </Show>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "diff --git a/src/a.rs b/src/a.rs
index 1..2 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,4 +1,4 @@ fn main
 one
-two
+deux
+zwei
 three
--- not a header, a removed line
@@ -10 +11 @@
-x
+y
\\ No newline at end of file
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..e69de29
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
diff --git a/old name.txt b/new name.txt
similarity index 90%
rename from old name.txt
rename to new name.txt
diff --git a/gone.rs b/gone.rs
deleted file mode 100644
--- a/gone.rs
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/img.png b/img.png
Binary files a/img.png and b/img.png differ
";

    #[test]
    fn parses_files_hunks_and_lines() {
        let f = parse(D);
        assert_eq!(f.len(), 5, "{f:#?}");
        assert_eq!(f[0].path, "src/a.rs");
        assert_eq!(f[0].kind, FileKind::Modified);
        assert_eq!(f[0].hunks.len(), 2);
        // "--- not a header" is the 4th old line of the first hunk.
        assert_eq!(f[0].hunks[0].lines.len(), 6);
        assert_eq!(f[0].hunks[0].lines[5].kind, LineKind::Del);
        assert_eq!(f[0].hunks[0].lines[5].text, "-- not a header, a removed line");
        assert_eq!((f[0].add, f[0].del), (3, 3));
        assert_eq!(f[0].hunks[0].lines[2], Line { kind: LineKind::Add, old: 0, new: 2, text: "deux".into() });
        assert_eq!(f[0].hunks[1].lines.last().unwrap().kind, LineKind::NoNewline);
        assert_eq!((f[1].path.as_str(), f[1].kind, f[1].add), ("new.txt", FileKind::Added, 2));
        assert_eq!((f[2].path.as_str(), f[2].old_path.as_deref(), f[2].kind), ("new name.txt", Some("old name.txt"), FileKind::Renamed));
        assert_eq!((f[3].path.as_str(), f[3].kind, f[3].del), ("gone.rs", FileKind::Deleted, 1));
        assert!(f[4].binary);
        assert_eq!(totals(&f), (5, 4));
    }

    #[test]
    fn plain_unified_and_quoted() {
        let f = parse("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n--- a/y\n+++ b/y\n@@ -1 +1 @@\n-c\n+d\n");
        assert_eq!(f.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["x", "y"]);
        assert_eq!(clean_path("\"a/caf\\303\\251 \\\"q\\\".txt\""), "café \"q\".txt");
    }

    #[test]
    fn html_escapes_and_splits() {
        let f = parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n-<b>\n+&x\n ctx\n");
        let u = body_html(&f[0], false);
        assert!(u.contains("&lt;b&gt;") && u.contains("&amp;x") && !u.contains("<b>"));
        let s = body_html(&f[0], true);
        // The removed and added line share a row.
        assert!(s.contains("&lt;b&gt;</td><td class=\"dv-n dv-add\">1</td>"), "{s}");
    }
}
