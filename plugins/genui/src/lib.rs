//! Generative UI: Draws ```ui blocks agents write as live cards, charts and tables.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "genui",
    name: "Generative UI",
    description: "Draws ```ui blocks agents write as live cards, charts and tables.",
    icon: "layout",
    group: Group::Looks,
    default_on: true,
    settings: &[],
};

pub mod lang;

/// What every agent is told (the hub part adds it to their system prompt): the language and exactly
/// the components the web part draws. The tests below evaluate its examples.
pub const INSTRUCTIONS: &str = include_str!("instructions.md");

#[cfg(test)]
mod tests {
    use super::lang::{self, Val};

    /// Every component the web part draws (web.rs: container, input, leaf).
    const DRAWN: &[&str] = &[
        "Card", "Row", "Stack", "Grid", "Tabs", "Details", "Form", "Slider", "Stepper", "Toggle", "Switch", "Choice", "Radio", "Select", "Checklist", "TextInput",
        "Input", "TextArea", "Text", "Heading", "Markdown", "Callout", "Metric", "Stat", "Badge", "Icon", "Divider", "Code", "KeyValue", "Table", "List", "Steps",
        "Timeline", "Progress", "Chart", "Sparkline", "Button", "Send", "Set", "OpenUrl", "Copy", "FollowUps", "Image",
    ];

    fn nodes(v: &Val, out: &mut Vec<String>) {
        match v {
            Val::Node(n) => {
                out.push(n.name.clone());
                for a in n.args.iter().chain(n.named.iter().map(|(_, v)| v)) {
                    nodes(a, out);
                }
            }
            Val::List(l) => l.iter().for_each(|x| nodes(x, out)),
            Val::Obj(o) => o.iter().for_each(|(_, x)| nodes(x, out)),
            _ => {}
        }
    }

    #[test]
    fn instructions_examples_draw() {
        let blocks: Vec<&str> = super::INSTRUCTIONS.split("```ui\n").skip(1).map(|b| b.split("```").next().unwrap()).collect();
        assert_eq!(blocks.len(), 3);
        for b in blocks {
            let p = lang::Program::parse(b);
            let (state, errs) = p.initial_state();
            assert!(errs.is_empty(), "{errs:?} in\n{b}");
            let mut names = vec![];
            for r in p.roots(&state) {
                nodes(&r, &mut names);
            }
            assert!(!names.is_empty());
            for n in names {
                assert!(DRAWN.contains(&n.as_str()), "{n} isn't drawn (in\n{b})");
            }
        }
        // Every component it names is one the web part draws.
        for line in super::INSTRUCTIONS.lines().filter(|l| ["Layout:", "Content:", "Data:", "Inputs", "Actions:"].iter().any(|p| l.starts_with(p))) {
            let body = line.split_once(':').map(|x| x.1).unwrap_or(line);
            for item in body.split(" · ") {
                let name: String = item.trim().chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                if name.starts_with(|c: char| c.is_ascii_uppercase()) {
                    assert!(DRAWN.contains(&name.as_str()), "{name} is described but not drawn");
                }
            }
        }
    }
}

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;
