//! A tiny HTTP/1.1 client for talking to a local server (OpenCode): JSON requests and an SSE stream.
use anyhow::{Context, bail};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

pub struct Resp {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(&self) -> anyhow::Result<serde_json::Value> {
        if self.body.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(&self.body).context("the server's answer is not JSON")
    }
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Percent-encode a query value.
pub fn enc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

enum Body {
    Length(usize),
    Chunked { left: usize, done: bool },
    Eof,
}

struct Conn {
    r: BufReader<TcpStream>,
    body: Body,
    status: u16,
}

async fn open(addr: &str, method: &str, path: &str, auth: &str, body: Option<&[u8]>, accept: &str) -> anyhow::Result<Conn> {
    let stream = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(addr)).await.context("connect timed out")??;
    stream.set_nodelay(true).ok();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: {auth}\r\nAccept: {accept}\r\nConnection: close\r\n");
    if let Some(b) = body {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    let mut r = BufReader::with_capacity(32 * 1024, stream);
    r.get_mut().write_all(head.as_bytes()).await?;
    if let Some(b) = body {
        r.get_mut().write_all(b).await?;
    }
    r.get_mut().flush().await?;
    let mut line = String::new();
    r.read_line(&mut line).await?;
    let status: u16 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).with_context(|| format!("bad status line {line:?}"))?;
    let mut len = None;
    let mut chunked = false;
    loop {
        line.clear();
        if r.read_line(&mut line).await? == 0 {
            bail!("connection closed in headers");
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "content-length" {
                len = v.parse().ok();
            } else if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }
    let body = if chunked {
        Body::Chunked { left: 0, done: false }
    } else if let Some(n) = len {
        Body::Length(n)
    } else if status == 204 || status == 304 || method == "HEAD" {
        Body::Length(0)
    } else {
        Body::Eof
    };
    Ok(Conn { r, body, status })
}

impl Conn {
    /// The next piece of the body, or None at its end.
    async fn piece(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        match &mut self.body {
            Body::Length(0) => Ok(None),
            Body::Length(n) => {
                let mut buf = vec![0u8; (*n).min(64 * 1024)];
                let k = self.r.read(&mut buf).await?;
                if k == 0 {
                    bail!("connection closed in body");
                }
                *n -= k;
                buf.truncate(k);
                Ok(Some(buf))
            }
            Body::Eof => {
                let mut buf = vec![0u8; 64 * 1024];
                let k = self.r.read(&mut buf).await?;
                if k == 0 {
                    return Ok(None);
                }
                buf.truncate(k);
                Ok(Some(buf))
            }
            Body::Chunked { left, done } => {
                if *done {
                    return Ok(None);
                }
                if *left == 0 {
                    let mut line = String::new();
                    if self.r.read_line(&mut line).await? == 0 {
                        bail!("connection closed in chunked body");
                    }
                    let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or("").trim(), 16).with_context(|| format!("bad chunk size {line:?}"))?;
                    if size == 0 {
                        // Trailers, then the end.
                        loop {
                            line.clear();
                            if self.r.read_line(&mut line).await? == 0 || line.trim().is_empty() {
                                break;
                            }
                        }
                        *done = true;
                        return Ok(None);
                    }
                    *left = size;
                }
                let mut buf = vec![0u8; (*left).min(64 * 1024)];
                let k = self.r.read(&mut buf).await?;
                if k == 0 {
                    bail!("connection closed in chunk");
                }
                *left -= k;
                buf.truncate(k);
                if *left == 0 {
                    let mut crlf = String::new();
                    self.r.read_line(&mut crlf).await?;
                }
                Ok(Some(buf))
            }
        }
    }
}

/// One request; the whole body.
pub async fn request(addr: &str, method: &str, path: &str, auth: &str, body: Option<&serde_json::Value>) -> anyhow::Result<Resp> {
    let fut = async {
        let b = body.map(|b| b.to_string().into_bytes());
        let mut c = open(addr, method, path, auth, b.as_deref(), "application/json").await?;
        let mut out = Vec::new();
        while let Some(p) = c.piece().await? {
            out.extend_from_slice(&p);
        }
        Ok::<_, anyhow::Error>(Resp { status: c.status, body: out })
    };
    tokio::time::timeout(Duration::from_secs(120), fut).await.with_context(|| format!("{method} {path} timed out"))?
}

/// A server-sent event stream: `next()` gives each event's data.
pub struct Sse {
    c: Conn,
    buf: Vec<u8>,
    data: String,
}

pub async fn sse(addr: &str, path: &str, auth: &str) -> anyhow::Result<Sse> {
    let c = open(addr, "GET", path, auth, None, "text/event-stream").await?;
    if !(200..300).contains(&c.status) {
        bail!("event stream: HTTP {}", c.status);
    }
    Ok(Sse { c, buf: Vec::new(), data: String::new() })
}

impl Sse {
    /// The next event's data (its `data:` lines joined), or None when the stream ends.
    pub async fn next(&mut self) -> anyhow::Result<Option<String>> {
        loop {
            while let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim_end_matches(['\n', '\r']);
                if line.is_empty() {
                    if !self.data.is_empty() {
                        return Ok(Some(std::mem::take(&mut self.data)));
                    }
                } else if let Some(d) = line.strip_prefix("data:") {
                    if !self.data.is_empty() {
                        self.data.push('\n');
                    }
                    self.data.push_str(d.strip_prefix(' ').unwrap_or(d));
                }
            }
            match self.c.piece().await? {
                Some(p) => self.buf.extend_from_slice(&p),
                None => return Ok(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    async fn serve_once(reply: &'static [u8]) -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = s.read(&mut buf).await;
            s.write_all(reply).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn chunked_json() {
        let addr = serve_once(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"a\":\r\n2\r\n1}\r\n0\r\n\r\n").await;
        let r = request(&addr, "GET", "/x", "Basic x", None).await.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.json().unwrap(), serde_json::json!({"a": 1}));
    }

    #[tokio::test]
    async fn event_stream() {
        let addr = serve_once(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n1b\r\ndata: {\"type\":\"a\"}\n\ndata: {\r\nd\r\n\"type\":\"b\"}\n\n\r\n0\r\n\r\n").await;
        let mut s = sse(&addr, "/e", "Basic x").await.unwrap();
        assert_eq!(s.next().await.unwrap().as_deref(), Some("{\"type\":\"a\"}"));
        assert_eq!(s.next().await.unwrap().as_deref(), Some("{\"type\":\"b\"}"));
        assert_eq!(s.next().await.unwrap(), None);
    }

    #[test]
    fn encoding() {
        assert_eq!(enc("location[directory]"), "location%5Bdirectory%5D");
        assert_eq!(enc("/a b"), "%2Fa%20b");
    }
}
