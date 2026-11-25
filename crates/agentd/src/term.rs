//! Terminals: a shell in a PTY per terminal id, its output streamed (coalesced ~8 ms or 32 KB)
//! and kept in a 256 KB ring for repainting.
use agentum_proto::machine::{FromAgentd, TermReq};
use anyhow::{Context, bail};
use base64::Engine;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

pub const RING: usize = 256 * 1024;
const FLUSH_BYTES: usize = 32 * 1024;
const FLUSH_AFTER: Duration = Duration::from_millis(8);

struct Term {
    cwd: String,
    cmd: Option<String>,
    master: Box<dyn MasterPty + Send>,
    input: std::sync::mpsc::Sender<Vec<u8>>,
    ring: Arc<Mutex<VecDeque<u8>>>,
    pid: Option<u32>,
}

#[derive(Clone)]
pub struct Terms {
    map: Arc<Mutex<HashMap<String, Term>>>,
    out: mpsc::Sender<FromAgentd>,
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

impl Terms {
    pub fn new(out: mpsc::Sender<FromAgentd>) -> Terms {
        Terms { map: Arc::new(Mutex::new(HashMap::new())), out }
    }

    pub fn count(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    /// Handle a request. Quick: nothing here waits on the terminal.
    pub fn handle(&self, req: TermReq) -> anyhow::Result<Value> {
        match req {
            TermReq::Open { term, cwd, cols, rows, cmd } => self.open(term, cwd, cols, rows, cmd),
            TermReq::Input { term, data } => {
                let bytes = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).context("input is not base64")?;
                let map = self.map.lock().unwrap();
                let t = map.get(&term).with_context(|| format!("no terminal {term}"))?;
                t.input.send(bytes).map_err(|_| anyhow::anyhow!("terminal {term} has ended"))?;
                Ok(Value::Null)
            }
            TermReq::Resize { term, cols, rows } => {
                let map = self.map.lock().unwrap();
                let t = map.get(&term).with_context(|| format!("no terminal {term}"))?;
                t.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
                Ok(Value::Null)
            }
            TermReq::Close { term } => {
                let t = self.map.lock().unwrap().remove(&term);
                match t {
                    Some(t) => {
                        if let Some(pid) = t.pid {
                            // SAFETY: signalling the shell we started (its process group).
                            unsafe {
                                libc::kill(-(pid as i32), libc::SIGHUP);
                                libc::kill(pid as i32, libc::SIGHUP);
                            }
                        }
                        Ok(Value::Null)
                    }
                    None => bail!("no terminal {term}"),
                }
            }
            TermReq::Replay { term } => {
                let map = self.map.lock().unwrap();
                let t = map.get(&term).with_context(|| format!("no terminal {term}"))?;
                let ring = t.ring.lock().unwrap();
                let (a, b) = ring.as_slices();
                let mut all = Vec::with_capacity(a.len() + b.len());
                all.extend_from_slice(a);
                all.extend_from_slice(b);
                Ok(json!({"term": term, "data": b64(&all)}))
            }
            TermReq::List => {
                let map = self.map.lock().unwrap();
                let mut list: Vec<Value> = map.iter().map(|(id, t)| json!({"term": id, "cwd": t.cwd, "cmd": t.cmd})).collect();
                list.sort_by(|a, b| a["term"].as_str().cmp(&b["term"].as_str()));
                Ok(Value::Array(list))
            }
        }
    }

    fn open(&self, term: String, cwd: String, cols: u16, rows: u16, cmd: Option<String>) -> anyhow::Result<Value> {
        let size = PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 };
        if let Some(t) = self.map.lock().unwrap().get(&term) {
            // Already open (a page reconnecting): just fit it.
            let _ = t.master.resize(size);
            return Ok(json!({"term": term, "existing": true}));
        }
        if !Path::new(&cwd).is_dir() {
            bail!("{cwd}: no such directory");
        }
        let pair = native_pty_system().openpty(size)?;
        // $SHELL, else the account's login shell (a system service has no $SHELL, and `sh -l`
        // would reset PATH from /etc/profile), else sh.
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).or_else(login_shell).unwrap_or_else(|| "/bin/sh".into());
        let mut cb = CommandBuilder::new(&shell);
        match &cmd {
            Some(c) => {
                cb.arg("-lc");
                cb.arg(c);
            }
            None => cb.arg("-l"),
        }
        cb.cwd(&cwd);
        cb.env("TERM", "xterm-256color");
        cb.env("COLORTERM", "truecolor");
        let mut child = pair.slave.spawn_command(cb).context("starting the shell")?;
        drop(pair.slave);
        let pid = child.process_id();
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let ring = Arc::new(Mutex::new(VecDeque::with_capacity(16 * 1024)));

        // Keystrokes, in order, on their own thread.
        let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new().name("agentd-term-in".into()).spawn(move || {
            while let Ok(b) = in_rx.recv() {
                if writer.write_all(&b).is_err() || writer.flush().is_err() {
                    break;
                }
            }
        })?;

        // Output: read on a thread, coalesce on the runtime. Terminal queries the shell sends (fish
        // waits for the device-attributes reply before its first prompt) are answered here, so a
        // terminal works whether or not a page is showing it.
        let (chunk_tx, mut chunk_rx) = mpsc::channel::<Vec<u8>>(64);
        let answer_tx = in_tx.clone();
        std::thread::Builder::new().name("agentd-term-out".into()).spawn(move || {
            let mut buf = vec![0u8; 16 * 1024];
            let mut carry: Vec<u8> = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let (reply, keep) = answer_queries(&carry, &buf[..n]);
                        carry = keep;
                        if !reply.is_empty() {
                            let _ = answer_tx.send(reply);
                        }
                        if chunk_tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        })?;
        let (out, r2, id2) = (self.out.clone(), ring.clone(), term.clone());
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<Option<i32>>();
        std::thread::Builder::new().name("agentd-term-wait".into()).spawn(move || {
            let code = child.wait().ok().map(|s| s.exit_code() as i32);
            let _ = exit_tx.send(code);
        })?;
        let map = self.map.clone();
        tokio::spawn(async move {
            let mut pending: Vec<u8> = Vec::new();
            let flush = |pending: &mut Vec<u8>| {
                let data = b64(pending);
                pending.clear();
                FromAgentd::TermData { term: id2.clone(), data }
            };
            loop {
                let first = if pending.is_empty() {
                    match chunk_rx.recv().await {
                        Some(c) => Some(c),
                        None => break,
                    }
                } else {
                    match tokio::time::timeout(FLUSH_AFTER, chunk_rx.recv()).await {
                        Ok(Some(c)) => Some(c),
                        Ok(None) => {
                            let _ = out.send(flush(&mut pending)).await;
                            break;
                        }
                        Err(_) => None,
                    }
                };
                match first {
                    Some(c) => {
                        {
                            let mut r = r2.lock().unwrap();
                            r.extend(c.iter().copied());
                            let over = r.len().saturating_sub(RING);
                            r.drain(..over);
                        }
                        pending.extend_from_slice(&c);
                        if pending.len() >= FLUSH_BYTES {
                            let _ = out.send(flush(&mut pending)).await;
                        }
                    }
                    None => {
                        let _ = out.send(flush(&mut pending)).await;
                    }
                }
            }
            let code = exit_rx.await.ok().flatten();
            map.lock().unwrap().remove(&id2);
            let _ = out.send(FromAgentd::TermExit { term: id2, code }).await;
        });
        self.map.lock().unwrap().insert(term.clone(), Term { cwd, cmd, master: pair.master, input: in_tx, ring, pid });
        Ok(json!({"term": term, "pid": pid}))
    }

    /// End every terminal (agentd is stopping).
    pub fn close_all(&self) {
        let ids: Vec<String> = self.map.lock().unwrap().keys().cloned().collect();
        for t in ids {
            let _ = self.handle(TermReq::Close { term: t });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_a_command_and_replays() {
        let (tx, mut rx) = mpsc::channel(64);
        let terms = Terms::new(tx);
        terms.handle(TermReq::Open { term: "t1".into(), cwd: "/".into(), cols: 80, rows: 24, cmd: Some("echo hello-pty; read x; echo got-$x".into()) }).unwrap();
        let mut seen = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !seen.contains("hello-pty") {
            match tokio::time::timeout_at(deadline, rx.recv()).await.unwrap().unwrap() {
                FromAgentd::TermData { data, .. } => seen.push_str(&String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(data).unwrap())),
                other => panic!("{other:?}"),
            }
        }
        terms.handle(TermReq::Input { term: "t1".into(), data: b64(b"abc\n") }).unwrap();
        let mut exit = None;
        while exit.is_none() {
            match tokio::time::timeout_at(deadline, rx.recv()).await.unwrap().unwrap() {
                FromAgentd::TermData { data, .. } => seen.push_str(&String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(data).unwrap())),
                FromAgentd::TermExit { code, .. } => exit = Some(code),
                other => panic!("{other:?}"),
            }
        }
        assert!(seen.contains("got-abc"), "{seen:?}");
        assert_eq!(exit, Some(Some(0)));
        assert!(terms.handle(TermReq::Replay { term: "t1".into() }).is_err(), "gone after exit");
    }
}

/// This account's shell in /etc/passwd.
fn login_shell() -> Option<String> {
    let uid = std::fs::read_to_string("/proc/self/status").ok()?.lines().find_map(|l| l.strip_prefix("Uid:"))?.split_whitespace().next()?.to_string();
    std::fs::read_to_string("/etc/passwd").ok()?.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 7 && f[2] == uid && Path::new(f[6]).exists()).then(|| f[6].to_string())
    })
}

/// Replies to the queries a shell sends its terminal: device attributes (primary and secondary),
/// the terminal's name (XTVERSION), status, foreground/background colours (OSC 10/11) and
/// capabilities (XTGETTCAP, answered "unknown"). `carry` is the end of the previous chunk (a query
/// split across reads); returns the reply and the new carry.
pub fn answer_queries(carry: &[u8], chunk: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut data = carry.to_vec();
    data.extend_from_slice(chunk);
    let mut reply = Vec::new();
    let mut i = 0;
    let mut done = 0; // everything before this was looked at completely
    while i < data.len() {
        if data[i] != 0x1b {
            i += 1;
            done = i;
            continue;
        }
        let rest = &data[i..];
        if rest.len() < 2 {
            break;
        }
        match rest[1] {
            b'[' => {
                // CSI: parameters then a final byte (0x40..0x7e).
                let Some(end) = rest[2..].iter().position(|b| (0x40..=0x7e).contains(b)) else { break };
                let params = &rest[2..2 + end];
                let fin = rest[2 + end];
                match (params, fin) {
                    (b"" | b"0", b'c') => reply.extend_from_slice(b"\x1b[?62;22c"),
                    (b">" | b">0", b'c') => reply.extend_from_slice(b"\x1b[>1;10;0c"),
                    (b">" | b">0", b'q') => reply.extend_from_slice(b"\x1bP>|agentum\x1b\\"),
                    (b"5", b'n') => reply.extend_from_slice(b"\x1b[0n"),
                    _ => {}
                }
                i += 3 + end;
            }
            b']' => {
                // OSC, ended by BEL or ST.
                let body = &rest[2..];
                let (end, term_len) = match (body.iter().position(|b| *b == 0x07), body.windows(2).position(|w| w == b"\x1b\\")) {
                    (Some(a), Some(b)) if b < a => (b, 2),
                    (Some(a), _) => (a, 1),
                    (None, Some(b)) => (b, 2),
                    (None, None) => break,
                };
                match &body[..end] {
                    b"10;?" => reply.extend_from_slice(b"\x1b]10;rgb:d8d8/dada/e7e7\x1b\\"),
                    b"11;?" => reply.extend_from_slice(b"\x1b]11;rgb:2323/2626/3838\x1b\\"),
                    _ => {}
                }
                i += 2 + end + term_len;
            }
            b'P' => {
                // DCS, ended by ST.
                let body = &rest[2..];
                let Some(end) = body.windows(2).position(|w| w == b"\x1b\\") else { break };
                if body.starts_with(b"+q") {
                    reply.extend_from_slice(b"\x1bP0+r\x1b\\");
                }
                i += 2 + end + 2;
            }
            _ => i += 2,
        }
        done = i;
    }
    // Keep an unfinished sequence (bounded) for the next read.
    let keep = if done < data.len() && data.len() - done <= 256 { data[done..].to_vec() } else { Vec::new() };
    (reply, keep)
}

#[cfg(test)]
mod query_tests {
    use super::answer_queries;
    #[test]
    fn answers_what_fish_asks() {
        let out = b"hi\x1b[>0q\x1b]11;?\x1b\\\x1bP+q696e646e\x1b\\\x1b[c";
        let (r, keep) = answer_queries(b"", out);
        assert!(keep.is_empty());
        let r = String::from_utf8_lossy(&r);
        assert!(r.contains("\x1b[?62;22c") && r.contains("\x1bP>|agentum") && r.contains("]11;rgb:") && r.contains("\x1bP0+r"));
        // Split across two reads.
        let (r1, keep) = answer_queries(b"", b"x\x1b[");
        assert!(r1.is_empty() && keep == b"\x1b[");
        let (r2, _) = answer_queries(&keep, b"c done");
        assert_eq!(r2, b"\x1b[?62;22c");
        // Ordinary colours and moves get no answer.
        assert!(answer_queries(b"", b"\x1b[31mred\x1b[0m\x1b[2J\x1b]0;title\x07").0.is_empty());
    }
}
