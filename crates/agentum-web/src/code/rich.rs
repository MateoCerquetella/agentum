//! Reads what a command printed and finds the things worth a card: a test run, changed files,
//! commits, compile and type errors. Plain text in, small typed facts out; anything it doesn't
//! recognise stays text. (The Empirical Factory's generative conversation cards, in Rust.)
//! No web dependency, so it is tested natively.
use regex_lite::Regex;
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq)]
pub struct FileStat {
    pub path: String,
    /// M, A, D, R or U.
    pub status: char,
    pub plus: Option<u32>,
    pub minus: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rich {
    Tests { pass: u32, fail: u32, skip: u32, files: Option<u32>, secs: Option<f64>, failures: Vec<String> },
    Changes { files: Vec<FileStat>, plus: Option<u32>, minus: Option<u32> },
    Commits(Vec<(String, String)>),
    /// rustc and tsc errors, as `file:line message`.
    Errors(Vec<String>),
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("a valid pattern"))
}

macro_rules! re {
    ($pat:literal) => {{
        static R: OnceLock<Regex> = OnceLock::new();
        re(&R, $pat)
    }};
}

fn num(s: Option<regex_lite::Match>) -> u32 {
    s.and_then(|m| m.as_str().parse().ok()).unwrap_or(0)
}

/// The cards in a command's output, at most one of each kind.
pub fn rich_of(output: &str) -> Vec<Rich> {
    let mut out = vec![];
    // The end is where the summaries are.
    let mut start = output.len().saturating_sub(60_000);
    while !output.is_char_boundary(start) {
        start += 1;
    }
    let text = &output[start..];

    if let Some(t) = tests(text) {
        out.push(t);
    }

    // rustc: "error[E0308]: mismatched types" then "  --> src/a.rs:12:5"; tsc: "a.ts(3,1): error TS2322: …".
    let mut errors: Vec<String> = re!(r"(?m)^error(?:\[E\d+\])?: (.+)\n\s+--> (\S+?):(\d+):\d+")
        .captures_iter(text)
        .map(|c| format!("{}:{} {}", &c[2], &c[3], &c[1]))
        .collect();
    errors.extend(re!(r"(?m)^(\S+\.tsx?)\((\d+),\d+\): error TS\d+: (.+)$").captures_iter(text).map(|c| format!("{}:{} {}", &c[1], &c[2], &c[3])));
    if !errors.is_empty() {
        errors.truncate(8);
        out.push(Rich::Errors(errors));
    }

    // git diff --stat: " src/a.rs | 29 ++++---" and "3 files changed, 53 insertions(+), 11 deletions(-)".
    let stat: Vec<_> = re!(r"(?m)^\s*([\w./@-]+\.\w+|[\w./@-]+/)\s+\|\s+(\d+) ([+-]*)$").captures_iter(text).collect();
    if !stat.is_empty() {
        let total = re!(r"(\d+) files? changed(?:, (\d+) insertions?\(\+\))?(?:, (\d+) deletions?\(-\))?").captures(text);
        let files = stat
            .iter()
            .map(|c| {
                let n: u32 = c[2].parse().unwrap_or(0);
                let (p, m) = (c[3].matches('+').count() as u32, c[3].matches('-').count() as u32);
                let share = (p + m).max(1);
                FileStat { path: c[1].to_string(), status: 'M', plus: Some((n * p + share / 2) / share), minus: Some((n * m + share / 2) / share) }
            })
            .collect();
        out.push(Rich::Changes { files, plus: total.as_ref().map(|c| num(c.get(2))), minus: total.as_ref().map(|c| num(c.get(3))) });
    } else {
        // git status --short: " M src/a.rs", "?? new.rs", "A  b.rs"; only when every such line reads as one.
        let short: Vec<_> = re!(r"(?m)^ ?([MADRU?]{1,2}) {1,2}([\w./@-]+\.\w+)$").captures_iter(text).collect();
        let loose = text.lines().filter(|l| re!(r"^ ?[MADRU?]{1,2} {1,2}\S").is_match(l)).count();
        if !short.is_empty() && short.len() == loose {
            let files = short
                .iter()
                .map(|c| {
                    let s = c[1].trim();
                    FileStat { path: c[2].to_string(), status: if s == "??" { 'A' } else { s.chars().next().unwrap_or('M') }, plus: None, minus: None }
                })
                .collect();
            out.push(Rich::Changes { files, plus: None, minus: None });
        }
    }

    // git log --oneline. A hash has a letter in it: a run of digits is a pid or a count.
    if !out.iter().any(|r| matches!(r, Rich::Tests { .. })) {
        let commits: Vec<(String, String)> = re!(r"(?m)^([0-9a-f]{7,12}) (.{3,})$")
            .captures_iter(text)
            .filter(|c| c[1].chars().any(|ch| ch.is_ascii_alphabetic()) && !c[2].starts_with('|'))
            .map(|c| (c[1].to_string(), c[2].to_string()))
            .collect();
        if (1..=30).contains(&commits.len()) {
            out.push(Rich::Commits(commits.into_iter().take(8).collect()));
        }
    }
    out
}

/// bun test / vitest, cargo test, pytest.
fn tests(text: &str) -> Option<Rich> {
    let failed_names = |pat: &Regex| pat.captures_iter(text).map(|c| c[1].to_string()).take(6).collect::<Vec<_>>();

    // cargo: one "test result:" line per test binary.
    let cargo: Vec<_> = re!(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored;.*?finished in ([\d.]+)s").captures_iter(text).collect();
    if !cargo.is_empty() {
        let sum = |i| cargo.iter().map(|c| num(c.get(i))).sum::<u32>();
        let secs = cargo.iter().filter_map(|c| c[4].parse::<f64>().ok()).sum();
        return Some(Rich::Tests { pass: sum(1), fail: sum(2), skip: sum(3), files: None, secs: Some(secs), failures: failed_names(re!(r"(?m)^test (\S+) \.\.\. FAILED$")) });
    }

    // pytest: "==== 2 failed, 10 passed, 1 skipped in 0.52s ====".
    if let Some(c) = re!(r"(?m)^=+ (.*\b(?:passed|failed)\b.*) in ([\d.]+)s").captures(text) {
        let count = |word: &str| re!(r"(\d+) (\w+)").captures_iter(&c[1]).find(|x| &x[2] == word).map(|x| num(x.get(1))).unwrap_or(0);
        return Some(Rich::Tests { pass: count("passed"), fail: count("failed") + count("error") + count("errors"), skip: count("skipped"), files: None, secs: c[2].parse().ok(), failures: failed_names(re!(r"(?m)^FAILED (\S+)")) });
    }

    // XCTest (swift test): "Executed 5 tests, with 1 test skipped and 0 failures (0 unexpected) in 0.03 (0.04) seconds",
    // once per suite; the last is the whole run.
    if let Some(c) = re!(r"Executed (\d+) tests?, with (?:(\d+) tests? skipped and )?(\d+) failures? .*?in ([\d.]+)").captures_iter(text).last() {
        let (n, skip, fail) = (num(c.get(1)), num(c.get(2)), num(c.get(3)));
        return Some(Rich::Tests { pass: n.saturating_sub(skip + fail), fail, skip, files: None, secs: c[4].parse().ok(), failures: failed_names(re!(r"(?m)^Test Case '(.+?)' failed")) });
    }

    // bun test / vitest: the summary lines, else count (pass)/(fail) lines.
    let line_num = |r: &Regex| r.captures(text).map(|c| num(c.get(1)));
    let pass = line_num(re!(r"(?m)^\s*(\d+) pass\s*$"));
    let fail = line_num(re!(r"(?m)^\s*(\d+) fail\s*$"));
    let ran = re!(r"Ran (\d+) tests? across (\d+) files?\.?\s*\[([\d.]+)(ms|s)\]").captures(text);
    let failures = failed_names(re!(r"(?m)^\(fail\) (.+?)(?: \[[\d.]+m?s\])?$"));
    if pass.is_some() || fail.is_some() || ran.is_some() {
        let secs = ran.as_ref().and_then(|c| c[3].parse::<f64>().ok().map(|n| if &c[4] == "ms" { n / 1000.0 } else { n }));
        return Some(Rich::Tests {
            pass: pass.unwrap_or(0),
            fail: fail.unwrap_or(failures.len() as u32),
            skip: line_num(re!(r"(?m)^\s*(\d+) skip\s*$")).unwrap_or(0),
            files: ran.as_ref().map(|c| num(c.get(2))),
            secs,
            failures,
        });
    }
    let p = re!(r"(?m)^\(pass\) ").find_iter(text).count() as u32;
    (p as usize + failures.len() >= 3).then(|| Rich::Tests { pass: p, fail: failures.len() as u32, skip: 0, files: None, secs: None, failures })
}

/// Agent message HTML with inline `code` that is a commit hash or a file path marked as a chip.
pub fn chips(html: &str) -> String {
    if !html.contains("<code>") {
        return html.to_string();
    }
    re!(r"<code>([^<\n]{7,200})</code>")
        .replace_all(html, |c: &regex_lite::Captures| {
            let v = &c[1];
            let class = if re!(r"^[0-9a-f]{7,40}$").is_match(v) && v.chars().any(|ch| ch.is_ascii_alphabetic()) {
                "chip-sha"
            } else if re!(r"^[\w@.-]+(/[\w@.-]+)+\.\w{1,5}(:\d+)?$").is_match(v) {
                "chip-file"
            } else {
                return c[0].to_string();
            };
            let shown = if class == "chip-sha" { &v[..7] } else { v };
            format!("<code class=\"{class}\" title=\"{v}\">{shown}</code>")
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_test_runs_are_summed() {
        let out = "running 3 tests\ntest a::one ... ok\ntest a::two ... FAILED\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s\n\
                   running 4 tests\ntest result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.30s\n";
        assert_eq!(rich_of(out), vec![Rich::Tests { pass: 5, fail: 1, skip: 1, files: None, secs: Some(1.5), failures: vec!["a::two".into()] }]);
    }

    #[test]
    fn bun_summary() {
        let out = "(pass) a > b [1.00ms]\n(fail) a > c [2.00ms]\n\n 12 pass\n 1 fail\n 2 skip\nRan 15 tests across 3 files. [420.00ms]\n";
        assert_eq!(rich_of(out), vec![Rich::Tests { pass: 12, fail: 1, skip: 2, files: Some(3), secs: Some(0.42), failures: vec!["a > c".into()] }]);
    }

    #[test]
    fn pytest_summary() {
        let out = "FAILED tests/test_a.py::test_x - assert 1 == 2\n==== 1 failed, 9 passed, 2 skipped in 0.52s ====\n";
        assert_eq!(rich_of(out), vec![Rich::Tests { pass: 9, fail: 1, skip: 2, files: None, secs: Some(0.52), failures: vec!["tests/test_a.py::test_x".into()] }]);
    }

    #[test]
    fn xctest_takes_the_whole_run() {
        let out = "Test Case 'testDark' failed (0.011 seconds)\nExecuted 3 tests, with 1 failure (0 unexpected) in 0.032 (0.033) seconds\n\
                   Executed 5 tests, with 1 test skipped and 1 failure (0 unexpected) in 0.050 (0.051) seconds\n";
        assert_eq!(rich_of(out), vec![Rich::Tests { pass: 3, fail: 1, skip: 1, files: None, secs: Some(0.05), failures: vec!["testDark".into()] }]);
    }

    #[test]
    fn rustc_and_tsc_errors() {
        let out = "error[E0308]: mismatched types\n  --> src/main.rs:12:5\n   |\nerror: could not compile `x`\nsrc/a.ts(3,1): error TS2322: Type 'x' is not assignable.\n";
        assert_eq!(rich_of(out), vec![Rich::Errors(vec!["src/main.rs:12 mismatched types".into(), "src/a.ts:3 Type 'x' is not assignable.".into()])]);
    }

    #[test]
    fn diff_stat() {
        let out = " src/a.rs   | 10 +++++++---\n docs/b.md  |  2 ++\n 2 files changed, 9 insertions(+), 3 deletions(-)\n";
        let Rich::Changes { files, plus, minus } = &rich_of(out)[0] else { panic!() };
        assert_eq!((files.len(), *plus, *minus), (2, Some(9), Some(3)));
        assert_eq!((files[0].plus, files[0].minus), (Some(7), Some(3)));
    }

    #[test]
    fn status_short_only_when_every_line_is_one() {
        let ok = " M src/a.rs\n?? new.rs\n";
        let Rich::Changes { files, .. } = &rich_of(ok)[0] else { panic!() };
        assert_eq!(files.iter().map(|f| f.status).collect::<String>(), "MA");
        assert!(rich_of(" M src/a.rs\n M weird path with spaces\n").is_empty());
    }

    #[test]
    fn commits_need_a_letter() {
        let out = "68c969c6 Merge branch 'beta'\n09992d2b A comment\n";
        assert_eq!(rich_of(out), vec![Rich::Commits(vec![("68c969c6".into(), "Merge branch 'beta'".into()), ("09992d2b".into(), "A comment".into())])]);
        assert!(rich_of("1234567 processes running\n").is_empty());
    }

    #[test]
    fn plain_output_has_no_cards() {
        assert!(rich_of("Compiling agentum v0.1.0\n    Finished `dev` profile in 3.2s\n").is_empty());
        assert!(rich_of("").is_empty());
    }

    #[test]
    fn chips_mark_hashes_and_paths_only() {
        assert_eq!(chips("<p>see <code>68c969c6aa</code></p>"), "<p>see <code class=\"chip-sha\" title=\"68c969c6aa\">68c969c</code></p>");
        assert_eq!(chips("<code>src/code/rich.rs:12</code>"), "<code class=\"chip-file\" title=\"src/code/rich.rs:12\">src/code/rich.rs:12</code>");
        assert_eq!(chips("<code>cargo build</code> <code>1234567</code>"), "<code>cargo build</code> <code>1234567</code>");
    }
}
