//! One HTTPS GET to a public host (Claude's usage check): rustls with the public roots, HTTP/1.1,
//! the whole body. Blocking: run it on the blocking pool.
use anyhow::{Context, bail};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

/// `(status, body)`. Header values are sent as given and never logged.
pub fn get(host: &str, path: &str, headers: &[(&str, String)]) -> anyhow::Result<(u16, Vec<u8>)> {
    let cfg = Arc::new(crate::hub::tls_config(None)?);
    let name = rustls::pki_types::ServerName::try_from(host.to_string())?;
    let conn = rustls::ClientConnection::new(cfg, name)?;
    let addr = (host, 443).to_socket_addrs()?.next().with_context(|| format!("{host}: no address"))?;
    let sock = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).with_context(|| format!("connecting to {host}"))?;
    sock.set_read_timeout(Some(Duration::from_secs(20)))?;
    sock.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut tls = rustls::StreamOwned::new(conn, sock);
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept-Encoding: identity\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    tls.write_all(req.as_bytes()).with_context(|| format!("writing to {host}"))?;
    let mut raw = Vec::new();
    match tls.read_to_end(&mut raw) {
        Ok(_) => {}
        // A server that closes without TLS close_notify: what came is the answer.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !raw.is_empty() => {}
        Err(e) => return Err(anyhow::Error::from(e).context(format!("reading from {host}"))),
    }
    parse(&raw)
}

fn parse(raw: &[u8]) -> anyhow::Result<(u16, Vec<u8>)> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").context("no HTTP header in the answer")?;
    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
    let status: u16 = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).context("no HTTP status")?;
    let body = &raw[end + 4..];
    let chunked = head.lines().any(|l| l.starts_with("transfer-encoding:") && l.contains("chunked"));
    Ok((status, if chunked { dechunk(body)? } else { body.to_vec() }))
}

fn dechunk(mut b: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let i = b.windows(2).position(|w| w == b"\r\n").context("bad chunk")?;
        let size = usize::from_str_radix(String::from_utf8_lossy(&b[..i]).split(';').next().unwrap_or("").trim(), 16)?;
        b = &b[i + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size {
            bail!("short chunk");
        }
        out.extend_from_slice(&b[..size]);
        b = b.get(size + 2..).unwrap_or(&[]);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn chunked_and_plain() {
        let r = super::parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n").unwrap();
        assert_eq!((r.0, r.1.as_slice()), (200, &b"{\"a\":1}"[..]));
        let r = super::parse(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        assert_eq!((r.0, r.1.as_slice()), (401, &b"{}"[..]));
    }
}
