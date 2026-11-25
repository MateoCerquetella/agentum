//! Markdown for the Secretary's answers and the notes preview (pulldown-cmark, in the browser).
//! Safe: raw HTML in the text shows as text, links are http(s), mailto or in-app, and a ```ui
//! block (or ```json of a known node) is drawn by [`super::genui`]. Checklist items can be
//! ticked: the callback gets the item's line in the source.
use super::genui;
use agentum_ui::prelude::*;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};
use wasm_bindgen::JsCast;

pub enum Piece {
    Html(String),
    Ui(Value),
}

fn attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

fn href(u: &str) -> String {
    let ok = genui::safe(u) || u.starts_with("mailto:") || u.starts_with('#');
    attr(if ok { u } else { "#" })
}

/// The text as HTML chunks and UI nodes, in order.
pub fn pieces(text: &str, ui: bool, tasks: bool) -> Vec<Piece> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut out: Vec<Piece> = vec![];
    let mut evs: Vec<Event> = vec![];
    // Inside a ```ui / ```json fence: its language, text and events (if it turns out not to be UI).
    let mut fence: Option<(String, String, Vec<Event>)> = None;
    let flush = |evs: &mut Vec<Event>, out: &mut Vec<Piece>| {
        if !evs.is_empty() {
            let mut html = String::new();
            pulldown_cmark::html::push_html(&mut html, evs.drain(..));
            out.push(Piece::Html(html));
        }
    };
    for (ev, range) in Parser::new_ext(text, opts).into_offset_iter() {
        if let Some((lang, body, raw)) = fence.as_mut() {
            match ev {
                Event::End(TagEnd::CodeBlock) => {
                    let parsed = genui::parse_ui(body).filter(|n| lang == "ui" || genui::UI_TYPES.contains(&n["t"].as_str().unwrap_or("")));
                    let (_, _, mut raw) = fence.take().unwrap();
                    match parsed {
                        Some(n) => {
                            flush(&mut evs, &mut out);
                            out.push(Piece::Ui(n));
                        }
                        None => {
                            evs.append(&mut raw);
                            evs.push(Event::End(TagEnd::CodeBlock));
                        }
                    }
                }
                ev => {
                    if let Event::Text(t) = &ev {
                        body.push_str(t);
                    }
                    raw.push(ev);
                }
            }
            continue;
        }
        match ev {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(ref l))) if ui && matches!(l.trim().to_lowercase().as_str(), "ui" | "json") => {
                fence = Some((l.trim().to_lowercase(), String::new(), vec![ev.clone()]));
            }
            Event::Html(s) | Event::InlineHtml(s) => evs.push(Event::Text(s)),
            Event::Start(Tag::Heading { .. }) => evs.push(Event::Html(CowStr::Borrowed("<div class=\"md-h\">"))),
            Event::End(TagEnd::Heading(_)) => evs.push(Event::Html(CowStr::Borrowed("</div>\n"))),
            Event::Start(Tag::Link { dest_url, .. }) | Event::Start(Tag::Image { dest_url, .. }) => {
                let internal = dest_url.starts_with('/') && !dest_url.starts_with("//");
                let target = if internal { "" } else { " target=\"_blank\" rel=\"noreferrer\"" };
                evs.push(Event::Html(format!("<a href=\"{}\"{target}>", href(&dest_url)).into()));
            }
            Event::End(TagEnd::Link) | Event::End(TagEnd::Image) => evs.push(Event::Html(CowStr::Borrowed("</a>"))),
            Event::TaskListMarker(done) => {
                if let Some(i) = evs.iter().rposition(|e| matches!(e, Event::Start(Tag::Item))) {
                    evs[i] = Event::Html(CowStr::Borrowed("<li class=\"md-task\">"));
                }
                let line = text[..range.start].matches('\n').count();
                let (on, off) = (if done { " checked" } else { "" }, if tasks { "" } else { " disabled" });
                evs.push(Event::Html(format!("<input type=\"checkbox\" data-line=\"{line}\"{on}{off}> ").into()));
            }
            ev => evs.push(ev),
        }
    }
    // A fence still being written (a streaming answer): show it as code so far.
    if let Some((_, _, mut raw)) = fence.take() {
        evs.append(&mut raw);
        evs.push(Event::End(TagEnd::CodeBlock));
    }
    flush(&mut evs, &mut out);
    out
}

/// The text drawn: `<div class="md">`. In-app links open beside what's on screen.
pub fn markdown(text: &str, ui: bool, on_task: Option<Callback<usize>>) -> AnyView {
    let parts = pieces(text, ui, on_task.is_some());
    let click = move |e: leptos::ev::MouseEvent| {
        let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
        if let Some(cb) = on_task {
            if let Some(line) = el.closest("input[data-line]").ok().flatten().and_then(|i| i.get_attribute("data-line")).and_then(|l| l.parse().ok()) {
                cb.run(line);
                return;
            }
        }
        if let Some(a) = el.closest("a[href]").ok().flatten() {
            let h = a.get_attribute("href").unwrap_or_default();
            if h.starts_with('/') && !h.starts_with("//") && !(e.meta_key() || e.ctrl_key()) {
                e.prevent_default();
                genui::open_url(&h);
            } else if h == "#" {
                e.prevent_default();
            }
        }
    };
    view! {
        <div class="md" on:click=click>
            {parts.into_iter().map(|p| match p {
                Piece::Html(h) => view! { <div class="md-part" inner_html=h/> }.into_any(),
                Piece::Ui(n) => view! { <div class="genui">{genui::node(&n)}</div> }.into_any(),
            }).collect_view()}
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn html(t: &str) -> String {
        pieces(t, true, true).into_iter().map(|p| match p {
            Piece::Html(h) => h,
            Piece::Ui(n) => format!("[ui {}]", n["t"]),
        }).collect()
    }
    #[test]
    fn safe_and_drawn() {
        assert!(html("<script>x</script> hi").contains("&lt;script&gt;"));
        assert!(html("[a](javascript:alert(1))").contains("href=\"#\""));
        assert!(html("x\n\n```ui\n{\"t\":\"metric\",\"label\":\"a\",\"value\":1}\n```\n").contains("[ui \"metric\"]"));
        assert!(html("```json\n{\"a\":1}\n```").contains("<code"));
        assert!(html("a\n\n- [ ] one\n- [x] two").contains("data-line=\"3\" checked"));
    }
}
