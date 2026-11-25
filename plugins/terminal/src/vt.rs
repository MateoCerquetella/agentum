//! The terminal's screen and keyboard, without the DOM: a vt100 screen row as HTML spans, and a
//! key press as the bytes a terminal sends.
use vt100::{Color, Screen};

fn esc_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
}

/// A colour as a CSS value: the 16 base colours are variables (the theme's), the rest computed.
fn css_color(c: Color) -> Option<String> {
    match c {
        Color::Default => None,
        Color::Idx(n) if n < 16 => Some(format!("var(--t{n})")),
        Color::Idx(n) if n < 232 => {
            const L: [u8; 6] = [0, 95, 135, 175, 215, 255];
            let n = n - 16;
            Some(format!("#{:02x}{:02x}{:02x}", L[(n / 36) as usize], L[((n / 6) % 6) as usize], L[(n % 6) as usize]))
        }
        Color::Idx(n) => {
            let v = 8 + 10 * (n - 232);
            Some(format!("#{v:02x}{v:02x}{v:02x}"))
        }
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
    }
}

#[derive(Clone, PartialEq)]
struct Style {
    fg: Option<String>,
    bg: Option<String>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
}

impl Style {
    fn of(c: &vt100::Cell) -> Style {
        let (mut fg, mut bg) = (css_color(c.fgcolor()), css_color(c.bgcolor()));
        // Bold base colours show bright, like most terminals.
        if c.bold() {
            if let Color::Idx(n) = c.fgcolor() {
                if n < 8 {
                    fg = Some(format!("var(--t{})", n + 8));
                }
            }
        }
        if c.inverse() {
            let f = fg.take().unwrap_or_else(|| "var(--t-fg)".into());
            fg = Some(bg.take().unwrap_or_else(|| "var(--t-bg)".into()));
            bg = Some(f);
        }
        Style { fg, bg, bold: c.bold(), dim: c.dim(), italic: c.italic(), underline: c.underline() }
    }
    fn plain(&self) -> bool {
        *self == Style { fg: None, bg: None, bold: false, dim: false, italic: false, underline: false }
    }
    fn open(&self, out: &mut String, cursor: bool) {
        out.push_str("<span");
        let mut cls = vec![];
        if cursor {
            cls.push("t-cur");
        }
        if self.bold {
            cls.push("t-b");
        }
        if self.dim {
            cls.push("t-d");
        }
        if self.italic {
            cls.push("t-i");
        }
        if self.underline {
            cls.push("t-u");
        }
        if !cls.is_empty() {
            out.push_str(&format!(" class=\"{}\"", cls.join(" ")));
        }
        let mut st = String::new();
        if let Some(f) = &self.fg {
            st.push_str(&format!("color:{f};"));
        }
        if let Some(b) = &self.bg {
            st.push_str(&format!("background:{b};"));
        }
        if !st.is_empty() {
            out.push_str(&format!(" style=\"{st}\""));
        }
        out.push('>');
    }
}

/// One row of the screen as HTML: runs of the same look in one span; `cursor` is the column of
/// the cursor on this row. Trailing blanks are left out.
pub fn row_html(screen: &Screen, row: u16, cursor: Option<u16>) -> String {
    let (_, cols) = screen.size();
    let cols = cols.get();
    let mut cells: Vec<(Style, String, bool)> = Vec::with_capacity(cols as usize);
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else { break };
        if cell.is_wide_continuation() {
            continue;
        }
        let text = if cell.has_contents() { cell.contents().to_string() } else { " ".to_string() };
        cells.push((Style::of(cell), text, cursor == Some(col)));
    }
    while cells.last().is_some_and(|(s, t, cur)| !cur && t == " " && s.bg.is_none() && !s.underline) {
        cells.pop();
    }
    let mut out = String::new();
    let mut i = 0;
    while i < cells.len() {
        let (st, _, cur) = &cells[i];
        if *cur {
            st.open(&mut out, true);
            esc_into(&mut out, &cells[i].1);
            out.push_str("</span>");
            i += 1;
            continue;
        }
        let plain = st.plain();
        if !plain {
            st.open(&mut out, false);
        }
        while i < cells.len() && !cells[i].2 && cells[i].0 == *st {
            esc_into(&mut out, &cells[i].1);
            i += 1;
        }
        if !plain {
            out.push_str("</span>");
        }
    }
    out
}

/// The bytes for a key press (None: not the terminal's: ⌘ shortcuts, a lone modifier).
/// `app_cursor`: the program asked for application cursor keys (vim, less).
pub fn key_bytes(key: &str, ctrl: bool, alt: bool, shift: bool, meta: bool, app_cursor: bool) -> Option<Vec<u8>> {
    if meta {
        return None;
    }
    let m = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let cursor = |c: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{c}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> { if m > 1 { format!("\x1b[{n};{m}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() } };
    let out: Vec<u8> = match key {
        "Enter" => if alt { b"\x1b\r".to_vec() } else { b"\r".to_vec() },
        "Backspace" => match (ctrl, alt) {
            (true, _) => vec![0x08],
            (_, true) => b"\x1b\x7f".to_vec(),
            _ => vec![0x7f],
        },
        "Tab" => if shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() },
        "Escape" => vec![0x1b],
        "ArrowUp" => cursor('A'),
        "ArrowDown" => cursor('B'),
        "ArrowRight" => cursor('C'),
        "ArrowLeft" => cursor('D'),
        "Home" => cursor('H'),
        "End" => cursor('F'),
        "Insert" => tilde(2),
        "Delete" => tilde(3),
        "PageUp" => tilde(5),
        "PageDown" => tilde(6),
        "F1" => b"\x1bOP".to_vec(),
        "F2" => b"\x1bOQ".to_vec(),
        "F3" => b"\x1bOR".to_vec(),
        "F4" => b"\x1bOS".to_vec(),
        "F5" => tilde(15),
        "F6" => tilde(17),
        "F7" => tilde(18),
        "F8" => tilde(19),
        "F9" => tilde(20),
        "F10" => tilde(21),
        "F11" => tilde(23),
        "F12" => tilde(24),
        k => {
            let mut it = k.chars();
            let (Some(c), None) = (it.next(), it.next()) else { return None };
            if ctrl {
                let b = match c {
                    'a'..='z' => c as u8 - b'a' + 1,
                    'A'..='Z' => c as u8 - b'A' + 1,
                    '@' | ' ' | '2' => 0,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '-' | '7' => 0x1f,
                    '?' | '8' => 0x7f,
                    _ => return None,
                };
                if alt { vec![0x1b, b] } else { vec![b] }
            } else {
                let mut v = vec![];
                // ⌥ with a plain key is Meta (ESC prefix); a character ⌥ made (macOS) is sent as is.
                if alt && c.is_ascii() {
                    v.push(0x1b);
                }
                let mut buf = [0u8; 4];
                v.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                v
            }
        }
    };
    Some(out)
}

/// Pasted text: newlines as Enter, wrapped as a paste when the program asked for it.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let t = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed { format!("\x1b[200~{t}\x1b[201~").into_bytes() } else { t.into_bytes() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(key_bytes("a", false, false, false, false, false), Some(b"a".to_vec()));
        assert_eq!(key_bytes("c", true, false, false, false, false), Some(vec![3]));
        assert_eq!(key_bytes("c", false, false, false, true, false), None);
        assert_eq!(key_bytes("ArrowUp", false, false, false, false, false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes("ArrowUp", false, false, false, false, true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_bytes("ArrowLeft", true, false, false, false, false), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(key_bytes("b", false, true, false, false, false), Some(b"\x1bb".to_vec()));
        assert_eq!(key_bytes("Shift", false, false, true, false, false), None);
        assert_eq!(key_bytes("é", false, false, false, false, false), Some("é".as_bytes().to_vec()));
        assert_eq!(paste_bytes("a\nb", true), b"\x1b[200~a\rb\x1b[201~".to_vec());
    }

    #[test]
    fn rows() {
        let mut p = vt100::Parser::new(std::num::NonZeroU16::new(3).unwrap(), std::num::NonZeroU16::new(20).unwrap(), 100);
        p.process(b"plain \x1b[1;31mred<b>\x1b[0m end\r\n\x1b[7mrev\x1b[0m\r\n");
        let s = p.screen();
        let r0 = row_html(s, 0, None);
        assert_eq!(r0, "plain <span class=\"t-b\" style=\"color:var(--t9);\">red&lt;b&gt;</span> end");
        let r1 = row_html(s, 1, None);
        assert!(r1.starts_with("<span style=\"color:var(--t-bg);background:var(--t-fg);\">rev</span>"), "{r1}");
        let (row, col) = s.cursor_position();
        assert_eq!(row_html(s, row, Some(col)), "<span class=\"t-cur\"> </span>");
    }
}
