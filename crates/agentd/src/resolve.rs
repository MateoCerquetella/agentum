//! Finding the hub's address.
//!
//! The static (musl) build resolves through /etc/hosts and /etc/resolv.conf only, and musl gives
//! up on a name when either its A or its AAAA query says NXDOMAIN, which some LAN resolvers
//! (AdGuard rewrites) answer for AAAA. So when the normal lookup fails we ask the nameservers
//! ourselves, A only. `hub_ip` in the config skips all of this.
use anyhow::{Context, bail};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

pub async fn resolve(host: &str, port: u16, fixed: Option<&str>) -> anyhow::Result<Vec<SocketAddr>> {
    if let Some(ip) = fixed {
        let ip: IpAddr = ip.trim().parse().with_context(|| format!("hub_ip {ip:?} is not an IP address"))?;
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let first = match tokio::net::lookup_host((host, port)).await {
        Ok(a) => {
            let v: Vec<SocketAddr> = a.collect();
            if !v.is_empty() {
                return Ok(v);
            }
            anyhow::anyhow!("no addresses")
        }
        Err(e) => e.into(),
    };
    for ns in nameservers() {
        match query_a(ns, host).await {
            Ok(ips) if !ips.is_empty() => {
                tracing::debug!("{host}: {ips:?} from {ns} (A only)");
                return Ok(ips.into_iter().map(|ip| SocketAddr::new(IpAddr::V4(ip), port)).collect());
            }
            Ok(_) => {}
            Err(e) => tracing::debug!("{host} via {ns}: {e}"),
        }
    }
    bail!("can't resolve {host}: {first}")
}

/// Nameservers of /etc/resolv.conf, then systemd-resolved's real upstreams.
fn nameservers() -> Vec<IpAddr> {
    let mut out = Vec::new();
    for f in ["/etc/resolv.conf", "/run/systemd/resolve/resolv.conf"] {
        for l in std::fs::read_to_string(f).unwrap_or_default().lines() {
            if let Some(ip) = l.trim().strip_prefix("nameserver").and_then(|r| r.trim().split('%').next()?.parse::<IpAddr>().ok())
                && !out.contains(&ip)
            {
                out.push(ip);
            }
        }
    }
    out
}

pub fn build_query(id: u16, host: &str) -> Vec<u8> {
    let mut q = Vec::with_capacity(32 + host.len());
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // RD; one question
    for label in host.trim_end_matches('.').split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]); // root, type A, class IN
    q
}

fn skip_name(b: &[u8], mut i: usize) -> Option<usize> {
    loop {
        let l = *b.get(i)? as usize;
        if l == 0 {
            return Some(i + 1);
        }
        if l & 0xc0 == 0xc0 {
            return Some(i + 2);
        }
        i += 1 + l;
    }
}

/// The A records of a DNS answer to query `id`.
pub fn parse_answer(id: u16, b: &[u8]) -> anyhow::Result<Vec<Ipv4Addr>> {
    if b.len() < 12 || u16::from_be_bytes([b[0], b[1]]) != id {
        bail!("not our answer");
    }
    let rcode = b[3] & 0x0f;
    if rcode != 0 {
        bail!("rcode {rcode}");
    }
    let qd = u16::from_be_bytes([b[4], b[5]]);
    let an = u16::from_be_bytes([b[6], b[7]]);
    let mut i = 12;
    for _ in 0..qd {
        i = skip_name(b, i).context("bad question")? + 4;
    }
    let mut out = Vec::new();
    for _ in 0..an {
        i = skip_name(b, i).context("bad answer")?;
        let h = b.get(i..i + 10).context("short answer")?;
        let (ty, len) = (u16::from_be_bytes([h[0], h[1]]), u16::from_be_bytes([h[8], h[9]]) as usize);
        let data = b.get(i + 10..i + 10 + len).context("short data")?;
        if ty == 1 && len == 4 {
            out.push(Ipv4Addr::new(data[0], data[1], data[2], data[3]));
        }
        i += 10 + len;
    }
    Ok(out)
}

async fn query_a(ns: IpAddr, host: &str) -> anyhow::Result<Vec<Ipv4Addr>> {
    let bind: SocketAddr = if ns.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
    let sock = UdpSocket::bind(bind).await?;
    let id = u16::from_str_radix(&crate::util::random_hex(2), 16).unwrap_or(0x4a6b);
    sock.send_to(&build_query(id, host), SocketAddr::new(ns, 53)).await?;
    let mut buf = vec![0u8; 1500];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let (n, from) = tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await.context("no answer")??;
        if from.ip() != ns {
            continue;
        }
        if let Ok(ips) = parse_answer(id, &buf[..n]) {
            return Ok(ips);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_answer() {
        let q = build_query(0x1234, "hub.example.lan");
        assert_eq!(&q[12..], b"\x03hub\x07example\x03lan\x00\x00\x01\x00\x01");
        // The answer: our header with QR, the question, one A record by name pointer.
        let mut a = q.clone();
        a[2] = 0x81;
        a[3] = 0x80;
        a[7] = 1;
        a.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 10, 0, 4, 10, 0, 0, 7]);
        assert_eq!(parse_answer(0x1234, &a).unwrap(), vec![Ipv4Addr::new(10, 0, 0, 7)]);
        assert!(parse_answer(0x9999, &a).is_err());
        a[3] = 0x83; // NXDOMAIN
        assert!(parse_answer(0x1234, &a).is_err());
    }

    #[tokio::test]
    async fn fixed_and_literal_addresses() {
        assert_eq!(resolve("hub.lan", 443, Some("10.0.0.2")).await.unwrap(), vec!["10.0.0.2:443".parse().unwrap()]);
        assert_eq!(resolve("127.0.0.1", 80, None).await.unwrap(), vec!["127.0.0.1:80".parse().unwrap()]);
        assert!(resolve("hub.lan", 1, Some("nope")).await.is_err());
    }
}
