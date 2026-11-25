//! A one-shot fake hub with agentd in-process, against the real agent CLIs on this machine.
//!
//!   cargo run -p agentd --example hub_probe -- [claude-code|codex|opencode ...] \
//!       [--model AGENT=MODEL] [--cwd DIR] [--text "Reply with the word ok"]
//!
//! For each agent it lists the models, starts a thread with the text and prints every event
//! until the turn ends. Permission prompts are allowed. Data goes to a temp dir; the agents use
//! your real HOME (their logins).
use agentd::config::Config;
use agentum_proto::machine::{FromAgentd, Reply, Req, ToAgentd};
use agentum_proto::{AgentKind, EventKind, Input, Permission, ThreadSpec, ThreadState};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

async fn send(ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, m: ToAgentd) -> anyhow::Result<()> {
    ws.send(Message::text(serde_json::to_string(&m)?)).await?;
    Ok(())
}

/// Just a hub on `port`: prints each agentd's Hello, acks events, answers pings. For measuring
/// a separately started agentd (`agentd run` with `hub = "ws://127.0.0.1:PORT/agentd"`).
async fn hub_only(port: u16) -> anyhow::Result<()> {
    let l = TcpListener::bind(("127.0.0.1", port)).await?;
    println!("hub on ws://127.0.0.1:{port}/agentd");
    loop {
        let (stream, _) = l.accept().await?;
        tokio::spawn(async move {
            let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return };
            while let Some(Ok(m)) = ws.next().await {
                let Message::Text(t) = m else { continue };
                match serde_json::from_str::<FromAgentd>(t.as_str()) {
                    Ok(FromAgentd::Hello(h)) | Ok(FromAgentd::Update(h)) => {
                        println!("hello/update: {} {} agents={} projects={} threads={} rss={} MB", h.machine, h.version, h.agents.iter().filter(|a| a.installed).count(), h.projects.len(), h.threads.len(), h.stats.rss_mb)
                    }
                    Ok(FromAgentd::Events { thread, events }) => {
                        if let Some(last) = events.last() {
                            let _ = send(&mut ws, ToAgentd::Ack { thread, seq: last.seq }).await;
                        }
                    }
                    Ok(other) => println!("{other:?}"),
                    Err(e) => println!("bad frame: {e}"),
                }
            }
            println!("agentd disconnected");
        });
    }
}

fn arg(args: &[String], name: &str) -> Vec<String> {
    args.iter().enumerate().filter(|(_, a)| *a == name).filter_map(|(i, _)| args.get(i + 1).cloned()).collect()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_max_level(tracing::Level::WARN).with_writer(std::io::stderr).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(port) = arg(&args, "--hub-only").pop() {
        return hub_only(port.parse()?).await;
    }
    let mut agents: Vec<AgentKind> = args.iter().filter_map(|a| AgentKind::parse(a)).collect();
    if agents.is_empty() {
        agents = AgentKind::ALL.to_vec();
    }
    let models: HashMap<String, String> = arg(&args, "--model").into_iter().filter_map(|m| m.split_once('=').map(|(a, b)| (a.to_string(), b.to_string()))).collect();
    let text = arg(&args, "--text").pop().unwrap_or_else(|| "Reply with the word ok".into());
    let cwd = match arg(&args, "--cwd").pop() {
        Some(c) => c,
        None => {
            let d = std::env::temp_dir().join("agentd-probe-repo");
            std::fs::create_dir_all(&d)?;
            if !d.join(".git").exists() {
                std::process::Command::new("git").args(["init", "-q"]).current_dir(&d).status()?;
            }
            d.to_string_lossy().into_owned()
        }
    };

    let l = TcpListener::bind("127.0.0.1:0").await?;
    let port = l.local_addr()?.port();
    let data = std::env::temp_dir().join(format!("agentd-probe-{}", std::process::id()));
    let cfg = Config { hub: format!("ws://127.0.0.1:{port}/agentd"), token: "probe".into(), name: "probe".into(), data: Some(data.to_string_lossy().into()), idle_minutes: 10, ..Default::default() };
    let t0 = Instant::now();
    let app = agentd::App::new(cfg).await?;
    println!("agentd up in {:?} (scanned agents and projects)", t0.elapsed());
    tokio::spawn(agentd::hub::run(app.clone()));

    let (stream, _) = l.accept().await?;
    let mut ws = tokio_tungstenite::accept_async(stream).await?;
    let hello = loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => break serde_json::from_str::<FromAgentd>(t.as_str())?,
            Some(Ok(_)) => continue,
            other => anyhow::bail!("connection ended: {other:?}"),
        }
    };
    let FromAgentd::Hello(h) = hello else { anyhow::bail!("no hello") };
    println!("hello: {} on {} ({} {}), {} projects, rss {} MB", h.machine, h.hostname, h.os, h.arch, h.projects.len(), h.stats.rss_mb);
    for a in &h.agents {
        println!("  {:<12} installed={} version={:?} path={:?}", a.agent.id(), a.installed, a.version, a.path);
    }

    let mut id = 0u64;
    for agent in agents {
        println!("\n=== {} ===", agent.label());
        id += 1;
        send(&mut ws, ToAgentd::Req { id, req: Req::Models { agent } }).await?;
        let thread = format!("t_probe{:05}", id);
        let spec = ThreadSpec { agent: Some(agent), model: models.get(agent.id()).cloned(), effort: None, permission: Permission::Ask, cwd: cwd.clone() };
        let started = Instant::now();
        send(&mut ws, ToAgentd::Req { id: id + 1000, req: Req::Start { thread: thread.clone(), spec, resume: None, input: Some(Input::text(&text)), last_seq: 0 } }).await?;
        let deadline = Instant::now() + Duration::from_secs(240);
        let mut turn_over = false;
        let mut finished = false;
        while !finished && Instant::now() < deadline {
            let msg = tokio::time::timeout(Duration::from_secs(240), ws.next()).await?;
            let Some(Ok(Message::Text(t))) = msg else { continue };
            let m: FromAgentd = serde_json::from_str(t.as_str())?;
            match m {
                FromAgentd::Reply { id: rid, result } if rid == id => match result {
                    Reply::Ok(v) => {
                        let list: Vec<String> = v.as_array().into_iter().flatten().take(8).map(|m| format!("{}{}", m["id"].as_str().unwrap_or(""), if m["default"] == true { "*" } else { "" })).collect();
                        println!("models ({}): {} …", v.as_array().map(|a| a.len()).unwrap_or(0), list.join(", "));
                    }
                    Reply::Err(e) => println!("models: error: {e}"),
                },
                FromAgentd::Reply { result: Reply::Err(e), .. } => {
                    println!("start: error: {e}");
                    break;
                }
                FromAgentd::Events { thread: th, events } if th == thread => {
                    let last = events.last().map(|e| e.seq).unwrap_or(0);
                    for e in &events {
                        let mut j = serde_json::to_value(&e.kind)?;
                        if let Some(o) = j.as_object_mut() {
                            o.remove("input");
                        }
                        let s = j.to_string();
                        println!("  {:>3} +{:>6}ms {}", e.seq, started.elapsed().as_millis(), agentd::util::truncate(&s, 300));
                        if let EventKind::Ask { id: ask, .. } = &e.kind {
                            send(&mut ws, ToAgentd::Req { id: id + 2000, req: Req::Answer { thread: thread.clone(), ask: ask.clone(), option: "allow".into(), text: None } }).await?;
                        }
                        if matches!(e.kind, EventKind::TurnDone { .. }) {
                            turn_over = true;
                        }
                        if turn_over && matches!(e.kind, EventKind::State { state: ThreadState::Idle | ThreadState::Error | ThreadState::Interrupted }) {
                            turn_over = false;
                            send(&mut ws, ToAgentd::Ack { thread: thread.clone(), seq: last }).await?;
                            println!("  turn over in {:?}; rss now {} MB", started.elapsed(), agentd::machine::rss_kb() / 1024);
                            send(&mut ws, ToAgentd::Req { id: id + 3000, req: Req::Sleep { thread: thread.clone() } }).await?;
                            finished = true;
                        }
                    }
                    send(&mut ws, ToAgentd::Ack { thread: thread.clone(), seq: last }).await?;
                }
                _ => {}
            }
        }
        id += 1;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = std::fs::remove_dir_all(&data);
    Ok(())
}
