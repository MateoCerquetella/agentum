//! The one WebSocket to the hub: Hello first, then requests in, replies/events/terminal output
//! out. Reconnects forever with backoff (0.5 s → 30 s, jittered); every reconnect resends what
//! the hub hasn't acked.
use crate::App;
use agentum_proto::machine::{FromAgentd, Reply, ToAgentd};
use anyhow::Context;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

const MIN_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Events per frame, at most.
const BATCH: usize = 500;
const PING_EVERY: Duration = Duration::from_secs(20);
/// No frame from the hub for this long: the connection is dead.
const SILENT: Duration = Duration::from_secs(70);

fn jitter(d: Duration) -> Duration {
    // ±20%, from the clock's low bits (good enough to spread reconnects).
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.subsec_nanos()).unwrap_or(0);
    let f = 0.8 + (n % 1000) as f64 / 1000.0 * 0.4;
    d.mul_f64(f)
}

pub fn tls_config(ca: Option<&str>) -> anyhow::Result<rustls::ClientConfig> {
    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::pem::PemObject;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = ca {
        let path = crate::util::expand(path);
        let mut n = 0;
        for c in CertificateDer::pem_file_iter(&path).with_context(|| format!("reading CA {}", path.display()))? {
            roots.add(c.with_context(|| format!("parsing CA {}", path.display()))?)?;
            n += 1;
        }
        anyhow::ensure!(n > 0, "{}: no certificates", path.display());
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Ok(rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions()?.with_root_certificates(roots).with_no_client_auth())
}

pub async fn run(app: Arc<App>) {
    let mut backoff = MIN_BACKOFF;
    loop {
        let t0 = Instant::now();
        match session(&app).await {
            Ok(()) => tracing::info!("hub closed the connection"),
            Err(e) => tracing::warn!("hub: {e:#}"),
        }
        if app.is_connected() {
            app.set_connected(false);
        }
        if t0.elapsed() > Duration::from_secs(30) {
            backoff = MIN_BACKOFF;
        }
        let wait = jitter(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
        // Meanwhile, frames that can't wait (replies, terminal output) are dropped; events are kept.
        let mut rx = app.take_out_rx().await;
        let sleep = tokio::time::sleep(wait);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                m = rx.recv() => if m.is_none() { return },
            }
        }
    }
}

async fn session(app: &Arc<App>) -> anyhow::Result<()> {
    let mut req = app.cfg.hub.as_str().into_client_request().context("bad hub URL")?;
    req.headers_mut().insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", app.cfg.token)).context("bad token")?);
    req.headers_mut().insert("User-Agent", HeaderValue::from_static(concat!("agentd/", env!("CARGO_PKG_VERSION"))));
    let connector = if app.cfg.hub.starts_with("wss:") { Some(tokio_tungstenite::Connector::Rustls(Arc::new(tls_config(app.cfg.ca.as_deref())?))) } else { None };
    let mut wscfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
    wscfg.max_message_size = Some(64 << 20);
    wscfg.max_frame_size = Some(64 << 20);
    // Resolve and connect ourselves (see resolve.rs); TLS and the handshake still use the name.
    let host = req.uri().host().context("no host in the hub URL")?.to_string();
    let port = req.uri().port_u16().unwrap_or(if app.cfg.hub.starts_with("wss:") { 443 } else { 80 });
    let connect = async {
        let addrs = crate::resolve::resolve(&host, port, app.cfg.hub_ip.as_deref()).await?;
        let mut last = None;
        for a in addrs {
            match tokio::net::TcpStream::connect(a).await {
                Ok(s) => {
                    s.set_nodelay(true).ok();
                    return Ok(tokio_tungstenite::client_async_tls_with_config(req, s, Some(wscfg), connector).await.context("handshake")?);
                }
                Err(e) => last = Some(anyhow::anyhow!("{a}: {e}")),
            }
        }
        Err::<_, anyhow::Error>(last.unwrap_or_else(|| anyhow::anyhow!("no address for {host}")))
    };
    let (ws, _) = tokio::time::timeout(Duration::from_secs(20), connect).await.context("connecting timed out")??;
    let (mut sink, mut stream) = ws.split();
    let mut out = app.take_out_rx().await;
    // Anything left over from the last connection is stale.
    while out.try_recv().is_ok() {}
    let hello = FromAgentd::Hello(app.hello());
    sink.send(Message::text(serde_json::to_string(&hello)?)).await?;
    app.outbox.rewind();
    app.set_connected(true);
    tracing::info!("connected to {}", app.cfg.hub);
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.reset();
    let mut last_rx = Instant::now();
    loop {
        // Events first: everything queued goes out before we wait again.
        while app.outbox.has_unsent() {
            for (thread, events) in app.outbox.take_unsent(BATCH) {
                let f = FromAgentd::Events { thread, events };
                sink.feed(Message::text(serde_json::to_string(&f)?)).await?;
            }
            sink.flush().await?;
        }
        tokio::select! {
            biased;
            m = stream.next() => {
                let m = match m {
                    Some(m) => m?,
                    None => return Ok(()),
                };
                last_rx = Instant::now();
                match m {
                    Message::Text(t) => incoming(app, t.as_str()),
                    Message::Close(_) => return Ok(()),
                    _ => {}
                }
            }
            m = out.recv() => {
                let Some(m) = m else { return Ok(()) };
                sink.feed(Message::text(serde_json::to_string(&m)?)).await?;
                // Take what else is already waiting, then flush once.
                let mut n = 0;
                while n < 256 {
                    match out.try_recv() {
                        Ok(m) => sink.feed(Message::text(serde_json::to_string(&m)?)).await?,
                        Err(_) => break,
                    }
                    n += 1;
                }
                sink.flush().await?;
            }
            _ = app.outbox.notify.notified() => {}
            _ = ping.tick() => {
                anyhow::ensure!(last_rx.elapsed() < SILENT, "the hub has been silent for {}s", last_rx.elapsed().as_secs());
                sink.send(Message::Ping(Default::default())).await?;
            }
        }
    }
}

fn incoming(app: &Arc<App>, text: &str) {
    let msg: ToAgentd = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            // Not the frame itself: it may carry a secret meant for a newer agentd.
            tracing::warn!("hub sent something odd ({e}), {} bytes", text.len());
            return;
        }
    };
    match msg {
        ToAgentd::Ack { thread, seq } => app.outbox.ack(&thread, seq),
        ToAgentd::Ping { ts } => {
            let _ = app.out.try_send(FromAgentd::Pong { ts });
        }
        ToAgentd::Req { id, req } => {
            let fut = app.dispatch(req);
            let out = app.out.clone();
            tokio::spawn(async move {
                let result = match fut.await {
                    Ok(v) => Reply::Ok(v),
                    Err(e) => Reply::Err(format!("{e:#}")),
                };
                let _ = out.send(FromAgentd::Reply { id, result }).await;
            });
        }
    }
}
