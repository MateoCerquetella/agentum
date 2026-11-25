//! Markdown → HTML for the timeline, on the hub (so the web app stays small): GitHub-flavoured,
//! code highlighted by syntect into `sy-` classes, raw HTML escaped (agents write anything), and
//! fences a plugin draws (```ui) left as `<agentum-fence>` for the web app.
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};
use std::sync::OnceLock;
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Fence languages a plugin's web part draws.
pub const PLUGIN_FENCES: &[&str] = &["ui", "app"];

fn syntaxes() -> &'static SyntaxSet {
    static S: OnceLock<SyntaxSet> = OnceLock::new();
    S.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn esc(s: &str) -> String {
    html_escape::encode_text(s).into_owned()
}

fn highlight(lang: &str, code: &str) -> String {
    let ss = syntaxes();
    let syn = ss.find_syntax_by_token(lang).or_else(|| ss.find_syntax_by_extension(lang));
    let Some(syn) = syn.filter(|_| code.len() < 200_000) else { return esc(code) };
    let mut g = ClassedHTMLGenerator::new_with_class_style(syn, ss, ClassStyle::SpacedPrefixed { prefix: "sy-" });
    for line in LinesWithEndings::from(code) {
        if g.parse_html_for_line_which_includes_newline(line).is_err() {
            return esc(code);
        }
    }
    g.finalize()
}

pub fn markdown(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES);
    let mut out: Vec<Event> = vec![];
    let mut in_code: Option<String> = None;
    let mut code = String::new();
    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code = Some(match kind {
                    CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                });
                code.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                let lang = in_code.take().unwrap_or_default();
                let html = if PLUGIN_FENCES.contains(&lang.as_str()) {
                    format!("<agentum-fence data-lang=\"{lang}\">{}</agentum-fence>", esc(&code))
                } else {
                    let cls = if lang.is_empty() { String::new() } else { format!(" class=\"language-{}\"", esc(&lang)) };
                    format!("<pre class=\"code\"><code{cls}>{}</code></pre>", highlight(&lang, &code))
                };
                out.push(Event::Html(CowStr::from(html)));
            }
            Event::Text(t) if in_code.is_some() => code.push_str(&t),
            // Agents' raw HTML is shown, never run.
            Event::Html(h) | Event::InlineHtml(h) => out.push(Event::Text(h)),
            Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
                let safe = dest_url.starts_with("http://") || dest_url.starts_with("https://") || dest_url.starts_with('/') || dest_url.starts_with('#') || dest_url.starts_with("mailto:");
                let dest = if safe { dest_url } else { CowStr::from("#") };
                out.push(Event::Start(Tag::Link { link_type, dest_url: dest, title, id }));
            }
            Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
                let safe = dest_url.starts_with("https://") || dest_url.starts_with("data:image/");
                out.push(Event::Start(Tag::Image { link_type, dest_url: if safe { dest_url } else { CowStr::from("") }, title, id }));
            }
            e => out.push(e),
        }
    }
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, out.into_iter());
    html
}

/// CSS for the `sy-` classes: a dark and a light theme, following html[data-scheme].
pub fn syntax_css() -> &'static str {
    static C: OnceLock<String> = OnceLock::new();
    C.get_or_init(|| {
        let ts = syntect::highlighting::ThemeSet::load_defaults();
        let style = ClassStyle::SpacedPrefixed { prefix: "sy-" };
        let scope = |css: String, sel: &str| css.lines().map(|l| if l.starts_with('.') { format!("{sel} {l}") } else { l.to_string() }).collect::<Vec<_>>().join("\n");
        let dark = syntect::html::css_for_theme_with_class_style(&ts.themes["base16-ocean.dark"], style).unwrap_or_default();
        let light = syntect::html::css_for_theme_with_class_style(&ts.themes["InspiredGitHub"], style).unwrap_or_default();
        format!("{}\n{}", scope(dark, "html[data-scheme=\"dark\"] .md"), scope(light, "html[data-scheme=\"light\"] .md"))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_and_fences() {
        let h = super::markdown("hi <script>x</script>\n\n```rust\nfn a() {}\n```\n\n```ui\nCard(\"x\")\n```\n[a](javascript:alert(1))");
        assert!(!h.contains("<script>"));
        assert!(h.contains("sy-"));
        assert!(h.contains("<agentum-fence data-lang=\"ui\">Card(&quot;x&quot;)") || h.contains("<agentum-fence data-lang=\"ui\">Card(\"x\")"));
        assert!(!h.contains("javascript:"));
    }
}
