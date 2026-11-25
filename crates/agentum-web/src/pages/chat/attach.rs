//! The text of a file attached to a chat, pulled out here in the browser: anything that is plain
//! text, and Word, Excel, PowerPoint and OpenDocument (a zip of XML: inflated with miniz_oxide).
//! The hub stores the text (`POST /api/chat/files`) and the model reads it.
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

pub const ACCEPT: &str = ".docx,.xlsx,.pptx,.odt,.ods,.odp,.txt,.md,.csv,.tsv,.json,.xml,.html,.htm,.yaml,.yml,.toml,.ini,.log,.rtf,.sql,.js,.ts,.tsx,.jsx,.py,.rs,.go,.java,.kt,.swift,.c,.h,.cpp,.cs,.rb,.php,.sh,.css,.ics,.vcf,.eml,text/*";
const MAX_BYTES: f64 = 25.0 * 1024.0 * 1024.0;

pub async fn extract_text(file: &web_sys::File) -> Result<String, String> {
    let name = file.name();
    if file.size() > MAX_BYTES {
        return Err(format!("{name} is over 25 MB."));
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    let ty = file.type_();
    if ext == "pdf" || ty == "application/pdf" {
        return Err(format!("{name}: PDFs can't be read here yet. Copy its text into the message, or save it as text and attach that."));
    }
    if ["doc", "xls", "ppt"].contains(&ext.as_str()) {
        return Err(format!("Old .{ext} files can't be read. Save it as .{ext}x and attach that."));
    }
    if ty.starts_with("image/") || ty.starts_with("audio/") || ty.starts_with("video/") {
        return Err(format!("{name}: the Secretary reads text, Word, Excel and PowerPoint files."));
    }
    let buf = JsFuture::from(file.array_buffer()).await.map_err(|_| format!("Couldn't read {name}."))?;
    let bytes = js_sys::Uint8Array::new(buf.unchecked_ref()).to_vec();
    if ["docx", "xlsx", "pptx", "odt", "ods", "odp"].contains(&ext.as_str()) {
        return office(&ext, &bytes).ok_or_else(|| format!("{name} couldn't be opened."));
    }
    if bytes.iter().take(4096).any(|b| *b == 0) {
        return Err(format!("{name} isn't a text file the Secretary can read."));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// ── zip ──

fn u16le(b: &[u8], i: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?) as usize)
}
fn u32le(b: &[u8], i: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize)
}

/// Every file in a zip: (name, its bytes, inflated).
fn unzip(b: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let eocd = (0..b.len().saturating_sub(21)).rev().find(|&i| b[i..].starts_with(&[0x50, 0x4b, 0x05, 0x06]))?;
    let (count, mut at) = (u16le(b, eocd + 10)?, u32le(b, eocd + 16)?);
    let mut out = vec![];
    for _ in 0..count {
        if !b.get(at..)?.starts_with(&[0x50, 0x4b, 0x01, 0x02]) {
            break;
        }
        let (method, size) = (u16le(b, at + 10)?, u32le(b, at + 20)?);
        let (n, m, k, local) = (u16le(b, at + 28)?, u16le(b, at + 30)?, u16le(b, at + 32)?, u32le(b, at + 42)?);
        let name = String::from_utf8_lossy(b.get(at + 46..at + 46 + n)?).into_owned();
        at += 46 + n + m + k;
        let start = local + 30 + u16le(b, local + 26)? + u16le(b, local + 28)?;
        let data = b.get(start..start + size)?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec(data).ok()?,
            _ => continue,
        };
        out.push((name, bytes));
    }
    Some(out)
}

// ── XML, just enough ──

enum Tok<'a> {
    Open(&'a str, &'a str, bool),
    Close(&'a str),
    Text(&'a str),
}

fn walk<'a>(xml: &'a str, mut f: impl FnMut(Tok<'a>)) {
    let mut rest = xml;
    while !rest.is_empty() {
        match rest.find('<') {
            Some(0) => {
                let Some(end) = rest.find('>') else { break };
                let tag = &rest[1..end];
                rest = &rest[end + 1..];
                if tag.starts_with('?') || tag.starts_with('!') {
                    continue;
                }
                if let Some(name) = tag.strip_prefix('/') {
                    f(Tok::Close(name.trim()));
                } else {
                    let selfclose = tag.ends_with('/');
                    let tag = tag.trim_end_matches('/');
                    let (name, attrs) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
                    f(Tok::Open(name, attrs, selfclose));
                }
            }
            Some(i) => {
                f(Tok::Text(&rest[..i]));
                rest = &rest[i..];
            }
            None => {
                f(Tok::Text(rest));
                break;
            }
        }
    }
}

fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e < 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let c = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match c {
            Some(c) => out.push(c),
            None => out.push_str(&rest[..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let i = attrs.find(&format!("{key}=\""))?;
    let v = &attrs[i + key.len() + 2..];
    Some(&v[..v.find('"')?])
}

/// Text inside `text` elements, a line per `para` element.
fn paragraphs(xml: &str, para: &str, text: &str) -> String {
    let (mut out, mut line, mut inside) = (vec![], String::new(), false);
    walk(xml, |t| match t {
        Tok::Open(n, _, false) if n == text => inside = true,
        Tok::Close(n) if n == text => inside = false,
        Tok::Open("w:tab", _, _) => line.push('\t'),
        Tok::Open("w:br", _, _) => line.push('\n'),
        Tok::Close(n) if n == para => out.push(std::mem::take(&mut line)),
        Tok::Text(s) if inside => line.push_str(&decode(s)),
        _ => {}
    });
    out.join("\n")
}

fn num_in(path: &str) -> usize {
    path.trim_end_matches(".xml").chars().rev().take_while(|c| c.is_ascii_digit()).collect::<String>().chars().rev().collect::<String>().parse().unwrap_or(0)
}

fn office(ext: &str, bytes: &[u8]) -> Option<String> {
    let files = unzip(bytes)?;
    let get = |p: &str| files.iter().find(|f| f.0 == p).map(|f| String::from_utf8_lossy(&f.1).into_owned());
    let ordered = |prefix: &str| {
        let mut v: Vec<&(String, Vec<u8>)> = files.iter().filter(|f| f.0.starts_with(prefix) && f.0.ends_with(".xml") && !f.0[prefix.len()..].contains('/')).collect();
        v.sort_by_key(|f| num_in(&f.0));
        v.into_iter().map(|f| String::from_utf8_lossy(&f.1).into_owned()).collect::<Vec<_>>()
    };
    let text = match ext {
        "docx" => paragraphs(&get("word/document.xml")?, "w:p", "w:t"),
        "pptx" => ordered("ppt/slides/slide").iter().enumerate().map(|(i, x)| format!("[Slide {}]\n{}", i + 1, paragraphs(x, "a:p", "a:t").lines().filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n"))).collect::<Vec<_>>().join("\n\n"),
        "xlsx" => {
            let mut shared: Vec<String> = vec![];
            if let Some(x) = get("xl/sharedStrings.xml") {
                let (mut cur, mut inside) = (String::new(), false);
                walk(&x, |t| match t {
                    Tok::Open("si", _, _) => cur.clear(),
                    Tok::Close("si") => shared.push(std::mem::take(&mut cur)),
                    Tok::Open("t", _, false) => inside = true,
                    Tok::Close("t") => inside = false,
                    Tok::Text(s) if inside => cur.push_str(&decode(s)),
                    _ => {}
                });
            }
            let mut names: Vec<String> = vec![];
            if let Some(x) = get("xl/workbook.xml") {
                walk(&x, |t| {
                    if let Tok::Open("sheet", a, _) = t {
                        names.push(decode(attr(a, "name").unwrap_or("")));
                    }
                });
            }
            ordered("xl/worksheets/sheet").iter().enumerate().map(|(i, x)| {
                let (mut rows, mut row, mut cell, mut kind, mut inside): (Vec<String>, Vec<String>, String, String, bool) = (vec![], vec![], String::new(), String::new(), false);
                walk(x, |t| match t {
                    Tok::Open("c", a, sc) => {
                        kind = attr(a, "t").unwrap_or("").to_string();
                        cell.clear();
                        if sc {
                            row.push(String::new());
                        }
                    }
                    Tok::Open("v" | "t", _, false) => inside = true,
                    Tok::Close("v" | "t") => inside = false,
                    Tok::Text(s) if inside => cell.push_str(&decode(s)),
                    Tok::Close("c") => {
                        let v = if kind == "s" { cell.trim().parse::<usize>().ok().and_then(|n| shared.get(n).cloned()).unwrap_or_default() } else { cell.clone() };
                        row.push(v);
                    }
                    Tok::Close("row") => {
                        let r = std::mem::take(&mut row).join("\t").trim_end().to_string();
                        if !r.is_empty() {
                            rows.push(r);
                        }
                    }
                    _ => {}
                });
                format!("## Sheet: {}\n{}", names.get(i).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| (i + 1).to_string()), rows.join("\n"))
            }).collect::<Vec<_>>().join("\n\n")
        }
        _ => {
            // OpenDocument: every paragraph and heading's text, spans included.
            let x = get("content.xml")?;
            let (mut out, mut line, mut depth) = (vec![], String::new(), 0usize);
            walk(&x, |t| match t {
                Tok::Open("text:p" | "text:h", _, false) => depth += 1,
                Tok::Open("text:p" | "text:h", _, true) => out.push(String::new()),
                Tok::Close("text:p" | "text:h") => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        out.push(std::mem::take(&mut line));
                    }
                }
                Tok::Open("text:s", _, _) if depth > 0 => line.push(' '),
                Tok::Open("text:tab", _, _) if depth > 0 => line.push('\t'),
                Tok::Open("text:line-break", _, _) if depth > 0 => line.push('\n'),
                Tok::Text(s) if depth > 0 => line.push_str(&decode(s)),
                _ => {}
            });
            out.join("\n")
        }
    };
    let mut t = text;
    while t.contains("\n\n\n") {
        t = t.replace("\n\n\n", "\n\n");
    }
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xml_text() {
        assert_eq!(paragraphs("<w:body><w:p><w:r><w:t>Hi &amp; </w:t></w:r><w:tab/><w:r><w:t xml:space=\"preserve\">there</w:t></w:r></w:p><w:p/><w:p><w:t>Two</w:t></w:p></w:body>", "w:p", "w:t"), "Hi & \tthere\nTwo");
        assert_eq!(decode("a&#65;&#x42;&lt;"), "aAB<");
        assert_eq!(num_in("ppt/slides/slide12.xml"), 12);
    }
}
