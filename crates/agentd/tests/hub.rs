//! agentd against an in-process fake hub: the real binary, a fake `claude` (tests/fake/claude),
//! a throwaway HOME with one git repo.
use agentum_proto::machine::{FromAgentd, FsReq, GitReq, Hello, Reply, Req, TermReq, ToAgentd};
use agentum_proto::{AgentKind, Event, EventKind, Input, Permission, SendMode, StopReason, ThreadSpec, ThreadState};
use base64::Engine;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

const T: Duration = Duration::from_secs(20);
const THREAD: &str = "t_test000001";

/// One agentd connection, as the hub sees it.
struct Conn {
    auth: String,
    tx: mpsc::UnboundedSender<Option<ToAgentd>>,
    rx: mpsc::UnboundedReceiver<FromAgentd>,
    buf: VecDeque<FromAgentd>,
    next_id: u64,
}

impl Conn {
    async fn recv(&mut self) -> FromAgentd {
        tokio::time::timeout(T, self.rx.recv()).await.expect("agentd went quiet").expect("connection closed")
    }

    /// The first message (buffered or new) that `f` accepts; the others stay buffered.
    async fn wait(&mut self, what: &str, mut f: impl FnMut(&FromAgentd) -> bool) -> FromAgentd {
        if let Some(i) = self.buf.iter().position(&mut f) {
            return self.buf.remove(i).unwrap();
        }
        let deadline = tokio::time::Instant::now() + T;
        loop {
            let m = tokio::time::timeout_at(deadline, self.rx.recv()).await.unwrap_or_else(|_| panic!("timed out waiting for {what}; have {:?}", self.buf)).expect("connection closed");
            if f(&m) {
                return m;
            }
            self.buf.push_back(m);
        }
    }

    fn send(&self, m: ToAgentd) {
        self.tx.send(Some(m)).unwrap();
    }

    fn close(&self) {
        let _ = self.tx.send(None);
    }

    async fn req(&mut self, req: Req) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(ToAgentd::Req { id, req });
        match self.wait(&format!("reply {id}"), |m| matches!(m, FromAgentd::Reply { id: i, .. } if *i == id)).await {
            FromAgentd::Reply { result: Reply::Ok(v), .. } => Ok(v),
            FromAgentd::Reply { result: Reply::Err(e), .. } => Err(e),
            _ => unreachable!(),
        }
    }

    async fn ok(&mut self, req: Req) -> Value {
        let what = format!("{req:?}");
        self.req(req).await.unwrap_or_else(|e| panic!("{what}: {e}"))
    }

    /// Thread events until `done` accepts one; returns them all (in arrival order).
    async fn events_until(&mut self, mut done: impl FnMut(&Event) -> bool) -> Vec<Event> {
        let mut got = Vec::new();
        loop {
            let m = self.wait("events", |m| matches!(m, FromAgentd::Events { thread, .. } if thread == THREAD)).await;
            let FromAgentd::Events { events, .. } = m else { unreachable!() };
            let mut stop = false;
            for e in events {
                stop |= done(&e);
                got.push(e);
            }
            if stop {
                return got;
            }
        }
    }

    fn ack(&self, seq: u64) {
        self.send(ToAgentd::Ack { thread: THREAD.into(), seq });
    }
}

struct Hub {
    port: u16,
    conns: mpsc::UnboundedReceiver<Conn>,
}

impl Hub {
    async fn start() -> Hub {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let (ctx, conns) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = l.accept().await else { return };
                let auth = Arc::new(Mutex::new(String::new()));
                let a2 = auth.clone();
                let cb = move |req: &Request, resp: Response| {
                    *a2.lock().unwrap() = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    Ok(resp)
                };
                let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, cb).await else { continue };
                let (mut sink, mut stream) = ws.split();
                let (tx, mut out_rx) = mpsc::unbounded_channel::<Option<ToAgentd>>();
                let (in_tx, rx) = mpsc::unbounded_channel();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            m = out_rx.recv() => match m {
                                Some(Some(m)) => { if sink.send(Message::text(serde_json::to_string(&m).unwrap())).await.is_err() { return } }
                                // Drop the connection without a goodbye.
                                _ => return,
                            },
                            m = stream.next() => match m {
                                Some(Ok(Message::Text(t))) => { let _ = in_tx.send(serde_json::from_str::<FromAgentd>(t.as_str()).expect("agentd sent bad JSON")); }
                                Some(Ok(_)) => {}
                                _ => return,
                            },
                        }
                    }
                });
                let a = auth.lock().unwrap().clone();
                let _ = ctx.send(Conn { auth: a, tx, rx, buf: VecDeque::new(), next_id: 0 });
            }
        });
        Hub { port, conns }
    }

    async fn conn(&mut self) -> Conn {
        tokio::time::timeout(T, self.conns.recv()).await.expect("agentd did not connect").unwrap()
    }
}

struct Agentd {
    home: PathBuf,
    child: Child,
}

impl Agentd {
    fn spawn(home: &Path) -> Agentd {
        let child = Command::new(env!("CARGO_BIN_EXE_agentd"))
            .arg("run")
            .env("HOME", home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("AGENTD_CONFIG")
            .env("AGENTD_LOG", "debug")
            .env("FAKE_CLAUDE_LOG", home.join("claude.log"))
            .env("SHELL", "/bin/sh")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::OpenOptions::new().create(true).append(true).open(home.join("agentd.log")).unwrap())
            .spawn()
            .unwrap();
        Agentd { home: home.to_path_buf(), child }
    }
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Agentd {
    fn drop(&mut self) {
        self.kill();
        if std::thread::panicking() {
            eprintln!("--- agentd.log ---\n{}", std::fs::read_to_string(self.home.join("agentd.log")).unwrap_or_default());
        }
    }
}

fn sh(dir: &Path, cmd: &str) {
    let st = Command::new("sh").arg("-c").arg(cmd).current_dir(dir).status().unwrap();
    assert!(st.success(), "{cmd}");
}

/// A HOME with a config pointing at the hub and a project with one commit.
fn setup(port: u16) -> (PathBuf, PathBuf) {
    let home = std::env::temp_dir().join(format!("agentd-it-{}-{}", std::process::id(), port));
    let _ = std::fs::remove_dir_all(&home);
    let repo = home.join("code/demo");
    std::fs::create_dir_all(&repo).unwrap();
    sh(&repo, "git init -q -b main && git config user.email t@example.com && git config user.name Test && echo hello > README.md && git add . && git commit -qm first");
    let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake/claude");
    let cfg = format!(
        "hub = \"ws://127.0.0.1:{port}/agentd\"\ntoken = \"test-token\"\nname = \"testbox\"\nidle_minutes = 1\nclaude = \"{}\"\ncodex = \"/nonexistent/codex\"\nopencode = \"/nonexistent/opencode\"\n",
        fake.display()
    );
    std::fs::create_dir_all(home.join(".config/agentum")).unwrap();
    std::fs::write(home.join(".config/agentum/agentd.toml"), cfg).unwrap();
    (home, repo.canonicalize().unwrap())
}

async fn hello(c: &mut Conn) -> Hello {
    match c.recv().await {
        FromAgentd::Hello(h) => h,
        other => panic!("first frame is not Hello: {other:?}"),
    }
}

fn kinds(evs: &[Event]) -> Vec<String> {
    evs.iter().map(|e| serde_json::to_value(&e.kind).unwrap()["k"].as_str().unwrap().to_string()).collect()
}

fn assert_contiguous(evs: &[Event], from: u64) {
    let seqs: Vec<u64> = evs.iter().map(|e| e.seq).collect();
    let want: Vec<u64> = (from..from + evs.len() as u64).collect();
    assert_eq!(seqs, want, "seqs must count up from {from} with no gaps: {:?}", kinds(evs));
}

fn input(t: &str) -> Input {
    Input::text(t)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agentd_against_a_fake_hub() {
    let mut hub = Hub::start().await;
    let (home, repo) = setup(hub.port);
    let mut agentd = Agentd::spawn(&home);
    let mut c = hub.conn().await;
    assert_eq!(c.auth, "Bearer test-token");

    // Hello: who we are, the fake claude, the project.
    let h = hello(&mut c).await;
    assert_eq!(h.protocol, agentum_proto::PROTOCOL);
    assert_eq!(h.machine, "testbox");
    let claude = h.agents.iter().find(|a| a.agent == AgentKind::ClaudeCode).unwrap();
    assert!(claude.installed);
    assert_eq!(claude.version.as_deref(), Some("9.9.9"));
    assert!(!h.agents.iter().find(|a| a.agent == AgentKind::Codex).unwrap().installed);
    let p = h.projects.iter().find(|p| p.name == "demo").expect("the demo repo is listed");
    assert_eq!(p.branch.as_deref(), Some("main"));
    assert!(h.threads.is_empty());
    assert!(h.stats.cpus > 0 && h.stats.rss_mb > 0);

    // A thread: start with input, stream, finish.
    let spec = ThreadSpec { agent: Some(AgentKind::ClaudeCode), model: None, effort: None, permission: Permission::Ask, cwd: repo.to_string_lossy().into(), instructions: None };
    c.ok(Req::Start { thread: THREAD.into(), spec: spec.clone(), resume: None, input: Some(input("hello")), last_seq: 0 }).await;
    let evs = c.events_until(|e| matches!(e.kind, EventKind::State { state: ThreadState::Idle })).await;
    assert_contiguous(&evs, 1);
    assert_eq!(kinds(&evs), vec!["user", "state", "turn-started", "state", "session", "delta", "message", "usage", "turn-done", "state"]);
    assert!(matches!(&evs[4].kind, EventKind::Session { session_id, .. } if session_id == "fake-session-1"));
    assert!(matches!(&evs[8].kind, EventKind::TurnDone { turn: 1, stop: StopReason::End, .. }));
    let mut last = evs.last().unwrap().seq;
    c.ack(last);

    // A permission prompt, answered.
    c.ok(Req::Send { thread: THREAD.into(), input: input("ask please"), mode: SendMode::Queue }).await;
    let evs = c.events_until(|e| matches!(e.kind, EventKind::State { state: ThreadState::Waiting })).await;
    assert_contiguous(&evs, last + 1);
    let ask = evs.iter().find_map(|e| if let EventKind::Ask { id, title, .. } = &e.kind { Some((id.clone(), title.clone())) } else { None }).unwrap();
    assert_eq!(ask.1, "Bash: touch x");
    let mut evs2 = Vec::new();
    assert!(c.req(Req::Answer { thread: THREAD.into(), ask: "nope".into(), option: "allow".into(), text: None }).await.is_err());
    c.ok(Req::Answer { thread: THREAD.into(), ask: ask.0.clone(), option: "allow".into(), text: None }).await;
    evs2.extend(c.events_until(|e| matches!(e.kind, EventKind::State { state: ThreadState::Idle })).await);
    let all: Vec<Event> = evs.into_iter().chain(evs2).collect();
    assert_contiguous(&all, last + 1);
    let k = kinds(&all);
    assert!(k.ends_with(&["state", "answered", "state", "tool-done", "delta", "message", "usage", "turn-done", "state"].map(String::from)), "{k:?}");
    last = all.last().unwrap().seq;
    c.ack(last);

    // Queued while a turn runs: User{queued}, then Dequeued when it starts.
    c.ok(Req::Send { thread: THREAD.into(), input: input("slow one"), mode: SendMode::Queue }).await;
    let r = c.ok(Req::Send { thread: THREAD.into(), input: input("after"), mode: SendMode::Queue }).await;
    assert_eq!(r["queued"], true);
    let mut second_done = false;
    let evs = c
        .events_until(|e| {
            second_done |= matches!(e.kind, EventKind::Message { ref text, .. } if text == "ok");
            second_done && matches!(e.kind, EventKind::State { state: ThreadState::Idle })
        })
        .await;
    assert_contiguous(&evs, last + 1);
    let queued_seq = evs.iter().find(|e| matches!(e.kind, EventKind::User { queued: true, .. })).unwrap().seq;
    assert!(evs.iter().any(|e| matches!(e.kind, EventKind::Dequeued { seq_of_user, dropped: false } if seq_of_user == queued_seq)));
    last = evs.last().unwrap().seq;
    c.ack(last);

    // Drop the connection mid-turn: everything unacked comes again, in order, then the rest.
    c.ok(Req::Send { thread: THREAD.into(), input: input("slow two"), mode: SendMode::Queue }).await;
    let first = c.events_until(|e| matches!(e.kind, EventKind::Delta { .. })).await;
    assert_contiguous(&first, last + 1);
    c.close();
    let mut c = hub.conn().await;
    let h = hello(&mut c).await;
    let live = h.threads.iter().find(|t| t.thread == THREAD).expect("the running thread is in Hello");
    assert!(live.last_seq >= first.last().unwrap().seq);
    let evs = c.events_until(|e| matches!(e.kind, EventKind::State { state: ThreadState::Idle })).await;
    assert_contiguous(&evs, last + 1);
    assert_eq!(evs[..first.len()], first[..], "the resent events are the same events");
    let text: String = evs.iter().filter_map(|e| if let EventKind::Delta { text, .. } = &e.kind { Some(text.as_str()) } else { None }).collect();
    assert_eq!(text, "1 2 3 4 5 6 7 8 9 10 ");
    last = evs.last().unwrap().seq;
    c.ack(last);

    // Stop: the turn ends interrupted and a queued message is dropped.
    c.ok(Req::Send { thread: THREAD.into(), input: input("slow three"), mode: SendMode::Queue }).await;
    c.ok(Req::Send { thread: THREAD.into(), input: input("never sent"), mode: SendMode::Queue }).await;
    c.events_until(|e| matches!(e.kind, EventKind::Delta { .. })).await;
    let r = c.ok(Req::Stop { thread: THREAD.into() }).await;
    assert_eq!(r["dropped"], 1);
    let evs = c.events_until(|e| matches!(e.kind, EventKind::State { state: ThreadState::Interrupted })).await;
    assert!(evs.iter().any(|e| matches!(e.kind, EventKind::Dequeued { dropped: true, .. })));
    assert!(evs.iter().any(|e| matches!(e.kind, EventKind::TurnDone { stop: StopReason::Interrupted, .. })));
    last = evs.last().unwrap().seq;
    c.ack(last);

    // Models come from the CLI's initialize answer.
    let m = c.ok(Req::Models { agent: AgentKind::ClaudeCode }).await;
    assert_eq!(m[1]["id"], "fake-model");
    assert!(c.req(Req::Models { agent: AgentKind::Codex }).await.is_err());

    // Files.
    let r = repo.to_string_lossy().to_string();
    let ls = c.ok(Req::Fs(FsReq::List { path: r.clone() })).await;
    let names: Vec<&str> = ls["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec![".git", "README.md"]);
    let readme = format!("{r}/README.md");
    let f = c.ok(Req::Fs(FsReq::Read { path: readme.clone() })).await;
    assert_eq!(f["content"], "hello\n");
    let sha = f["sha"].as_str().unwrap().to_string();
    c.ok(Req::Fs(FsReq::Write { path: readme.clone(), content: "hello world\n".into(), expect_sha: Some(sha.clone()) })).await;
    let e = c.req(Req::Fs(FsReq::Write { path: readme.clone(), content: "x".into(), expect_sha: Some(sha) })).await.unwrap_err();
    assert!(e.contains("conflict"), "{e}");
    assert!(c.req(Req::Fs(FsReq::Read { path: "/etc/hostname".into() })).await.unwrap_err().contains("outside"));
    let found = c.ok(Req::Fs(FsReq::Find { root: r.clone(), query: "rdme".into(), limit: None })).await;
    assert_eq!(found["paths"], json!(["README.md"]));

    // Git.
    let st = c.ok(Req::Git(GitReq::Status { cwd: r.clone() })).await;
    assert_eq!(st["branch"], "main");
    assert_eq!(st["files"], json!([{"path": "README.md", "status": ".M"}]));
    let d = c.ok(Req::Git(GitReq::Diff { cwd: r.clone(), target: None, path: None })).await;
    assert!(d["diff"].as_str().unwrap().contains("+hello world"));
    let commit = c.ok(Req::Git(GitReq::Commit { cwd: r.clone(), message: "second".into(), all: true })).await;
    let log = c.ok(Req::Git(GitReq::Log { cwd: r.clone(), limit: Some(5), path: None })).await;
    assert_eq!(log.as_array().unwrap().len(), 2);
    assert_eq!(log[0]["sha"], commit["sha"]);
    assert_eq!(log[0]["subject"], "second");
    let show = c.ok(Req::Git(GitReq::Show { cwd: r.clone(), sha: commit["sha"].as_str().unwrap().into() })).await;
    assert!(show["diff"].as_str().unwrap().contains("+hello world"));
    let br = c.ok(Req::Git(GitReq::Branches { cwd: r.clone() })).await;
    assert_eq!(br[0]["name"], "main");
    assert_eq!(br[0]["current"], true);
    let wt = c.ok(Req::Git(GitReq::WorktreeAdd { repo: r.clone(), branch: "feat/x".into(), base: None })).await;
    let wt_path = wt["path"].as_str().unwrap().to_string();
    assert!(wt_path.ends_with(".local/share/agentum/worktrees/demo-feat-x"), "{wt_path}");
    assert!(Path::new(&wt_path).join("README.md").is_file());
    assert_eq!(wt["branch"], "feat/x");
    c.ok(Req::Git(GitReq::WorktreeRemove { path: wt_path.clone() })).await;
    assert!(!Path::new(&wt_path).exists());

    // Terminals.
    let b64 = |s: &[u8]| base64::engine::general_purpose::STANDARD.encode(s);
    c.ok(Req::Term(TermReq::Open { term: "x1".into(), cwd: r.clone(), cols: 100, rows: 30, cmd: Some("echo hi-term; read v; echo got-$v".into()) })).await;
    let mut out = String::new();
    while !out.contains("hi-term") {
        if let FromAgentd::TermData { data, .. } = c.wait("term data", |m| matches!(m, FromAgentd::TermData { term, .. } if term == "x1")).await {
            out.push_str(&String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(data).unwrap()));
        }
    }
    let list = c.ok(Req::Term(TermReq::List)).await;
    assert_eq!(list[0]["term"], "x1");
    let replay = c.ok(Req::Term(TermReq::Replay { term: "x1".into() })).await;
    assert!(String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(replay["data"].as_str().unwrap()).unwrap()).contains("hi-term"));
    c.ok(Req::Term(TermReq::Resize { term: "x1".into(), cols: 120, rows: 40 })).await;
    c.ok(Req::Term(TermReq::Input { term: "x1".into(), data: b64(b"abc\n") })).await;
    loop {
        match c.wait("term output or exit", |m| matches!(m, FromAgentd::TermData { term, .. } | FromAgentd::TermExit { term, .. } if term == "x1")).await {
            FromAgentd::TermData { data, .. } => out.push_str(&String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(data).unwrap())),
            FromAgentd::TermExit { code, .. } => {
                assert_eq!(code, Some(0));
                break;
            }
            _ => unreachable!(),
        }
    }
    assert!(out.contains("got-abc"), "{out:?}");

    // Ping.
    c.send(ToAgentd::Ping { ts: 42 });
    c.wait("pong", |m| matches!(m, FromAgentd::Pong { ts: 42 })).await;

    // agentd restarts mid-turn: the turn becomes Interrupted, the next message resumes the session.
    c.ok(Req::Send { thread: THREAD.into(), input: input("slow four"), mode: SendMode::Queue }).await;
    let evs = c.events_until(|e| matches!(e.kind, EventKind::Delta { .. })).await;
    assert_contiguous(&evs, last + 1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    agentd.kill();
    let mut agentd = Agentd::spawn(&home);
    let mut c = hub.conn().await;
    let h = hello(&mut c).await;
    assert!(h.threads.iter().any(|t| t.thread == THREAD));
    let evs = c.events_until(|e| matches!(e.kind, EventKind::State { state: ThreadState::Interrupted })).await;
    assert_contiguous(&evs, last + 1);
    assert!(evs.iter().any(|e| matches!(e.kind, EventKind::TurnDone { stop: StopReason::Interrupted, .. })));
    last = evs.last().unwrap().seq;
    c.ack(last);
    c.ok(Req::Send { thread: THREAD.into(), input: input("hello again"), mode: SendMode::Queue }).await;
    let evs = c.events_until(|e| matches!(e.kind, EventKind::State { state: ThreadState::Idle })).await;
    assert_contiguous(&evs, last + 1);
    assert!(evs.iter().any(|e| matches!(&e.kind, EventKind::Session { session_id, .. } if session_id == "fake-session-1")));
    let log = std::fs::read_to_string(home.join("claude.log")).unwrap();
    assert!(log.lines().filter(|l| l.starts_with("start")).last().unwrap().contains("resume=fake-session-1"), "resumed with --resume: {log}");
    c.ack(evs.last().unwrap().seq);

    // Sleep ends the process; the next send resumes it.
    c.ok(Req::Sleep { thread: THREAD.into() }).await;
    agentd.kill();
    if std::env::var_os("AGENTD_KEEP_TEST_HOME").is_none() {
        let _ = std::fs::remove_dir_all(&home);
    } else {
        eprintln!("kept {}", home.display());
    }
}
