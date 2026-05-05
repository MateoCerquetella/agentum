//! Local-PTY plumbing for the lazygit side pane.
//!
//! `agentum serve` provides remote sessions over WebSocket; this module
//! handles the *local* case — spawning a child process under our own
//! pseudo-terminal so we can render its output via vt100 and forward
//! keystrokes to it. Used today only by the lazygit extension.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result, anyhow};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::mpsc;
use vt100::{Parser, Screen};

const SCROLLBACK: usize = 4096;

/// Bytes streamed off the local PTY's master end.
pub enum PtyMsg {
    Bytes(Vec<u8>),
    Closed,
}

/// A locally-spawned child process attached to a pseudoterminal.
///
/// Kept simple on purpose: one persistent writer taken at spawn time
/// (portable_pty's `take_writer` may only be called once), one reader
/// thread that pumps bytes into an mpsc, and a vt100 `Parser` that the UI
/// reads from each frame. Dropping the struct kills the child.
pub struct LocalPty {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    parser: Parser,
    rows: u16,
    cols: u16,
}

impl LocalPty {
    /// Spawn `binary` with `args` in `cwd`, sized to `rows x cols`.
    pub fn spawn(
        binary: &str,
        args: &[String],
        cwd: &PathBuf,
        rows: u16,
        cols: u16,
        sink: mpsc::UnboundedSender<PtyMsg>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow!("openpty failed: {e}"))?;

        let mut cmd = CommandBuilder::new(binary);
        for a in args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);
        // Make sure embedded TUIs render well: a sane terminfo + colour.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawn {binary}"))?;

        // Dropping the slave on this side hands ownership entirely to the
        // child; the master is the only thing we still talk to.
        drop(pair.slave);

        let master = Arc::new(Mutex::new(pair.master));
        let child = Arc::new(Mutex::new(child));

        // Reader thread + persistent writer: take both up-front. portable_pty
        // only allows `take_writer` to be called once, so caching it here is
        // mandatory — re-taking on each write fails with "cannot take writer
        // more than once" after the first keystroke.
        let (reader, writer) = match master.lock() {
            Ok(m) => {
                let r = m
                    .try_clone_reader()
                    .map_err(|e| anyhow!("clone pty reader: {e}"))?;
                let w = m
                    .take_writer()
                    .map_err(|e| anyhow!("take pty writer: {e}"))?;
                (r, w)
            }
            Err(_) => return Err(anyhow!("pty master mutex poisoned")),
        };
        let writer = Arc::new(Mutex::new(writer));
        let mut reader = reader;
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = sink.send(PtyMsg::Closed);
                        break;
                    }
                    Ok(n) => {
                        if sink.send(PtyMsg::Bytes(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = sink.send(PtyMsg::Closed);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            master,
            writer,
            child,
            parser: Parser::new(rows.max(1), cols.max(1), SCROLLBACK),
            rows: rows.max(1),
            cols: cols.max(1),
        })
    }

    pub fn screen(&self) -> &Screen {
        self.parser.screen()
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        if let Ok(m) = self.master.lock() {
            let _ = m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        self.parser.set_size(rows, cols);
        self.rows = rows;
        self.cols = cols;
    }

    /// Forward bytes to the child's stdin (its PTY master).
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("pty writer mutex poisoned"))?;
        writer.write_all(bytes).context("write to pty")?;
        writer.flush().context("flush pty writer")?;
        Ok(())
    }

    /// Has the child exited (without blocking)?
    pub fn finished(&self) -> bool {
        match self.child.lock() {
            Ok(mut c) => matches!(c.try_wait(), Ok(Some(_))),
            Err(_) => true,
        }
    }
}

impl Drop for LocalPty {
    fn drop(&mut self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
