//! Host-aware tmux operations + the shared SSH connection builder.
//!
//! The watchdog and the server both need to sample tmux panes on a session's
//! host, which may be `Local` (run tmux directly) or `Ssh` (run tmux over an
//! `ssh` connection). The connection builder ([`ssh_command`]) and the small
//! set of read/poll ops the watchdog uses live here, in the shared lower
//! crate, so the watchdog can be host-aware without depending on
//! `agentum-server` (which depends on the watchdog — the dependency only runs
//! one way).
//!
//! `agentum-server`'s `host_runtime` imports [`ssh_command`] from here rather
//! than re-deriving the argv, so there is a single source of truth for the
//! `ssh` flags (BatchMode / ConnectTimeout / StrictHostKeyChecking / auth
//! handling — key, agent, and SSH_ASKPASS-based password, no external
//! `sshpass`).

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use agentum_core::{Host, HostKind, SshAuth};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::time::timeout;
use uuid::Uuid;

use crate::{Result, TmuxError};

/// Matches `host_runtime`'s probe budget so a hung remote can't wedge the
/// watchdog's tick loop (`tokio::time::timeout` bounds every SSH round trip).
const SSH_TIMEOUT: Duration = Duration::from_secs(12);

#[cfg(unix)]
const EMPTY_SSH_CONFIG: &str = "/dev/null";
#[cfg(windows)]
const EMPTY_SSH_CONFIG: &str = "NUL";

/// Build an OpenSSH command that cannot evaluate the user's configuration.
///
/// Agentum uses this only after it already owns the exact private ControlPath
/// the command will address. At that point routing and authentication have
/// already happened in the master, while reparsing `~/.ssh/config` can still
/// run expensive `Match exec` predicates for every channel (NetBird's detector
/// is a common multi-second example). Cold connections deliberately keep using
/// the normal config so ProxyJump, aliases, and custom connection policy remain
/// available when the master is first established.
pub fn ssh_existing_control_command() -> Command {
    let mut command = Command::new("ssh");
    command.arg("-F").arg(EMPTY_SSH_CONFIG);
    command
}

fn reusable_control_socket_exists(control_path: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        std::fs::symlink_metadata(control_path)
            .is_ok_and(|metadata| metadata.file_type().is_socket())
    }
    #[cfg(not(unix))]
    {
        Path::new(control_path).exists()
    }
}

/// Return the process-wide lifecycle mutex for one persisted host.
///
/// Host mutations and every SSH operation that depends on a persisted [`Host`]
/// revision must share this registry. In particular, callers should acquire
/// this lock *before* loading the host row and keep it through the operation;
/// that prevents an in-flight command from authenticating with stale
/// credentials after a concurrent host update has rotated its ControlMaster.
/// Weak registry entries avoid retaining one allocation forever for every host
/// UUID that has ever existed in this process.
fn host_lifecycle_lock(id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<std::sync::Mutex<HashMap<Uuid, Weak<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    let locks = LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(id, Arc::downgrade(&lock));
    lock
}

/// Acquire the shared lifecycle lease for `host_id`.
///
/// The returned owned guard is intentionally independent of application state,
/// allowing the server, watchdog, and other lower-level SSH consumers to
/// serialize against the same host PUT/delete boundary.
pub async fn acquire_host_lifecycle(host_id: Uuid) -> tokio::sync::OwnedMutexGuard<()> {
    host_lifecycle_lock(host_id).lock_owned().await
}

/// Private base dir for ControlMaster sockets: `$XDG_RUNTIME_DIR/agentum-ssh`
/// (preferred — short and user-private on Linux) else `$HOME/.agentum/ssh`.
/// Never the world-writable temp dir: the socket backs an *authenticated* SSH
/// channel, so a hijackable location is a real risk (and macOS's `$TMPDIR` is
/// long enough to blow the unix-socket path cap with a namespaced leaf).
fn control_socket_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return Some(p.join("agentum-ssh"));
        }
    }
    let home = std::env::var_os("HOME")?;
    let home = PathBuf::from(home);
    home.is_absolute()
        .then(|| home.join(".agentum").join("ssh"))
}

/// Both private roots in which an older Agentum process may still have a
/// persistent master. The preferred root can change across an upgrade (for
/// example when `XDG_RUNTIME_DIR` starts being exported), while the old ssh
/// process and its reverse forward remain alive. Keep both absolute roots,
/// deduplicated, so migration can retire either generation without scanning a
/// directory or touching any non-Agentum socket.
fn legacy_control_socket_dirs_from(
    xdg_runtime_dir: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::with_capacity(2);
    if let Some(xdg) = xdg_runtime_dir {
        let xdg = PathBuf::from(xdg);
        if xdg.is_absolute() {
            dirs.push(xdg.join("agentum-ssh"));
        }
    }
    if let Some(home) = home {
        let home = PathBuf::from(home);
        if home.is_absolute() {
            let fallback = home.join(".agentum").join("ssh");
            if !dirs.contains(&fallback) {
                dirs.push(fallback);
            }
        }
    }
    dirs
}

fn legacy_control_socket_dirs() -> Vec<PathBuf> {
    let xdg = std::env::var_os("XDG_RUNTIME_DIR");
    let home = std::env::var_os("HOME");
    legacy_control_socket_dirs_from(xdg.as_deref(), home.as_deref())
}

/// Which pooled ControlMaster a connection attaches to. Separate masters keep
/// latency-sensitive input, long-lived pane tails, and low-frequency liveness
/// observation from starving or circularly depending on one another.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SshMux {
    /// No pooling — a fresh TCP+auth connection. Used by the stale-master retry.
    Off,
    /// The interactive master (`cm-`): tmux/git/fs execs, keystrokes, input loop.
    Interactive,
    /// The streaming master (`cms-`): persistent pane-log `tail -f` channels.
    /// One shared connection for ALL a host's tails — so opening the app fires
    /// ONE connection per host instead of one-per-session (which overran sshd's
    /// `MaxStartups` and timed tails out as "[session stream closed]").
    Streaming,
    /// The observer master (`cmo-`): low-frequency pane title/log-progress
    /// probes. It must be independent of `Streaming`, because those probes are
    /// what prove that a live tail has silently stopped forwarding.
    Observer,
}

/// Short, deterministic connection revision for one persisted host record.
/// OpenSSH's `%C` hashes only endpoint/user/port, so without this namespace two
/// duplicate Agentum records can share authenticated masters. The persisted id
/// separates those records, while `updated_at` is the opaque persisted mutation
/// revision (health probes update only `last_seen_at`). Destination, auth kind,
/// and key path are also included defensively. Password bytes are deliberately
/// absent so the visible path can never become an offline password verifier;
/// every explicit host PUT closes the old revision under the host lifecycle
/// lock before the revised record is persisted.
fn host_control_namespace(host: &Host) -> String {
    fn field(hash: &mut Sha256, bytes: &[u8]) {
        // Length-prefix every field so different component boundaries cannot
        // produce the same byte stream.
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }

    // Truncated SHA-256 makes it infeasible for configurable public fields to
    // force two records/revisions onto one authenticated master. Secret
    // password bytes never enter this fingerprint.
    let mut hash = Sha256::new();
    field(&mut hash, b"agentum-control-v2");
    field(&mut hash, host.id.as_bytes());
    field(
        &mut hash,
        &host.updated_at.unix_timestamp_nanos().to_le_bytes(),
    );
    match &host.kind {
        HostKind::Local => field(&mut hash, b"local"),
        HostKind::Ssh {
            user,
            hostname,
            port,
            auth,
        } => {
            field(&mut hash, b"ssh");
            field(&mut hash, user.as_bytes());
            field(&mut hash, hostname.as_bytes());
            field(&mut hash, &port.to_le_bytes());
            match auth {
                SshAuth::Agent => field(&mut hash, b"agent"),
                SshAuth::Key { path } => {
                    field(&mut hash, b"key");
                    field(&mut hash, path.as_bytes());
                }
                SshAuth::Password { .. } => field(&mut hash, b"password"),
            }
        }
    }
    // 64 truncated bits keep the leaf short while retaining a negligible
    // accidental-collision probability for persisted host records.
    hash.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `ControlPath` template for OpenSSH multiplexing, or `None` when no safe,
/// short-enough socket dir exists (then we skip multiplexing and connect fresh
/// rather than risk a too-long path or an unsafe socket). `prefix` separates
/// interactive/streaming masters; [`host_control_namespace`] separates records
/// and connection revisions. The directory is validated as a real,
/// current-owner `0700` directory before every auth mode may use it.
fn control_path_template_in(host: &Host, prefix: &str, dir: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        ensure_private_ssh_dir(dir).ok()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir).ok()?;
    }
    let template = dir
        .join(format!("{prefix}-{}", host_control_namespace(host)))
        .to_string_lossy()
        .into_owned();
    if template.len() > 100 {
        return None;
    }
    Some(template)
}

fn control_path_template_for(host: &Host, prefix: &str) -> Option<String> {
    let dir = control_socket_dir()?;
    control_path_template_in(host, prefix, &dir)
}

/// The interactive master path — a named helper kept for the tests.
#[cfg(test)]
fn control_path_template(host: &Host) -> Option<String> {
    control_path_template_for(host, "cm")
}

fn control_path_for(host: &Host, mux: SshMux) -> Option<String> {
    match mux {
        SshMux::Off => None,
        SshMux::Interactive => control_path_template_for(host, "cm"),
        SshMux::Streaming => control_path_template_for(host, "cms"),
        SshMux::Observer => control_path_template_for(host, "cmo"),
    }
}

/// ControlPath used by Agentum releases before persisted-host/revision
/// namespacing was introduced. `%C` is expanded by OpenSSH to a 40-hex digest
/// of the effective connection tuple, so the two fixed leaves still select
/// only this exact user/host/port in Agentum's private socket directory.
///
/// Account for the expansion when applying the same conservative 100-byte
/// socket-path cap as the current format. A legacy master cannot have been
/// created beyond the platform's unix-socket limit, so skipping an overlong
/// path is both safe and avoids turning an impossible migration into a startup
/// failure.
fn legacy_control_path_template_in(prefix: &str, dir: &Path) -> Option<String> {
    const OPENSSH_PERCENT_C_HEX_LEN: usize = 40;

    #[cfg(unix)]
    {
        ensure_private_ssh_dir(dir).ok()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir).ok()?;
    }
    let template = dir
        .join(format!("{prefix}-%C"))
        .to_string_lossy()
        .into_owned();
    let expanded_len = template
        .len()
        .checked_sub("%C".len())?
        .checked_add(OPENSSH_PERCENT_C_HEX_LEN)?;
    (expanded_len <= 100).then_some(template)
}

fn legacy_control_paths_for(mux: SshMux) -> Vec<String> {
    let prefix = match mux {
        SshMux::Off => return Vec::new(),
        SshMux::Interactive => "cm",
        SshMux::Streaming => "cms",
        // No Agentum release predating namespaced paths had this role.
        SshMux::Observer => return Vec::new(),
    };
    legacy_control_socket_dirs()
        .into_iter()
        .filter_map(|dir| legacy_control_path_template_in(prefix, &dir))
        .collect()
}

/// Env var the askpass helper uses to locate its unique owner-only secret file.
/// The SSH process environment contains this non-secret path, never the
/// password itself.
const ASKPASS_SECRET_ENV: &str = "AGENTUM_SSH_ASKPASS_SECRET_FILE";

/// Failsafe lifetime for password files handed to public bare-`Command`
/// callers. Captured SSH operations remove them immediately on completion,
/// timeout, or future cancellation; the bounded cleanup covers streaming/raw
/// callers whose public API cannot carry a cleanup guard.
const ASKPASS_SECRET_MAX_LIFETIME: Duration = Duration::from_secs(120);

#[cfg(unix)]
const ASKPASS_SCRIPT: &str = "#!/bin/sh\nsecret=${AGENTUM_SSH_ASKPASS_SECRET_FILE-}\n[ -n \"$secret\" ] || exit 1\n[ -f \"$secret\" ] || exit 1\nexec /bin/cat \"$secret\"\n";

#[cfg(unix)]
fn private_file_path(dir: &Path, stem: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);
    dir.join(format!(
        ".{stem}.{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Ensure `dir` is a real, current-user-owned `0700` directory and return the
/// effective uid. Rust's standard library does not expose `geteuid`, so a
/// freshly-created owner-only probe supplies the uid without trusting `$UID`
/// or invoking a PATH-resolved helper process.
#[cfg(unix)]
fn ensure_private_ssh_dir(dir: &Path) -> std::io::Result<u32> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;

    let metadata = std::fs::symlink_metadata(dir)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "SSH helper directory is not a real directory: {}",
                dir.display()
            ),
        ));
    }

    // The probe contains no data and is 0600 from creation. Besides giving us
    // the effective uid, successful creation proves the directory is writable.
    let probe_path = private_file_path(dir, "owner-probe");
    let probe = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&probe_path)?;
    let probe_metadata = probe.metadata();
    drop(probe);
    let cleanup = std::fs::remove_file(&probe_path);
    let effective_uid = probe_metadata?.uid();
    cleanup?;

    if metadata.uid() != effective_uid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "SSH helper directory {} is owned by uid {}, expected uid {effective_uid}",
                dir.display(),
                metadata.uid()
            ),
        ));
    }

    if metadata.mode() & 0o7777 != 0o700 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }

    // Re-read after chmod so a symlink/type/owner race fails closed instead of
    // handing OpenSSH a helper from an attacker-controlled directory.
    let verified = std::fs::symlink_metadata(dir)?;
    if !verified.file_type().is_dir()
        || verified.file_type().is_symlink()
        || verified.uid() != effective_uid
        || verified.mode() & 0o7777 != 0o700
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "SSH helper directory failed owner/mode verification: {}",
                dir.display()
            ),
        ));
    }
    Ok(effective_uid)
}

#[cfg(unix)]
fn private_regular_file(path: &Path, expected_uid: u32, expected_mode: u32) -> bool {
    use std::os::unix::fs::MetadataExt;

    std::fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_file()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == expected_uid
            && metadata.mode() & 0o7777 == expected_mode
            && metadata.nlink() == 1
    })
}

/// Path to a tiny SSH_ASKPASS helper that reads the password from the owner-only
/// file named by [`ASKPASS_SECRET_ENV`] — OpenSSH's askpass protocol. This is how we
/// feed a password to `ssh` non-interactively *without* the external `sshpass`
/// binary: the stock `ssh` on every modern macOS/Linux runs this helper when
/// `SSH_ASKPASS_REQUIRE=force` is set (OpenSSH 8.4+, 2020). Created `0700` on
/// demand in the same private dir as the ControlMaster sockets. Failure is
/// returned to the command builder and password authentication then fails
/// locally, before `ssh` can fall back to an interactive prompt.
#[cfg(unix)]
fn askpass_script_path_in(dir: &Path) -> std::io::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let effective_uid = ensure_private_ssh_dir(dir)?;
    let path = dir.join("askpass.sh");

    // Reuse only a regular, singly-linked file owned by this process's uid.
    // Repair an owner-controlled stale mode before reuse; symlinks and files
    // with unexpected ownership/content are atomically replaced below.
    let reusable = std::fs::symlink_metadata(&path).is_ok_and(|metadata| {
        use std::os::unix::fs::MetadataExt;
        metadata.file_type().is_file()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == effective_uid
            && metadata.nlink() == 1
            && std::fs::read_to_string(&path).is_ok_and(|content| content == ASKPASS_SCRIPT)
    });
    if reusable {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        if private_regular_file(&path, effective_uid, 0o700)
            && std::fs::read_to_string(&path).is_ok_and(|content| content == ASKPASS_SCRIPT)
        {
            return Ok(path);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "SSH askpass helper failed owner/mode verification: {}",
                path.display()
            ),
        ));
    }

    // Write to a uniquely-named temp then atomically rename into place, so a
    // concurrent ssh always sees either the old helper or the fully-written new
    // one — never a truncated/half-written file. `create_new` prevents even a
    // same-name stale temp from being followed or truncated.
    let tmp = private_file_path(dir, "askpass.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&tmp)?;
    if let Err(error) = f
        .write_all(ASKPASS_SCRIPT.as_bytes())
        .and_then(|_| f.sync_all())
    {
        drop(f);
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    drop(f);
    if let Err(error) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o700)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    if !private_regular_file(&tmp, effective_uid, 0o700) {
        let _ = std::fs::remove_file(&tmp);
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "temporary SSH askpass helper failed owner/mode verification",
        ));
    }
    if let Err(error) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    if !private_regular_file(&path, effective_uid, 0o700)
        || !std::fs::read_to_string(&path).is_ok_and(|content| content == ASKPASS_SCRIPT)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "SSH askpass helper failed final owner/mode verification: {}",
                path.display()
            ),
        ));
    }
    Ok(path)
}

#[cfg(unix)]
fn askpass_script_path() -> std::io::Result<PathBuf> {
    let dir = control_socket_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no safe runtime or home directory for SSH askpass helper",
        )
    })?;
    askpass_script_path_in(&dir)
}

/// Non-unix: the askpass helper is a POSIX shell script, so SSH_ASKPASS-based
/// password auth isn't wired there (`sshpass` was essentially never present on
/// Windows either). The caller falls back to a plain `ssh` with no helper.
#[cfg(not(unix))]
fn askpass_script_path() -> std::io::Result<PathBuf> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "SSH_ASKPASS password authentication is unavailable on this platform",
    ))
}

/// Per-command password material staged outside argv and environment. The file
/// is unique, regular, singly linked, current-owner, and `0600`. A live guard
/// removes it immediately when a captured operation completes or is canceled.
#[derive(Debug)]
struct PasswordSecret {
    path: PathBuf,
    expected_uid: u32,
    expected_device: u64,
    expected_inode: u64,
    remove_on_drop: bool,
}

#[cfg(unix)]
fn remove_password_secret(
    path: &Path,
    expected_uid: u32,
    expected_device: u64,
    expected_inode: u64,
) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    // Never follow or delete a replacement object. The containing directory is
    // current-owner 0700, but this extra check keeps cleanup fail-safe if local
    // state is unexpectedly modified after staging.
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != expected_uid
        || metadata.dev() != expected_device
        || metadata.ino() != expected_inode
        || metadata.nlink() != 1
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "refusing to remove replaced SSH password file: {}",
                path.display()
            ),
        ));
    }
    std::fs::remove_file(path)
}

#[cfg(not(unix))]
fn remove_password_secret(
    _path: &Path,
    _expected_uid: u32,
    _expected_device: u64,
    _expected_inode: u64,
) -> std::io::Result<()> {
    Ok(())
}

impl PasswordSecret {
    fn remove_now(&mut self) -> std::io::Result<()> {
        if !self.remove_on_drop {
            return Ok(());
        }
        remove_password_secret(
            &self.path,
            self.expected_uid,
            self.expected_device,
            self.expected_inode,
        )?;
        self.remove_on_drop = false;
        Ok(())
    }

    fn schedule_bounded_cleanup(mut self, after: Duration) -> std::io::Result<()> {
        let path = self.path.clone();
        let expected_uid = self.expected_uid;
        let expected_device = self.expected_device;
        let expected_inode = self.expected_inode;
        std::thread::Builder::new()
            .name("agentum-ssh-secret-cleanup".into())
            .spawn(move || {
                std::thread::sleep(after);
                if let Err(error) =
                    remove_password_secret(&path, expected_uid, expected_device, expected_inode)
                {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "could not remove staged SSH password file"
                    );
                }
            })?;
        // The cleanup thread now owns the bounded-removal obligation.
        self.remove_on_drop = false;
        Ok(())
    }
}

impl Drop for PasswordSecret {
    fn drop(&mut self) {
        if let Err(error) = self.remove_now() {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                "could not remove staged SSH password file"
            );
        }
    }
}

#[cfg(unix)]
fn stage_password_secret_in(dir: &Path, password: &str) -> std::io::Result<PasswordSecret> {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let effective_uid = ensure_private_ssh_dir(dir)?;
    let path = private_file_path(dir, "askpass-secret");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    if let Err(error) = file
        .write_all(password.as_bytes())
        .and_then(|_| file.sync_all())
    {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    drop(file);
    if let Err(error) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    if !private_regular_file(&path, effective_uid, 0o600) {
        let _ = std::fs::remove_file(&path);
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "staged SSH password file failed owner/mode verification",
        ));
    }
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
    };
    Ok(PasswordSecret {
        path,
        expected_uid: effective_uid,
        expected_device: metadata.dev(),
        expected_inode: metadata.ino(),
        remove_on_drop: true,
    })
}

#[cfg(not(unix))]
fn stage_password_secret_in(_dir: &Path, _password: &str) -> std::io::Result<PasswordSecret> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "SSH password files are unavailable on this platform",
    ))
}

struct PreparedAskpass {
    helper: PathBuf,
    secret: PasswordSecret,
}

fn prepare_password_askpass(password: &str) -> std::io::Result<PreparedAskpass> {
    let dir = control_socket_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no safe runtime or home directory for SSH password staging",
        )
    })?;
    let helper = askpass_script_path()?;
    let secret = stage_password_secret_in(&dir, password)?;
    Ok(PreparedAskpass { helper, secret })
}

/// Build the `ssh` argv for running `script` on `host`. Returns a plain tokio
/// [`Command`]; the caller drives `.output()` / `.status()`.
///
/// This is the single source of truth for our SSH connection flags. Password
/// auth is fed through OpenSSH's own SSH_ASKPASS helper (see
/// [`askpass_script_path`]) rather than an external `sshpass` binary, so the
/// watchdog (which can't depend on the server) and the server share one
/// builder and password hosts need nothing installed.
pub fn ssh_command(host: &Host, script: &str) -> Command {
    ssh_command_opts(host, script, SshMux::Interactive)
}

/// Like [`ssh_command`] but selects which pooled master (or none) the
/// connection uses. [`ssh_output`]'s retry rebuilds with [`SshMux::Off`] so a
/// stale/racing pooled master (broken-pipe / "failed to connect to new control
/// master") can't keep failing an op — the replay connects fresh instead.
pub fn ssh_command_opts(host: &Host, script: &str, mux: SshMux) -> Command {
    try_ssh_command_opts(host, script, mux)
        .and_then(PreparedSshCommand::into_public_command)
        .unwrap_or_else(|error| password_auth_preflight_failure_command(&error))
}

struct PreparedSshCommand {
    command: Command,
    password_secret: Option<PasswordSecret>,
}

impl PreparedSshCommand {
    fn into_public_command(mut self) -> std::io::Result<Command> {
        if let Some(secret) = self.password_secret.take() {
            secret.schedule_bounded_cleanup(ASKPASS_SECRET_MAX_LIFETIME)?;
        }
        Ok(self.command)
    }
}

/// Build a local command that reports an askpass preparation failure and exits
/// 255 without contacting the host or opening an interactive password prompt.
/// The public command builder cannot return a `Result` without breaking all of
/// its streaming callers, so they receive this fail-closed command; captured
/// SSH operations use [`try_ssh_command_opts`] directly and receive the I/O
/// error before spawning anything.
fn password_auth_preflight_failure_command(error: &std::io::Error) -> Command {
    #[cfg(unix)]
    {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("printf '%s\\n' \"$1\" >&2; exit 255")
            .arg("agentum-ssh-preflight")
            .arg(format!(
                "agentum: cannot prepare noninteractive SSH password helper: {error}"
            ));
        cmd
    }

    #[cfg(not(unix))]
    {
        // Deliberately invalid OpenSSH option: parsing fails with status 255
        // before any network/authentication attempt can prompt the user.
        let mut cmd = Command::new("ssh");
        cmd.arg("-o")
            .arg("AgentumAskpassPreparationFailed=yes")
            .env("AGENTUM_SSH_PREFLIGHT_ERROR", error.to_string());
        cmd
    }
}

fn try_ssh_command_opts(
    host: &Host,
    script: &str,
    mux: SshMux,
) -> std::io::Result<PreparedSshCommand> {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        auth,
    } = &host.kind
    else {
        return Ok(PreparedSshCommand {
            command: Command::new("false"),
            password_secret: None,
        });
    };

    // Password auth feeds the secret through OpenSSH's SSH_ASKPASS helper (set
    // up in the `match auth` block below), so ssh must actually prompt for it —
    // that requires BatchMode=no (BatchMode=yes suppresses the prompt entirely).
    // Key/agent auth keeps BatchMode=yes so it never blocks waiting on a prompt.
    let password_value = match auth {
        SshAuth::Password { password } if !password.trim().is_empty() => Some(password.as_str()),
        _ => None,
    };
    let password = password_value.is_some();
    // Prepare and verify the helper before constructing a password-capable ssh
    // command. Failure must never degrade to `BatchMode=no` without askpass,
    // which could read `/dev/tty` and hang an otherwise headless operation.
    let askpass = password_value.map(prepare_password_askpass).transpose()?;

    let control_path = control_path_for(host, mux);
    // A hot ControlMaster already embodies the user's routing/auth config. Skip
    // reparsing it for the channel attach; `Match exec` hooks otherwise run on
    // every title poll and fallback keystroke even though no new connection is
    // being made. A missing/stale socket keeps the normal config path so the
    // cold master can still use ProxyJump and aliases.
    let mut cmd = if control_path
        .as_deref()
        .is_some_and(reusable_control_socket_exists)
    {
        ssh_existing_control_command()
    } else {
        Command::new("ssh")
    };

    // Every command built here is machine-oriented. Never let an SSH server
    // allocate a pseudo-terminal based on client config: a PTY can change
    // buffering, line endings, signal delivery, and password prompt behavior.
    cmd.arg("-T")
        .arg("-o")
        .arg(if password {
            "BatchMode=no"
        } else {
            "BatchMode=yes"
        })
        .arg("-o")
        .arg("ConnectTimeout=8")
        .arg("-o")
        .arg("ConnectionAttempts=1")
        .arg("-o")
        .arg("ServerAliveInterval=5")
        .arg("-o")
        // CountMax=3 (≈15s grace), not 1: with ControlMaster pooling a single
        // missed keepalive used to tear down the *shared* master on any transient
        // stall, orphaning its socket — the next op then hit "read from master
        // failed: Broken pipe" / "Failed to connect to new control master" and
        // ssh exited 255. Tolerating a few missed beats keeps the master alive;
        // ssh_output still retries unmultiplexed if it dies anyway.
        .arg("ServerAliveCountMax=3")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        // These exec connections never inherit LocalForward/RemoteForward or
        // DynamicForward directives from ~/.ssh/config. Agentum's deliberate
        // forwards are armed separately with `ssh -O forward` against the
        // authenticated interactive master.
        .arg("ClearAllForwardings=yes")
        .arg("-o")
        // GSSAPI is never used for these hosts, but when the server advertises
        // it the client-side attempt can stall a cold connect for seconds
        // (Kerberos/DNS lookups). Disabling it shaves that off every handshake.
        .arg("GSSAPIAuthentication=no")
        .arg("-o")
        // TCPKeepAlive: surface a dead peer (slept laptop, dropped link) at the
        // TCP layer, not only via the app-level ServerAlive probe. A pooled
        // master whose far end vanished is then reset by the OS rather than
        // lingering as a half-open socket the next op stalls on. Cheap, and it
        // pairs with the ssh_output stale-master retry to keep reconnects clean.
        .arg("TCPKeepAlive=yes")
        // Compression: terminal/agent output is highly repetitive (redraw
        // escape sequences, whitespace, repeated status lines) and compresses
        // ~10x. On a bandwidth-limited link (e.g. a Tailscale-relayed host at
        // ~0.6 MB/s) that is the difference between ~0.5 MB/s and ~6 MB/s of
        // effective pane throughput — an ~11x win measured against Freebee. The
        // gzip CPU cost is trivial at these data rates, and agentum's SSH
        // traffic (pane streams, git, small execs) is all compressible and
        // never the fast-link bulk transfer where compression would hurt.
        .arg("-o")
        .arg(if mux == SshMux::Streaming || mux == SshMux::Off {
            "Compression=yes"
        } else {
            // Input and observer traffic consists of tiny latency-sensitive
            // records. Compression adds framing/CPU work but saves no useful
            // bandwidth there; reserve it for the bulk pane stream.
            "Compression=no"
        })
        .arg("-p")
        .arg(port.to_string());

    // ControlMaster connection pooling: the first op authenticates and opens a
    // master socket; subsequent ops within ControlPersist reuse it, skipping the
    // TCP+auth handshake entirely — the big remote-latency win (each tmux/git/fs
    // round trip would otherwise pay a full SSH handshake). Applies to both
    // key/agent and password auth. Only enabled when we have a private,
    // short-enough socket dir; otherwise we connect fresh rather than risk a
    // too-long ControlPath (ssh exits 255) or an unsafe socket location.
    match control_path {
        Some(control_path) => {
            cmd.arg("-o")
                .arg("ControlMaster=auto")
                .arg("-o")
                .arg(format!("ControlPath={control_path}"))
                .arg("-o")
                // 10m, not 30s: the master idles cheaply (one ssh process, a
                // keepalive every 5s), but re-establishing it costs a full
                // TCP+auth handshake — 1-3s on WAN. 30s expired between normal
                // user interactions, so nearly every sidebar click paid that
                // handshake again. ssh_output's unmuxed retry still covers a
                // master that dies mid-window.
                .arg("ControlPersist=600s");
        }
        None => {
            // `SshMux::Off` is the stale-master retry and must be fresh even
            // when the user's ~/.ssh/config enables ControlMaster. Apply the
            // same fail-closed behavior when no safe private ControlPath can be
            // constructed for a requested pooled mode.
            cmd.arg("-o")
                .arg("ControlMaster=no")
                .arg("-o")
                .arg("ControlPath=none");
        }
    }

    match auth {
        SshAuth::Key { path } if !path.trim().is_empty() => {
            cmd.arg("-i").arg(path);
            // Pin auth to THIS key. Without IdentitiesOnly, ssh first offers
            // every ssh-agent/default identity; on a server with a low
            // `MaxAuthTries` that burns the attempt budget before our key is
            // tried and the connection is refused ("Too many authentication
            // failures"). Forcing the publickey method also stops a failed key
            // from falling through to a method that can't succeed under
            // BatchMode. Agent auth deliberately omits both (it must let the
            // agent offer its keys).
            cmd.arg("-o")
                .arg("IdentitiesOnly=yes")
                .arg("-o")
                .arg("PreferredAuthentications=publickey");
        }
        SshAuth::Password { password } if !password.trim().is_empty() => {
            // Force password-backed auth so ssh doesn't silently try a key
            // first (which would bypass the askpass prompt and fail
            // confusingly). Some hardened hosts disable the plain `password`
            // method but expose the same password prompt through PAM-backed
            // keyboard-interactive auth; SSH_ASKPASS handles both prompts.
            cmd.arg("-o")
                .arg("PreferredAuthentications=password,keyboard-interactive")
                .arg("-o")
                .arg("PubkeyAuthentication=no")
                .arg("-o")
                .arg("PasswordAuthentication=yes")
                .arg("-o")
                .arg("KbdInteractiveAuthentication=yes");
            // OpenSSH receives only an owner-only helper and a non-secret
            // locator. The password itself lives in a unique 0600 file and is
            // removed on completion/cancel or by bounded failsafe cleanup.
            if let Some(askpass) = &askpass {
                cmd.env("SSH_ASKPASS", &askpass.helper)
                    .env("SSH_ASKPASS_REQUIRE", "force")
                    .env(ASKPASS_SECRET_ENV, &askpass.secret.path);
                // Pre-8.4 ssh only consults askpass when DISPLAY is set; a
                // placeholder is harmless (our helper never touches X).
                if std::env::var_os("DISPLAY").is_none() {
                    cmd.env("DISPLAY", ":0");
                }
            }
        }
        _ => {}
    }

    cmd.arg(format!("{user}@{hostname}")).arg(script);
    Ok(PreparedSshCommand {
        command: cmd,
        password_secret: askpass.map(|askpass| askpass.secret),
    })
}

/// Build an `ssh -O forward -R …` control command that adds a **reverse** port
/// forward to the host's *already-established* interactive ControlMaster — no
/// new connection. On the host, `127.0.0.1:<host_port>` then tunnels back to the
/// Mac's `127.0.0.1:<mac_port>` (the embedded agentum MCP server). Bound to
/// loopback on BOTH ends, so it's never exposed to either machine's network.
///
/// Returns `None` for a non-SSH host or when no ControlPath is available (the
/// master must exist first — warm it via the normal path). `-O forward` matches
/// the running master by its record/revision ControlPath, so only the host
/// identity (`-p`, `user@host`) needs to agree with how the master was opened.
pub fn ssh_control_forward_cmd(host: &Host, host_port: u16, mac_port: u16) -> Option<Command> {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        ..
    } = &host.kind
    else {
        return None;
    };
    let control_path = control_path_for(host, SshMux::Interactive)?;
    // Explicit loopback bind on the host side (the leading `127.0.0.1:`); without
    // it a host with `GatewayPorts yes` could bind the wildcard and expose the
    // tunnel to the host's network. Mac side is loopback too.
    let spec = format!("127.0.0.1:{host_port}:127.0.0.1:{mac_port}");
    let mut cmd = ssh_existing_control_command();
    cmd.arg("-T")
        .arg("-o")
        .arg(format!("ControlPath={control_path}"))
        .arg("-p")
        .arg(port.to_string())
        .arg("-O")
        .arg("forward")
        .arg("-R")
        .arg(spec)
        .arg(format!("{user}@{hostname}"));
    Some(cmd)
}

/// `ssh -O cancel -R 127.0.0.1:<host_port>:127.0.0.1:<mac_port>` — tears down a
/// reverse forward on the host's master. OpenSSH requires the same full forward
/// specification used to arm it; a listen-side-only `-R 127.0.0.1:<host_port>`
/// is rejected as "port not forwarded" and silently leaves the tunnel behind.
/// Best-effort cleanup runs before re-arming so repeated starts in one embedded
/// server instance reuse the same remote port instead of leaking one per start.
pub fn ssh_control_cancel_cmd(host: &Host, host_port: u16, mac_port: u16) -> Option<Command> {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        ..
    } = &host.kind
    else {
        return None;
    };
    let control_path = control_path_for(host, SshMux::Interactive)?;
    let mut cmd = ssh_existing_control_command();
    cmd.arg("-T")
        .arg("-o")
        .arg(format!("ControlPath={control_path}"))
        .arg("-p")
        .arg(port.to_string())
        .arg("-O")
        .arg("cancel")
        .arg("-R")
        .arg(format!("127.0.0.1:{host_port}:127.0.0.1:{mac_port}"))
        .arg(format!("{user}@{hostname}"));
    Some(cmd)
}

/// Build an `ssh -O forward -L …` control command that adds a **local** port
/// forward to the host's *already-established* interactive ControlMaster — no
/// new connection. On the Mac, `127.0.0.1:<mac_port>` then tunnels to the host's
/// `127.0.0.1:<host_port>` (where headless Chromium binds its CDP debugger).
/// This is the mirror of [`ssh_control_forward_cmd`]'s reverse (-R) forward:
/// the MCP server lives on the Mac (reverse), but the browser/CDP lives on the
/// host, so the Mac reaches it with a forward (-L). Bound to loopback on BOTH
/// ends, so it's never exposed to either machine's network.
///
/// Returns `None` for a non-SSH host or when no ControlPath is available (the
/// master must exist first — warm it via the normal path). `-O forward` matches
/// the running master by its record/revision ControlPath, so only the host
/// identity (`-p`, `user@host`) needs to agree with how the master was opened.
pub fn ssh_control_local_forward_cmd(
    host: &Host,
    mac_port: u16,
    host_port: u16,
) -> Option<Command> {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        ..
    } = &host.kind
    else {
        return None;
    };
    let control_path = control_path_for(host, SshMux::Interactive)?;
    // Explicit `127.0.0.1:` listen bind on the Mac side so the forwarded port is
    // never offered on the Mac's network; the host side is loopback too (that's
    // where Chromium's CDP binds, via `--remote-debugging-address=127.0.0.1`).
    let spec = format!("127.0.0.1:{mac_port}:127.0.0.1:{host_port}");
    let mut cmd = ssh_existing_control_command();
    cmd.arg("-T")
        .arg("-o")
        .arg(format!("ControlPath={control_path}"))
        .arg("-p")
        .arg(port.to_string())
        .arg("-O")
        .arg("forward")
        .arg("-L")
        .arg(spec)
        .arg(format!("{user}@{hostname}"));
    Some(cmd)
}

/// `ssh -O cancel -L 127.0.0.1:<mac_port>:127.0.0.1:<host_port>` — tears down a
/// local forward on the host's master. OpenSSH rejects a listen-side-only
/// `-O cancel -L` ("Bad local forwarding specification"); like
/// [`ssh_control_cancel_cmd`], this must pass the SAME full spec used to arm it
/// (verified on OpenSSH 10.0p2).
/// Best-effort cleanup run before re-arming so a re-attach refreshes the forward
/// instead of colliding with its own still-present one.
pub fn ssh_control_local_cancel_cmd(host: &Host, mac_port: u16, host_port: u16) -> Option<Command> {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        ..
    } = &host.kind
    else {
        return None;
    };
    let control_path = control_path_for(host, SshMux::Interactive)?;
    let spec = format!("127.0.0.1:{mac_port}:127.0.0.1:{host_port}");
    let mut cmd = ssh_existing_control_command();
    cmd.arg("-T")
        .arg("-o")
        .arg(format!("ControlPath={control_path}"))
        .arg("-p")
        .arg(port.to_string())
        .arg("-O")
        .arg("cancel")
        .arg("-L")
        .arg(spec)
        .arg(format!("{user}@{hostname}"));
    Some(cmd)
}

/// Build an `ssh -O exit` command for one of `host`'s pooled
/// ControlMasters. This never opens a new connection: OpenSSH resolves the
/// host-specific record/revision path and asks only the master listening there
/// to exit.
///
/// Returns `None` for a local host, [`SshMux::Off`], or when no safe
/// ControlPath is available. Callers normally want [`ssh_close_control_masters`]
/// so both of a host's authenticated masters are invalidated together and the
/// commands are time-bounded.
pub fn ssh_control_exit_cmd(host: &Host, mux: SshMux) -> Option<Command> {
    if !matches!(&host.kind, HostKind::Ssh { .. }) {
        return None;
    }
    let control_path = control_path_for(host, mux)?;
    control_exit_cmd_for_path(host, &control_path)
}

/// Build the migration-only `ssh -O exit` command for an Agentum
/// pre-namespacing master. The ControlPath is one of the two exact historic
/// leaves (`cm-%C` / `cms-%C`) under Agentum's validated private directories;
/// there is no directory scan or wildcard that could select unrelated SSH
/// sockets.
fn ssh_legacy_control_exit_cmds(host: &Host, mux: SshMux) -> Vec<Command> {
    if !matches!(&host.kind, HostKind::Ssh { .. }) {
        return Vec::new();
    }
    legacy_control_paths_for(mux)
        .into_iter()
        .filter_map(|control_path| control_exit_cmd_for_path(host, &control_path))
        .collect()
}

fn control_exit_cmd_for_path(host: &Host, control_path: &str) -> Option<Command> {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        ..
    } = &host.kind
    else {
        return None;
    };
    let mut cmd = ssh_existing_control_command();
    cmd.arg("-T")
        .arg("-o")
        .arg(format!("ControlPath={control_path}"))
        .arg("-p")
        .arg(port.to_string())
        .arg("-O")
        .arg("exit")
        .arg(format!("{user}@{hostname}"));
    Some(cmd)
}

async fn close_control_master_command(cmd: Option<Command>, dur: Duration) -> std::io::Result<()> {
    let Some(cmd) = cmd else {
        return Ok(());
    };
    let output = command_output_with_timeout(cmd, dur, "ssh control master exit timed out").await?;
    classify_control_exit_output(
        output.status.success(),
        output.status.code(),
        &output.stdout,
        &output.stderr,
    )
}

/// Retire ControlMasters created by Agentum releases that used `cm-%C` /
/// `cms-%C`, under both historical private socket roots. Every exit is
/// independently bounded by `dur` and runs concurrently. Missing/refused
/// sockets are idempotent success; other SSH diagnostics remain visible.
///
/// This migration is deliberately narrow: OpenSSH expands `%C` for this exact
/// host's user/hostname/port, and the templates live only in Agentum's
/// validated owner-only XDG/HOME socket directories. It never enumerates,
/// unlinks, or sends control messages to any other OpenSSH socket.
pub async fn ssh_retire_legacy_control_masters(host: &Host, dur: Duration) -> std::io::Result<()> {
    // Two roots × two mux roles. Pull into fixed slots so all exits can be
    // polled concurrently without another async dependency; absent roots leave
    // a no-op slot.
    let mut commands = ssh_legacy_control_exit_cmds(host, SshMux::Interactive)
        .into_iter()
        .chain(ssh_legacy_control_exit_cmds(host, SshMux::Streaming));
    let first = commands.next();
    let second = commands.next();
    let third = commands.next();
    let fourth = commands.next();
    debug_assert!(commands.next().is_none());
    let (first, second, third, fourth) = tokio::join!(
        close_control_master_command(first, dur),
        close_control_master_command(second, dur),
        close_control_master_command(third, dur),
        close_control_master_command(fourth, dur)
    );
    first.and(second).and(third).and(fourth)
}

/// Close all current, record/revision-namespaced ControlMasters and both
/// pre-namespacing ControlMasters associated with `host`, bounded by `dur` per
/// command. All exits run concurrently, so the total wall-clock bound is one
/// `dur` even when both historical socket roots exist.
///
/// Exit status zero and OpenSSH's exact "control socket absent/refused" status
/// are idempotent success. Authentication/configuration/permission failures are
/// returned instead of being mistaken for a closed master. Spawn failures and
/// timeouts are also returned, but all exit attempts always run before the
/// first error is propagated. A local host, or an environment where no safe
/// ControlPath can be built, is a no-op.
pub async fn ssh_close_control_masters(host: &Host, dur: Duration) -> std::io::Result<()> {
    let (interactive, streaming, observer, legacy) = tokio::join!(
        close_control_master_command(ssh_control_exit_cmd(host, SshMux::Interactive), dur),
        close_control_master_command(ssh_control_exit_cmd(host, SshMux::Streaming), dur),
        close_control_master_command(ssh_control_exit_cmd(host, SshMux::Observer), dur),
        ssh_retire_legacy_control_masters(host, dur)
    );
    interactive.and(streaming).and(observer).and(legacy)
}

fn is_absent_control_socket_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("Control socket connect(") else {
        return false;
    };
    let Some((path, reason)) = rest.rsplit_once("): ") else {
        return false;
    };
    !path.is_empty() && matches!(reason, "No such file or directory" | "Connection refused")
}

fn classify_control_exit_output(
    success: bool,
    status: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> std::io::Result<()> {
    if success {
        return Ok(());
    }

    let stderr_text = String::from_utf8_lossy(stderr);
    let lines: Vec<_> = stderr_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if status == Some(255)
        && stdout.is_empty()
        && lines.len() == 1
        && is_absent_control_socket_line(lines[0])
    {
        return Ok(());
    }

    let diagnostic = if !stderr_text.trim().is_empty() {
        stderr_text.trim().to_string()
    } else if !stdout.is_empty() {
        String::from_utf8_lossy(stdout).trim().to_string()
    } else {
        "no diagnostic output".to_string()
    };
    Err(std::io::Error::other(format!(
        "ssh control master exit failed with status {}: {diagnostic}",
        status
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    )))
}

/// True only when every stderr line is one of OpenSSH's known *pre-session*
/// ControlMaster diagnostics. A loose `contains("mux_client")` check is unsafe:
/// `mux_client_read_packet` and `mux_client_request_session: read from master`
/// can be emitted after OpenSSH has sent the remote command, so replaying a
/// mutating operation may execute it twice.
///
/// This intentionally recognizes a small set. An unfamiliar mux failure is
/// surfaced to the caller instead of gambling on whether the remote script ran.
pub fn is_mux_transport_error(stderr: &str) -> bool {
    let mut found_pre_session_failure = false;
    let mut found_line = false;

    for line in stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        found_line = true;
        let pre_session_failure = matches!(
            line,
            // The alive exchange happens before OpenSSH serializes/sends the
            // new-session request containing the remote command.
            "mux_client_request_session: master alive request failed"
                | "mux_client_request_stdio_fwd: master alive request failed"
                | "mux_client_request_alive: write packet: Broken pipe"
                | "mux_client_request_alive: write packet: Connection reset by peer"
                // Hello exchange precedes every mux request.
                | "muxclient: master hello exchange failed"
        ) || is_absent_control_socket_line(line);

        if pre_session_failure {
            found_pre_session_failure = true;
            continue;
        }
        // ControlPersist's newly-forked foreground client adds this fatal line
        // after a preceding mux diagnostic. It is not sufficient by itself and
        // is accepted only as a companion to a known pre-session failure.
        if line == "Failed to connect to new control master" {
            continue;
        }
        return false;
    }

    found_line && found_pre_session_failure
}

fn is_replay_safe_mux_failure(status: Option<i32>, stdout: &[u8], stderr: &[u8]) -> bool {
    status == Some(255)
        && stdout.is_empty()
        && std::str::from_utf8(stderr).is_ok_and(is_mux_transport_error)
}

/// Run `script` over SSH with `.output()`, bounded by `dur`, transparently
/// retrying ONCE on a stale/racing-ControlMaster transport failure with
/// multiplexing disabled. The pooled master can die mid-flight (keepalive
/// timeout on a transient stall, or its ControlPersist window expiring exactly
/// as a new op connects), leaving a dead socket; the next op then exits 255 at
/// the mux layer *without having run the remote script*, which makes a replay
/// on a fresh connection safe. Returns the raw [`Output`] so each caller keeps
/// its own non-zero-exit semantics; only transport/timeout failures are `Err`.
///
/// [`Output`]: std::process::Output
pub async fn ssh_output(
    host: &Host,
    script: &str,
    dur: Duration,
) -> std::io::Result<std::process::Output> {
    ssh_output_on(host, script, dur, SshMux::Interactive).await
}

/// Like [`ssh_output`] but rides the caller-chosen pooled ControlMaster.
///
/// Why this exists: the watchdog's per-session pane sample ([`sample_pane`]) is
/// an `ssh` exec every tick. Riding the *interactive* master (`cm-`) meant those
/// execs shared one TCP connection's channel budget with — and starved — the
/// keystroke writer and other interactive execs. With several remote sessions
/// open that contention throttled typing AND pane output (the remote tmux server
/// was also busy servicing N `capture-pane`s/tick). Routing the sample onto the
/// *streaming* master (`cms-`, otherwise just long-lived `tail -f` channels)
/// takes it off the keystroke path entirely. Same stale-master retry as
/// [`ssh_output`]: a 255 mux-transport failure replays once unmultiplexed.
pub async fn ssh_output_on(
    host: &Host,
    script: &str,
    dur: Duration,
    mux: SshMux,
) -> std::io::Result<std::process::Output> {
    let first = run_ssh_once(host, script, dur, mux).await?;
    if mux != SshMux::Off
        && is_replay_safe_mux_failure(first.status.code(), &first.stdout, &first.stderr)
    {
        return run_ssh_once(host, script, dur, SshMux::Off).await;
    }
    Ok(first)
}

/// Drive a child to captured output with a hard deadline. Stdout and stderr are
/// drained concurrently to avoid pipe deadlocks; on timeout the child is
/// explicitly killed, given a bounded reap window, and its remaining pipe data
/// is drained within a second bounded window.
/// `kill_on_drop` remains a final cancellation safeguard for callers that drop
/// this entire future before its timeout branch can run.
async fn command_output_with_timeout(
    cmd: Command,
    dur: Duration,
    timeout_message: &'static str,
) -> std::io::Result<std::process::Output> {
    let child = spawn_output_child(cmd)?;
    child_output_with_timeout(child, dur, timeout_message).await
}

fn spawn_output_child(mut cmd: Command) -> std::io::Result<tokio::process::Child> {
    cmd.kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Spawn before constructing the timeout future. Besides making the order
    // explicit, this keeps immediate spawn errors distinct from a deadline.
    cmd.spawn()
}

async fn child_output_with_timeout(
    mut child: tokio::process::Child,
    dur: Duration,
    timeout_message: &'static str,
) -> std::io::Result<std::process::Output> {
    use tokio::io::AsyncReadExt;

    const REAP_AFTER_KILL_TIMEOUT: Duration = Duration::from_secs(2);
    const DRAIN_AFTER_KILL_TIMEOUT: Duration = Duration::from_secs(2);

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("SSH child stdout was not configured as piped"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("SSH child stderr was not configured as piped"))?;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();

    // Poll both pipe drains while waiting so a chatty child cannot fill either
    // kernel pipe and deadlock before exit.
    let operation = async {
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut stdout_bytes),
            stderr.read_to_end(&mut stderr_bytes),
        )?;
        Ok::<_, std::io::Error>(std::process::Output {
            status,
            stdout: std::mem::take(&mut stdout_bytes),
            stderr: std::mem::take(&mut stderr_bytes),
        })
    };

    match timeout(dur, operation).await {
        Ok(result) => result,
        Err(_) => {
            // The timed operation was canceled, so the mutable child/pipe
            // borrows are released. Explicitly SIGKILL, then make a bounded
            // attempt to reap; do not rely on kill_on_drop, which can leave a
            // zombie. Treat an InvalidInput kill as an exit race and still
            // call wait(). A pathological platform/driver must not turn this
            // cleanup path into a second unbounded hang.
            let kill_error = match child.start_kill() {
                Ok(()) => None,
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => None,
                Err(error) => Some(error),
            };
            let reap_result = timeout(REAP_AFTER_KILL_TIMEOUT, child.wait()).await;

            // Killing ssh closes its pipe writers. Drain whatever was already
            // buffered, but bound this tail in case a misconfigured descendant
            // inherited the descriptors; dropping the handles then closes them.
            let drain_result = timeout(DRAIN_AFTER_KILL_TIMEOUT, async {
                tokio::try_join!(
                    stdout.read_to_end(&mut stdout_bytes),
                    stderr.read_to_end(&mut stderr_bytes),
                )
            })
            .await;

            let mut cleanup = Vec::with_capacity(3);
            if let Some(error) = kill_error {
                cleanup.push(format!("kill failed: {error}"));
            }
            match reap_result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => cleanup.push(format!("reap failed: {error}")),
                Err(_) => cleanup.push("reap timed out".to_string()),
            }
            match drain_result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => cleanup.push(format!("pipe drain failed: {error}")),
                Err(_) => cleanup.push("pipe drain cleanup timed out".to_string()),
            }
            let message = if cleanup.is_empty() {
                timeout_message.to_string()
            } else {
                format!("{timeout_message}; {}", cleanup.join("; "))
            };
            Err(std::io::Error::new(std::io::ErrorKind::TimedOut, message))
        }
    }
}

/// One `.output()` attempt, bounded by `dur`. A timeout surfaces as an
/// `io::Error` of kind `TimedOut` so callers can map it to their own variant.
async fn run_ssh_once(
    host: &Host,
    script: &str,
    dur: Duration,
    mux: SshMux,
) -> std::io::Result<std::process::Output> {
    let PreparedSshCommand {
        command,
        mut password_secret,
    } = try_ssh_command_opts(host, script, mux)?;
    let result = command_output_with_timeout(command, dur, "ssh timed out").await;
    let cleanup = match &mut password_secret {
        Some(secret) => secret.remove_now(),
        None => Ok(()),
    };
    match (result, cleanup) {
        (Ok(output), Ok(())) => Ok(output),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), _) => Err(error),
    }
}

/// shell-quote `s`, mapping a quoting failure to [`TmuxError::Quote`].
fn q(s: &str) -> Result<Cow<'_, str>> {
    shlex::try_quote(s).map_err(|_| TmuxError::Quote)
}

/// Run `script` over SSH and return its stdout, erroring on a non-zero exit
/// or timeout. Mirrors `host_runtime::ssh_stdout`.
async fn ssh_stdout(host: &Host, script: &str) -> Result<String> {
    let output = ssh_output(host, script, SSH_TIMEOUT)
        .await
        .map_err(TmuxError::Io)?;
    if !output.status.success() {
        return Err(TmuxError::NonZero {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// Run `script` over SSH and error on a non-zero exit (or transport
/// failure / timeout). Mirrors `host_runtime::ssh_checked`.
async fn ssh_checked(host: &Host, script: &str) -> Result<()> {
    let output = ssh_output(host, script, SSH_TIMEOUT)
        .await
        .map_err(TmuxError::Io)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(TmuxError::NonZero {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

// ───────────────────────── host-aware tmux ops ─────────────────────────
// Each branches on `host.kind`: Local calls the existing `crate::<fn>`
// (identical behaviour to before the refactor); Ssh runs the same tmux
// command wrapped in `sh -c` (the remote login shell may be fish/zsh, which
// reject the POSIX `for`/`case`/quoting the SSH branches build).

/// True only for stderr forms tmux itself emits when a target (or the entire
/// tmux server) is absent. This intentionally does not classify generic
/// "connection refused", command-not-found, auth, or SSH transport messages.
pub fn is_tmux_session_missing_error(stderr: &str) -> bool {
    let mut found_missing = false;
    for line in stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let canonical_missing = line
            .strip_prefix("can't find session: ")
            .is_some_and(|target| !target.trim().is_empty())
            || line
                .strip_prefix("no server running on ")
                .is_some_and(|socket| !socket.trim().is_empty())
            // Recent tmux versions use this form for an absent named socket.
            || (line.starts_with("error connecting to ")
                && (line.ends_with("(No such file or directory)")
                    || line.ends_with("(Connection refused)")))
            // Older tmux versions emitted this path-independent form.
            || matches!(
                line,
                "failed to connect to server: No such file or directory"
                    | "failed to connect to server: Connection refused"
            );
        if canonical_missing {
            found_missing = true;
            continue;
        }
        return false;
    }
    found_missing
}

const HAS_SESSION_PROTOCOL_TAG: &str = "AGENTUM_TMUX_HAS_SESSION_V1";

/// POSIX-shell body that resolves an exact tmux session name to its immutable
/// `$N` session id. tmux 3.7 rejects the old `=name` exact-target syntax, while
/// plain `-t name` prefix-matches and can operate on the wrong Agentum session.
/// All remote operations therefore compare `#{session_name}` as shell strings
/// and use only the quoted id afterward.
fn exact_session_lookup_body(target: &str) -> Result<String> {
    let target_is_id = crate::session_target_is_id(target)?;
    let target = q(target)?;
    let candidate = if target_is_id {
        "$candidate_id"
    } else {
        "$candidate_name"
    };
    Ok(format!(
        "target={target}\n\
         tmux_rows=$(tmux list-sessions -F '#{{session_id}}_#{{session_name}}' 2>&1)\n\
         tmux_status=$?\n\
         tmux_error=\n\
         session_id=\n\
         session_missing=0\n\
         if [ \"$tmux_status\" -eq 0 ]; then\n\
           dollar=$(printf '\\044')\n\
           session_id=$(printf '%s\\n' \"$tmux_rows\" | while IFS=_ read -r candidate_id candidate_name; do\n\
             candidate_prefix=${{candidate_id%\"${{candidate_id#?}}\"}}\n\
             candidate_digits=${{candidate_id#?}}\n\
             if [ \"$candidate_prefix\" != \"$dollar\" ] || [ -z \"$candidate_digits\" ]; then continue; fi\n\
             nondigits=$(printf '%s' \"$candidate_digits\" | tr -d 0123456789)\n\
             if [ -n \"$nondigits\" ]; then continue; fi\n\
             if [ \"{candidate}\" = \"$target\" ]; then\n\
               printf '%s' \"$candidate_id\"\n\
               break\n\
             fi\n\
           done)\n\
           if [ -z \"$session_id\" ]; then\n\
             tmux_status=1\n\
             tmux_error=\"can't find session: $target\"\n\
             session_missing=1\n\
           fi\n\
         else\n\
           tmux_error=$tmux_rows\n\
           case \"$tmux_error\" in\n\
             \"no server running on \"* | \
             \"failed to connect to server: No such file or directory\" | \
             \"failed to connect to server: Connection refused\" | \
             \"error connecting to \"*\" (No such file or directory)\" | \
             \"error connecting to \"*\" (Connection refused)\") session_missing=1 ;;\n\
           esac\n\
         fi"
    ))
}

fn exact_session_action_script(
    target: &str,
    action: &str,
    missing_exit: Option<i32>,
) -> Result<String> {
    let lookup = exact_session_lookup_body(target)?;
    let missing = missing_exit.map_or_else(String::new, |status| {
        format!("if [ \"$session_missing\" -eq 1 ]; then exit {status}; fi\n")
    });
    let inner = format!(
        "{lookup}\n\
         {missing}\
         if [ \"$tmux_status\" -ne 0 ]; then\n\
           printf '%s\\n' \"$tmux_error\" >&2\n\
           exit \"$tmux_status\"\n\
         fi\n\
         {action}"
    );
    Ok(format!("sh -c {}", q(&inner)?))
}

fn has_session_probe_script(target: &str) -> Result<String> {
    let lookup = exact_session_lookup_body(target)?;
    let inner = format!(
        "{lookup}\n\
         tmux_error_hex=$(printf '%s' \"$tmux_error\" | od -An -v -tx1 | tr -d ' \\r\\n')\n\
         printf '{HAS_SESSION_PROTOCOL_TAG}\\t%s\\t%s\\n' \"$tmux_status\" \"$tmux_error_hex\""
    );
    // The remote login shell may be fish/zsh; make the probe itself explicitly
    // POSIX and keep any login banner/noise outside the tagged payload.
    Ok(format!("sh -c {}", q(&inner)?))
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if input.len() % 2 != 0 || !input.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    input
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

fn parse_has_session_probe(stdout: &[u8]) -> Option<(i32, String)> {
    let stdout = std::str::from_utf8(stdout).ok()?;
    let payload = stdout.lines().rev().find_map(|line| {
        line.strip_suffix('\r')
            .unwrap_or(line)
            .strip_prefix(HAS_SESSION_PROTOCOL_TAG)?
            .strip_prefix('\t')
    })?;
    let mut fields = payload.splitn(2, '\t');
    let status = fields.next()?.parse::<i32>().ok()?;
    if !(0..=255).contains(&status) {
        return None;
    }
    let stderr = decode_hex(fields.next()?)?;
    Some((status, String::from_utf8_lossy(&stderr).into_owned()))
}

fn classify_has_session_probe(status: i32, stderr: &str) -> Result<bool> {
    if status == 0 {
        return Ok(true);
    }
    if status == 1 && is_tmux_session_missing_error(stderr) {
        return Ok(false);
    }
    Err(TmuxError::NonZero {
        status,
        stderr: stderr.trim().to_string(),
    })
}

fn classify_remote_has_session_output(
    ssh_success: bool,
    ssh_status: Option<i32>,
    stdout: &[u8],
    ssh_stderr: &[u8],
) -> Result<bool> {
    if !ssh_success {
        return Err(TmuxError::NonZero {
            status: ssh_status.unwrap_or(-1),
            stderr: String::from_utf8_lossy(ssh_stderr).trim().to_string(),
        });
    }
    let (tmux_status, tmux_stderr) =
        parse_has_session_probe(stdout).ok_or_else(|| TmuxError::NonZero {
            status: -1,
            stderr: "remote tmux has-session returned no valid Agentum protocol record".to_string(),
        })?;
    // A successful SSH transport may legitimately write host-key notices or
    // login-shell noise to stderr. Only the encoded record came from tmux.
    classify_has_session_probe(tmux_status, &tmux_stderr)
}

/// `tmux has-session` on `host`. Returns `false` only for tmux's canonical
/// missing-target/no-server response. Auth failures, SSH transport exits,
/// missing `tmux` binaries, and unexpected tmux failures remain typed
/// [`TmuxError::NonZero`] errors rather than masquerading as an absent session.
pub async fn has_session(host: &Host, target: &str) -> Result<bool> {
    match &host.kind {
        HostKind::Local => crate::has_session(target).await,
        HostKind::Ssh { .. } => {
            let script = has_session_probe_script(target)?;
            let output = ssh_output(host, &script, SSH_TIMEOUT)
                .await
                .map_err(TmuxError::Io)?;
            classify_remote_has_session_output(
                output.status.success(),
                output.status.code(),
                &output.stdout,
                &output.stderr,
            )
        }
    }
}

/// Capture the last `lines` of `target`'s pane (incl. scrollback) as plain
/// text on `host`.
pub async fn capture_pane(host: &Host, target: &str, lines: usize) -> Result<String> {
    match &host.kind {
        HostKind::Local => crate::capture_pane(target, lines).await,
        HostKind::Ssh { .. } => {
            let action = format!("tmux capture-pane -p -S -{lines} -t \"$session_id\"");
            let script = exact_session_action_script(target, &action, None)?;
            ssh_stdout(host, &script).await
        }
    }
}

/// Capture only the currently-visible viewport of `target` (no scrollback)
/// as plain text on `host`.
pub async fn capture_pane_visible(host: &Host, target: &str) -> Result<String> {
    match &host.kind {
        HostKind::Local => crate::capture_pane_visible(target).await,
        HostKind::Ssh { .. } => {
            let script = exact_session_action_script(
                target,
                "tmux capture-pane -p -S 0 -t \"$session_id\"",
                None,
            )?;
            ssh_stdout(host, &script).await
        }
    }
}

/// Send `keys` (a tmux key spec or text) to `target` on `host`, optionally
/// appending Enter.
pub async fn send_keys(host: &Host, target: &str, keys: &str, append_enter: bool) -> Result<()> {
    match &host.kind {
        HostKind::Local => crate::send_keys(target, keys, append_enter).await,
        HostKind::Ssh { .. } => {
            let mut action = format!("tmux send-keys -t \"$session_id\" {}", q(keys)?);
            if append_enter {
                action.push_str(" Enter");
            }
            let script = exact_session_action_script(target, &action, None)?;
            ssh_checked(host, &script).await
        }
    }
}

/// Everything the watchdog needs about a pane, gathered in one round trip.
/// See [`sample_pane`].
#[derive(Debug)]
pub struct PaneSample {
    /// Last N lines including scrollback (crash + context-low matching).
    pub pane: String,
    /// Currently-visible viewport only (activity classification).
    pub viewport: String,
    /// Foreground process basename (`#{pane_current_command}`), trimmed.
    pub current_command: String,
}

/// Boundary line separating the three sections of [`sample_pane`]'s combined
/// remote output. High-entropy so rendered pane text essentially can't
/// collide with it; a collision parses as a section-count mismatch and
/// surfaces as an `Err` (one skipped watchdog tick), never as wrong data
/// silently attributed to the wrong section.
const SAMPLE_BOUNDARY: &str = ":::agentum-pane-sample-7f3a9c:::";

/// Exit code the sample script uses for "session is gone" — distinguishable
/// from ssh's own 255 (transport) and tmux's 1 (generic error).
const SAMPLE_GONE_EXIT: i32 = 43;

/// One watchdog sample of `target` on `host`: session existence, a
/// scrollback capture (`lines` deep), the visible viewport, and the
/// foreground command. Returns `Ok(None)` when the session no longer exists.
///
/// On SSH hosts this is ONE remote exec instead of the four the watchdog
/// previously issued per tick (`has-session` + two `capture-pane`s +
/// `display-message`). At a 1 s tick with several remote sessions open, those
/// per-call channel open/closes were the dominant load on the shared
/// ControlMaster — the same master that carries interactive keystrokes — so
/// batching directly reduces input latency, not just probe overhead.
/// Local hosts keep the four direct tmux calls (process spawns are cheap and
/// there is no channel contention to relieve).
pub async fn sample_pane(host: &Host, target: &str, lines: usize) -> Result<Option<PaneSample>> {
    match &host.kind {
        HostKind::Local => {
            if !crate::has_session(target).await? {
                return Ok(None);
            }
            Ok(Some(PaneSample {
                pane: crate::capture_pane(target, lines).await?,
                viewport: crate::capture_pane_visible(target).await?,
                current_command: crate::pane_current_command(target).await?,
            }))
        }
        HostKind::Ssh { .. } => {
            // `2>/dev/null` on the captures: if the session dies between the
            // exact-name resolution and a capture, malformed output parses as
            // a boundary mismatch (skipped tick) and the next tick exits 43.
            let action = format!(
                "tmux display-message -p -t \"$session_id\" '#{{pane_current_command}}' 2>/dev/null\n\
                 echo {SAMPLE_BOUNDARY}\n\
                 tmux capture-pane -p -S -{lines} -t \"$session_id\" 2>/dev/null\n\
                 echo {SAMPLE_BOUNDARY}\n\
                 tmux capture-pane -p -S 0 -t \"$session_id\" 2>/dev/null"
            );
            let script = exact_session_action_script(target, &action, Some(SAMPLE_GONE_EXIT))?;
            // Ride the STREAMING master, not the interactive one: this exec fires
            // every watchdog tick per session, and on the interactive master it
            // starved keystrokes (and, via remote tmux load, pane throughput).
            let output = ssh_output_on(host, &script, SSH_TIMEOUT, SshMux::Streaming)
                .await
                .map_err(TmuxError::Io)?;
            if output.status.code() == Some(SAMPLE_GONE_EXIT) {
                return Ok(None);
            }
            if !output.status.success() {
                return Err(TmuxError::NonZero {
                    status: output.status.code().unwrap_or(-1),
                    stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                });
            }
            let stdout = String::from_utf8(output.stdout)?;
            parse_pane_sample(&stdout)
                .map(Some)
                .ok_or_else(|| TmuxError::NonZero {
                    status: 0,
                    stderr: "pane sample output did not contain the expected sections".to_string(),
                })
        }
    }
}

/// Split the combined sample stdout into its three sections. `None` when the
/// boundary count is off (remote race or a pathological pane collision).
fn parse_pane_sample(stdout: &str) -> Option<PaneSample> {
    let sep = format!("\n{SAMPLE_BOUNDARY}\n");
    let mut parts = stdout.splitn(3, &sep);
    let current_command = parts.next()?.trim().to_string();
    let pane = parts.next()?.to_string();
    let viewport = parts.next()?.to_string();
    Some(PaneSample {
        pane,
        viewport,
        current_command,
    })
}

/// Basename of the foreground process inside `target`'s pane
/// (`#{pane_current_command}`) on `host`, trimmed.
pub async fn pane_current_command(host: &Host, target: &str) -> Result<String> {
    match &host.kind {
        HostKind::Local => crate::pane_current_command(target).await,
        HostKind::Ssh { .. } => {
            let script = exact_session_action_script(
                target,
                "tmux display-message -p -t \"$session_id\" '#{pane_current_command}'",
                None,
            )?;
            let out = ssh_stdout(host, &script).await?;
            Ok(out.trim().to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn host_lifecycle_lock_serializes_one_uuid_without_blocking_others() {
        let host_id = Uuid::new_v4();
        let other_host_id = Uuid::new_v4();
        let held = acquire_host_lifecycle(host_id).await;

        let mut same_host_waiter = tokio::spawn(acquire_host_lifecycle(host_id));
        assert!(
            timeout(Duration::from_millis(50), &mut same_host_waiter)
                .await
                .is_err(),
            "the same host acquired two lifecycle leases concurrently"
        );

        let other = timeout(
            Duration::from_secs(1),
            acquire_host_lifecycle(other_host_id),
        )
        .await
        .expect("an unrelated host lifecycle was blocked");
        drop(other);

        drop(held);
        let released = timeout(Duration::from_secs(1), same_host_waiter)
            .await
            .expect("same-host waiter stayed blocked after release")
            .expect("same-host waiter task panicked");
        drop(released);
    }

    fn ssh_host(auth: SshAuth) -> Host {
        Host {
            id: agentum_core::LOCAL_HOST_ID,
            name: "t".into(),
            kind: HostKind::Ssh {
                user: "me".into(),
                hostname: "box.local".into(),
                port: 2222,
                auth,
            },
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    // `ssh_command` returns a tokio Command; `.as_std()` exposes the inner
    // std Command for introspecting program + args.
    fn arg_strings(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn assert_ignores_user_config(args: &[String]) {
        assert!(
            args.windows(2).any(|pair| pair == ["-F", EMPTY_SSH_CONFIG]),
            "existing-master command reparses user SSH config: {args:?}"
        );
    }

    #[test]
    fn existing_control_command_uses_an_empty_config() {
        assert_ignores_user_config(&arg_strings(&ssh_existing_control_command()));
    }

    #[cfg(unix)]
    #[test]
    fn hot_pooled_command_skips_user_config_but_cold_command_keeps_it() {
        let mut host = ssh_host(SshAuth::Agent);
        host.id = Uuid::new_v4();
        let control_path = control_path_for(&host, SshMux::Interactive)
            .expect("test host has a safe control path");

        let cold = arg_strings(&ssh_command(&host, "true"));
        assert!(
            !cold.windows(2).any(|pair| pair == ["-F", EMPTY_SSH_CONFIG]),
            "cold connection must retain user SSH routing config: {cold:?}"
        );

        let listener = std::os::unix::net::UnixListener::bind(&control_path)
            .expect("bind fake private ControlMaster socket");
        let hot = arg_strings(&ssh_command(&host, "true"));
        assert_ignores_user_config(&hot);
        drop(listener);
        std::fs::remove_file(control_path).expect("remove fake ControlMaster socket");
    }

    // Env vars explicitly set on the Command (vars only cleared/inherited are
    // skipped) — lets the tests assert the SSH_ASKPASS wiring. Only the askpass
    // tests (unix-gated) use it, so gate it too to avoid a dead-code warning on
    // Windows.
    #[cfg(unix)]
    fn env_map(cmd: &Command) -> std::collections::HashMap<String, String> {
        cmd.as_std()
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect()
    }

    #[cfg(unix)]
    fn unix_test_dir(label: &str) -> PathBuf {
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after unix epoch")
                .as_nanos()
        );
        std::env::temp_dir().join(format!("agentum-{label}-{nonce}"))
    }

    #[cfg(unix)]
    fn unix_short_control_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};

        static SEQ: AtomicU64 = AtomicU64::new(0);
        // `/tmp` is intentionally used only as the parent of a freshly-created
        // randomish owner-only test directory. The production path never trusts
        // `/tmp`; keeping this lexical path short lets the test exercise the
        // unix-socket cap independently of macOS's very long `$TMPDIR`.
        PathBuf::from("/tmp").join(format!(
            "at-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[cfg(unix)]
    fn unix_control_dir_with_len(total_len: usize) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};

        static SEQ: AtomicU64 = AtomicU64::new(0);
        let stem = format!(
            "ab-{}-{}-",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let parent = "/tmp/";
        assert!(parent.len() + stem.len() <= total_len);
        let path = PathBuf::from(format!(
            "{parent}{stem}{}",
            "x".repeat(total_len - parent.len() - stem.len())
        ));
        assert_eq!(path.as_os_str().as_encoded_bytes().len(), total_len);
        path
    }

    #[test]
    fn ssh_command_key_uses_plain_ssh_with_batchmode() {
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        assert_eq!(cmd.as_std().get_program().to_string_lossy(), "ssh");
        let args = arg_strings(&cmd);
        assert!(args.contains(&"-T".to_string()));
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.iter().any(|a| a == "me@box.local"));
        // Key/agent must never reach for sshpass options.
        assert!(!args.contains(&"PreferredAuthentications=password".to_string()));
    }

    /// ControlMaster pooling must be present on every connection so repeated
    /// remote ops reuse one authenticated socket. Shared assertion for both auth
    /// paths (key/agent and password).
    fn assert_control_master(args: &[String]) {
        assert!(
            args.contains(&"ControlMaster=auto".to_string()),
            "missing ControlMaster=auto: {args:?}"
        );
        assert!(
            args.contains(&"ControlPersist=600s".to_string()),
            "missing ControlPersist=600s: {args:?}"
        );
        let control_path = args
            .iter()
            .find(|a| a.starts_with("ControlPath="))
            .unwrap_or_else(|| panic!("missing ControlPath=: {args:?}"));
        // The 16-hex SHA-256 namespace covers the full record id, mutation
        // revision, and public connection identity without lengthening sockets.
        let path = control_path.trim_start_matches("ControlPath=");
        let leaf = std::path::Path::new(path)
            .file_name()
            .expect("control socket leaf")
            .to_string_lossy();
        let namespace = leaf
            .strip_prefix("cm-")
            .unwrap_or_else(|| panic!("unexpected ControlPath: {control_path}"));
        assert_eq!(namespace.len(), 16, "unexpected namespace: {namespace}");
        assert!(
            namespace.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "namespace is not hexadecimal: {namespace}"
        );
        // The socket dir must exist (we create it on demand) — strip the
        // `ControlPath=` prefix and the namespaced leaf.
        let dir = std::path::Path::new(path).parent().expect("control dir");
        assert!(dir.is_dir(), "control dir not created: {}", dir.display());
    }

    #[test]
    fn ssh_command_key_enables_control_master_pooling() {
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        assert_control_master(&arg_strings(&cmd));
    }

    #[test]
    fn ssh_command_no_mux_explicitly_disables_user_config_pooling_and_forwards() {
        // The retry connection must NOT reuse the (stale) pooled socket, so the
        // command line explicitly overrides any ControlMaster/ControlPath from
        // the user's ssh config.
        let cmd = ssh_command_opts(&ssh_host(SshAuth::Agent), "echo hi", SshMux::Off);
        let args = arg_strings(&cmd);
        assert!(
            !args.iter().any(|a| a == "ControlMaster=auto"),
            "ControlMaster must be off on the unmultiplexed retry: {args:?}"
        );
        assert!(args.contains(&"ControlMaster=no".to_string()));
        assert!(args.contains(&"ControlPath=none".to_string()));
        assert!(args.contains(&"ClearAllForwardings=yes".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("ControlPersist=")));
    }

    #[test]
    fn every_ordinary_ssh_exec_clears_configured_forwards() {
        for mux in [
            SshMux::Off,
            SshMux::Interactive,
            SshMux::Streaming,
            SshMux::Observer,
        ] {
            let cmd = ssh_command_opts(&ssh_host(SshAuth::Agent), "echo hi", mux);
            assert!(
                arg_strings(&cmd).contains(&"ClearAllForwardings=yes".to_string()),
                "ordinary SSH exec inherited configured forwards (mux={mux:?})"
            );
        }
    }

    #[test]
    fn every_ssh_exec_disables_tty_allocation() {
        for mux in [
            SshMux::Off,
            SshMux::Interactive,
            SshMux::Streaming,
            SshMux::Observer,
        ] {
            let cmd = ssh_command_opts(&ssh_host(SshAuth::Agent), "echo hi", mux);
            assert!(
                arg_strings(&cmd).contains(&"-T".to_string()),
                "machine SSH exec allocated a tty (mux={mux:?})"
            );
        }
    }

    #[test]
    fn pooled_roles_use_three_distinct_sockets() {
        let path_of = |mux| {
            arg_strings(&ssh_command_opts(&ssh_host(SshAuth::Agent), "x", mux))
                .into_iter()
                .find(|a| a.starts_with("ControlPath="))
        };
        let interactive = path_of(SshMux::Interactive).expect("interactive path");
        let streaming = path_of(SshMux::Streaming).expect("streaming path");
        let observer = path_of(SshMux::Observer).expect("observer path");
        assert_ne!(interactive, streaming, "masters share a socket");
        assert_ne!(interactive, observer, "masters share a socket");
        assert_ne!(streaming, observer, "masters share a socket");
        assert!(
            interactive.contains("/cm-"),
            "interactive not cm-: {interactive}"
        );
        assert!(
            streaming.contains("/cms-"),
            "streaming not cms-: {streaming}"
        );
        assert!(observer.contains("/cmo-"), "observer not cmo-: {observer}");
    }

    #[test]
    fn control_namespace_tracks_record_and_explicit_host_revision() {
        let mut original = ssh_host(SshAuth::Agent);
        original.id = "00000000-0000-0000-0000-000000000001".parse().unwrap();

        let mut health_probe = original.clone();
        health_probe.last_seen_at = Some(time::OffsetDateTime::from_unix_timestamp(9_999).unwrap());
        assert_eq!(
            host_control_namespace(&original),
            host_control_namespace(&health_probe),
            "last_seen-only health updates stranded the live master"
        );

        let mut name_put = original.clone();
        name_put.name = "renamed display label".into();
        name_put.updated_at = time::OffsetDateTime::from_unix_timestamp(10_000).unwrap();
        assert_ne!(
            host_control_namespace(&original),
            host_control_namespace(&name_put),
            "an explicit host PUT did not rotate the mutation revision"
        );

        let mut duplicate_record = original.clone();
        duplicate_record.id = "00000000-0000-0000-0000-000000000002".parse().unwrap();
        assert_ne!(
            host_control_namespace(&original),
            host_control_namespace(&duplicate_record),
            "duplicate records shared an authenticated master"
        );

        let mut moved = original.clone();
        let HostKind::Ssh { hostname, .. } = &mut moved.kind else {
            unreachable!()
        };
        *hostname = "replacement.local".into();
        assert_ne!(
            host_control_namespace(&original),
            host_control_namespace(&moved)
        );

        let mut key_a = original.clone();
        let HostKind::Ssh { auth, .. } = &mut key_a.kind else {
            unreachable!()
        };
        *auth = SshAuth::Key {
            path: "/keys/a".into(),
        };
        let mut key_b = key_a.clone();
        let HostKind::Ssh { auth, .. } = &mut key_b.kind else {
            unreachable!()
        };
        *auth = SshAuth::Key {
            path: "/keys/b".into(),
        };
        assert_ne!(
            host_control_namespace(&key_a),
            host_control_namespace(&key_b)
        );

        let mut password_a = original.clone();
        let HostKind::Ssh { auth, .. } = &mut password_a.kind else {
            unreachable!()
        };
        *auth = SshAuth::Password {
            password: "revision-a".into(),
        };
        let mut password_b = password_a.clone();
        let HostKind::Ssh { auth, .. } = &mut password_b.kind else {
            unreachable!()
        };
        *auth = SshAuth::Password {
            password: "revision-b".into(),
        };
        assert_eq!(
            host_control_namespace(&password_a),
            host_control_namespace(&password_b),
            "password bytes must not create an offline verifier in process argv"
        );
        password_b.updated_at = time::OffsetDateTime::from_unix_timestamp(10_001).unwrap();
        assert_ne!(
            host_control_namespace(&password_a),
            host_control_namespace(&password_b),
            "password PUT revision did not rotate the ControlPath"
        );
        assert!(
            !host_control_namespace(&password_a).contains("revision-a"),
            "password appeared in the socket namespace"
        );
    }

    #[test]
    fn keepalive_tolerates_a_few_missed_beats() {
        // CountMax=1 orphaned the shared master on any transient stall; 3 gives
        // ~15s grace so the pooled socket survives a blip.
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        assert!(arg_strings(&cmd).contains(&"ServerAliveCountMax=3".to_string()));
    }

    #[test]
    fn ssh_command_uses_compression_only_for_bulk_capable_paths() {
        for mux in [SshMux::Off, SshMux::Streaming] {
            let cmd = ssh_command_opts(&ssh_host(SshAuth::Agent), "echo hi", mux);
            assert!(
                arg_strings(&cmd).contains(&"Compression=yes".to_string()),
                "compression missing (mux={mux:?})"
            );
        }
        for mux in [SshMux::Interactive, SshMux::Observer] {
            let cmd = ssh_command_opts(&ssh_host(SshAuth::Agent), "echo hi", mux);
            assert!(
                arg_strings(&cmd).contains(&"Compression=no".to_string()),
                "latency-sensitive role enables compression (mux={mux:?})"
            );
        }
    }

    #[test]
    fn ssh_command_key_pins_to_that_identity() {
        // An explicit key must pin auth to THAT key. IdentitiesOnly stops ssh
        // from offering every ssh-agent/default key BEFORE the configured one —
        // which trips a hardened server's `MaxAuthTries` and gets the whole
        // connection refused ("Too many authentication failures") before our key
        // is ever tried. Forcing the publickey method keeps a failed key from
        // silently falling through to a method that can't work under BatchMode.
        let cmd = ssh_command(
            &ssh_host(SshAuth::Key {
                path: "/home/me/.ssh/id_ed25519".into(),
            }),
            "echo hi",
        );
        let args = arg_strings(&cmd);
        assert!(
            args.contains(&"IdentitiesOnly=yes".to_string()),
            "key auth must set IdentitiesOnly=yes: {args:?}"
        );
        assert!(
            args.contains(&"PreferredAuthentications=publickey".to_string()),
            "key auth must force the publickey method: {args:?}"
        );
        // The configured key is still passed.
        assert!(args.iter().any(|a| a == "/home/me/.ssh/id_ed25519"));
    }

    #[test]
    fn ssh_command_agent_omits_identities_only() {
        // IdentitiesOnly=yes with no `-i` would stop ssh-agent keys from being
        // offered at all, breaking agent auth. The flag belongs ONLY on the
        // explicit-key path.
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        assert!(
            !arg_strings(&cmd).contains(&"IdentitiesOnly=yes".to_string()),
            "agent auth must not pin IdentitiesOnly (would suppress agent keys)"
        );
    }

    #[test]
    fn ssh_command_enables_tcp_keepalive() {
        // Detect a dead peer (slept laptop, dropped link) at the TCP layer, not
        // only via the app-level ServerAlive probe — so a stale pooled master is
        // torn down and replaced instead of lingering. Every mux mode carries it.
        for mux in [
            SshMux::Off,
            SshMux::Interactive,
            SshMux::Streaming,
            SshMux::Observer,
        ] {
            let cmd = ssh_command_opts(&ssh_host(SshAuth::Agent), "echo hi", mux);
            assert!(
                arg_strings(&cmd).contains(&"TCPKeepAlive=yes".to_string()),
                "TCPKeepAlive missing (mux={mux:?})"
            );
        }
    }

    #[test]
    fn pane_sample_parses_three_sections() {
        let stdout = format!(
            "claude\n{SAMPLE_BOUNDARY}\nline one\nline two\n{SAMPLE_BOUNDARY}\nviewport line\n"
        );
        let s = parse_pane_sample(&stdout).expect("well-formed sample");
        assert_eq!(s.current_command, "claude");
        // The pane section's trailing newline is consumed by the boundary
        // separator; the watchdog only does substring matching, so that's
        // contractually fine.
        assert_eq!(s.pane, "line one\nline two");
        assert_eq!(s.viewport, "viewport line\n");
    }

    #[test]
    fn pane_sample_rejects_missing_boundary() {
        // A capture race (session died mid-script) yields truncated output —
        // that must surface as a parse failure (skipped tick), never as
        // sections silently mis-attributed.
        assert!(parse_pane_sample("claude\nonly one section\n").is_none());
    }

    #[test]
    fn detects_stale_control_master_stderr() {
        // Only a failure known to happen before OpenSSH sends the remote
        // command is replayable.
        let safe = "mux_client_request_session: master alive request failed\r\n\
                    Failed to connect to new control master";
        assert!(is_mux_transport_error(safe));

        // This was the exact stderr from the reported `/api/fs/list` 400, but
        // OpenSSH emits it after sending the session request. A mutating remote
        // command may already have run, so retrying it would be unsafe.
        let ambiguous = "mux_client_request_session: read from master failed: Broken pipe\r\n\
                         Failed to connect to new control master";
        assert!(!is_mux_transport_error(ambiguous));
        // An ordinary remote failure must NOT trigger a replay (it really ran).
        assert!(!is_mux_transport_error("not a directory: /home/x/nope"));
        assert!(!is_mux_transport_error("Permission denied (publickey)."));
    }

    #[test]
    fn mux_retry_classifier_accepts_only_exact_pre_session_diagnostics() {
        for stderr in [
            "mux_client_request_session: master alive request failed",
            "mux_client_request_stdio_fwd: master alive request failed",
            "mux_client_request_alive: write packet: Broken pipe",
            "mux_client_request_alive: write packet: Connection reset by peer",
            "muxclient: master hello exchange failed",
            "Control socket connect(/tmp/cm): Connection refused",
            "Control socket connect(/tmp/cm): No such file or directory",
        ] {
            assert!(
                is_mux_transport_error(stderr),
                "missed known pre-session mux failure: {stderr}"
            );
        }
        for stderr in [
            // These can occur after the remote command was sent or started.
            "mux_client_request_session: read from master failed: Broken pipe",
            "mux_client_read_packet: read header failed: Broken pipe",
            "Connection to master closed by remote host",
            "Failed to connect to new control master",
            "Master refused session request: Permission denied",
            // Substrings and extra remote output are never enough to replay.
            "remote command mentioned mux_client_request_session in its error",
            "mux_client_request_session: master alive request failed\nremote mutation failed",
            "ssh: connect to host box port 22: Connection refused",
            "sh: tmux: command not found",
            "remote command mentioned a database control socket",
        ] {
            assert!(
                !is_mux_transport_error(stderr),
                "generic remote failure was misclassified as replay-safe: {stderr}"
            );
        }
    }

    #[test]
    fn mux_retry_requires_255_empty_stdout_and_valid_utf8_diagnostic() {
        let safe = b"mux_client_request_session: master alive request failed\n\
                     Failed to connect to new control master\n";
        assert!(is_replay_safe_mux_failure(Some(255), b"", safe));
        assert!(!is_replay_safe_mux_failure(Some(1), b"", safe));
        assert!(!is_replay_safe_mux_failure(
            Some(255),
            b"remote output means execution may have begun",
            safe
        ));
        assert!(!is_replay_safe_mux_failure(
            Some(255),
            b"",
            b"mux_client_request_session: master alive request failed\n\xff"
        ));
    }

    #[test]
    fn tmux_missing_session_classifier_accepts_only_canonical_forms() {
        for stderr in [
            "can't find session: agentum-test",
            "no server running on /tmp/tmux-501/default",
            "error connecting to /tmp/tmux-501/isolated (No such file or directory)",
            "failed to connect to server: Connection refused",
        ] {
            assert!(
                is_tmux_session_missing_error(stderr),
                "missed canonical missing-session form: {stderr}"
            );
        }
        for stderr in [
            "Permission denied (publickey).",
            "ssh: connect to host box port 22: Connection refused",
            "sh: tmux: command not found",
            "error connecting to /tmp/tmux-501/default (Permission denied)",
            "can't find session:",
            "can't find session: x\npermission denied opening tmux socket",
            // SSH notices are never fed into this tmux-only classifier. If one
            // appears here, fail closed instead of blessing mixed provenance.
            "Warning: Permanently added 'box' to the list of known hosts.\ncan't find session: x",
        ] {
            assert!(
                !is_tmux_session_missing_error(stderr),
                "unexpected failure looked like a missing tmux target: {stderr}"
            );
        }
    }

    fn has_session_record(status: i32, stderr: &str) -> String {
        let encoded: String = stderr
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("{HAS_SESSION_PROTOCOL_TAG}\t{status}\t{encoded}")
    }

    #[test]
    fn has_session_probe_is_posix_tagged_and_uses_exact_name_lookup() {
        let script = has_session_probe_script("odd target; printf injected").unwrap();
        assert!(
            script.starts_with("sh -c "),
            "probe was not POSIX-wrapped: {script}"
        );
        assert!(
            script.contains("tmux list-sessions"),
            "probe did not list exact session names: {script}"
        );
        assert!(
            script.contains("candidate_name") && script.contains("= \"$target\""),
            "probe did not exact-compare the session name: {script}"
        );
        assert!(!script.contains("tmux has-session"));
        assert!(!script.contains("-t ="));
        assert!(script.contains(HAS_SESSION_PROTOCOL_TAG));
        assert!(script.contains("od -An -v -tx1"));
        assert!(script.contains("odd target"));

        let id_script = has_session_probe_script("$17").unwrap();
        let id_lookup = exact_session_lookup_body("$17").unwrap();
        assert!(
            id_lookup.contains("\"$candidate_id\" = \"$target\""),
            "immutable control target was not matched by id: {id_script}"
        );
        for malformed in ["$", "$x", "$17suffix", "$17; kill-server"] {
            assert!(matches!(
                has_session_probe_script(malformed),
                Err(TmuxError::Parse(_))
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn has_session_probe_quotes_target_as_one_exact_tmux_argument() {
        use std::os::unix::fs::PermissionsExt;

        let root = unix_test_dir("has-session-quote");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let capture = root.join("argv");
        let injected = root.join("must-not-exist");
        let fake_tmux = bin.join("tmux");
        std::fs::write(
            &fake_tmux,
            "#!/bin/sh\nprintf '%s\\0' \"$@\" >\"$AGENTUM_CAPTURE\"\nprintf '%s' \"can't find session: probe\" >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_tmux, std::fs::Permissions::from_mode(0o700)).unwrap();
        let target = format!("odd target'; /usr/bin/touch '{}'; #", injected.display());
        let script = has_session_probe_script(&target).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("AGENTUM_CAPTURE", &capture)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "probe shell failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !injected.exists(),
            "target escaped its shell-quoted argument"
        );
        assert_eq!(
            std::fs::read(&capture).unwrap(),
            [
                b"list-sessions\0".as_slice(),
                b"-F\0".as_slice(),
                b"#{session_id}_#{session_name}\0".as_slice(),
            ]
            .concat()
        );
        assert!(
            !classify_remote_has_session_output(true, Some(0), &output.stdout, &output.stderr,)
                .unwrap()
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn exact_session_action_never_operates_on_a_prefix_match() {
        use std::os::unix::fs::PermissionsExt;

        let root = unix_test_dir("exact-session-prefix");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let capture = root.join("action-argv");
        let fake_tmux = bin.join("tmux");
        std::fs::write(
            &fake_tmux,
            "#!/bin/sh\ncase \"$1\" in\n  list-sessions) printf '%s_%s\\n' '$7' 'agentum-long' ;;\n  capture-pane) printf '%s\\0' \"$@\" >\"$AGENTUM_CAPTURE\"; printf 'captured' ;;\n  *) exit 99 ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_tmux, std::fs::Permissions::from_mode(0o700)).unwrap();
        let run = |target: &str| {
            let script = exact_session_action_script(
                target,
                "tmux capture-pane -p -t \"$session_id\"",
                None,
            )
            .unwrap();
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("AGENTUM_CAPTURE", &capture)
                .output()
                .unwrap()
        };

        let prefix = run("agentum");
        assert_eq!(
            prefix.status.code(),
            Some(1),
            "resolver shell failed: {}",
            String::from_utf8_lossy(&prefix.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&prefix.stderr).trim(),
            "can't find session: agentum"
        );
        assert!(!capture.exists(), "prefix match executed the tmux action");

        let exact = run("agentum-long");
        assert!(exact.status.success());
        assert_eq!(exact.stdout, b"captured");
        assert_eq!(
            std::fs::read(&capture).unwrap(),
            [
                b"capture-pane\0".as_slice(),
                b"-p\0".as_slice(),
                b"-t\0".as_slice(),
                b"$7\0".as_slice(),
            ]
            .concat()
        );

        let by_id = run("$7");
        assert!(by_id.status.success());
        assert_eq!(by_id.stdout, b"captured");
        assert!(std::fs::read(&capture).unwrap().ends_with(b"$7\0"));

        for malformed in ["$", "$x", "$7suffix", "$7; kill-server"] {
            assert!(matches!(
                exact_session_action_script(
                    malformed,
                    "tmux capture-pane -p -t \"$session_id\"",
                    None,
                ),
                Err(TmuxError::Parse(_))
            ));
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn has_session_protocol_ignores_banner_and_shell_noise() {
        let missing = has_session_record(1, "can't find session: absent");
        let stdout = format!("welcome from profile\n{missing}\nlast-login noise\n");
        let parsed = parse_has_session_probe(stdout.as_bytes()).unwrap();
        assert_eq!(parsed, (1, "can't find session: absent".into()));
        assert!(
            !classify_remote_has_session_output(
                true,
                Some(0),
                stdout.as_bytes(),
                b"Warning: Permanently added 'box' to known hosts\nprofile warning",
            )
            .unwrap(),
            "unrelated successful-SSH stderr hid a missing session"
        );

        let present = format!("motd\n{}\nprompt decoration\n", has_session_record(0, ""));
        assert!(
            classify_remote_has_session_output(
                true,
                Some(0),
                present.as_bytes(),
                b"login shell warning",
            )
            .unwrap()
        );
    }

    #[test]
    fn has_session_protocol_preserves_tmux_and_ssh_failures() {
        let missing = has_session_record(1, "can't find session: absent");
        match classify_remote_has_session_output(
            false,
            Some(255),
            missing.as_bytes(),
            b"Permission denied (publickey).",
        ) {
            Err(TmuxError::NonZero { status, stderr }) => {
                assert_eq!(status, 255);
                assert_eq!(stderr, "Permission denied (publickey).");
            }
            other => panic!("expected SSH transport error, got {other:?}"),
        }

        for (status, stderr) in [
            (127, "sh: tmux: command not found"),
            (
                1,
                "error connecting to /tmp/tmux/default (Permission denied)",
            ),
        ] {
            match classify_remote_has_session_output(
                true,
                Some(0),
                has_session_record(status, stderr).as_bytes(),
                b"ignored successful-SSH noise",
            ) {
                Err(TmuxError::NonZero {
                    status: actual,
                    stderr: actual_stderr,
                }) => {
                    assert_eq!(actual, status);
                    assert_eq!(actual_stderr, stderr);
                }
                other => panic!("expected typed status {status} error, got {other:?}"),
            }
        }
    }

    #[test]
    fn has_session_protocol_rejects_missing_or_malformed_records() {
        let malformed = [
            "banner only".to_string(),
            format!("{HAS_SESSION_PROTOCOL_TAG}\tbogus\t00"),
            format!("{HAS_SESSION_PROTOCOL_TAG}\t256\t00"),
            format!("{HAS_SESSION_PROTOCOL_TAG}\t1\t0"),
            format!("{HAS_SESSION_PROTOCOL_TAG}\t1\tzz"),
        ];
        for stdout in malformed {
            match classify_remote_has_session_output(true, Some(0), stdout.as_bytes(), b"") {
                Err(TmuxError::NonZero { status, .. }) => assert_eq!(status, -1),
                other => panic!("malformed protocol was accepted: {other:?}"),
            }
        }
    }

    /// Password auth feeds the secret through OpenSSH's own SSH_ASKPASS helper
    /// (no external `sshpass`): plain `ssh`, both password-backed methods are
    /// explicitly enabled, and child env contains only an owner-only file
    /// locator — never the password itself.
    #[cfg(unix)]
    #[test]
    fn ssh_command_password_uses_askpass_not_sshpass() {
        let PreparedSshCommand {
            command: cmd,
            password_secret,
        } = try_ssh_command_opts(
            &ssh_host(SshAuth::Password {
                password: "s3cret".into(),
            }),
            "echo hi",
            SshMux::Interactive,
        )
        .unwrap();
        let secret = password_secret.expect("password command owns a secret guard");
        // Plain ssh now — sshpass is gone.
        assert_eq!(cmd.as_std().get_program().to_string_lossy(), "ssh");
        let args = arg_strings(&cmd);
        assert!(
            !args.iter().any(|a| a.contains("sshpass")),
            "must not shell through sshpass: {args:?}"
        );
        // BatchMode=no so ssh actually prompts (firing the askpass helper);
        // force the password method so it never silently tries a key first.
        assert!(args.contains(&"BatchMode=no".to_string()));
        assert!(!args.contains(&"BatchMode=yes".to_string()));
        assert!(
            args.contains(&"PreferredAuthentications=password,keyboard-interactive".to_string())
        );
        assert!(args.contains(&"PubkeyAuthentication=no".to_string()));
        assert!(args.contains(&"PasswordAuthentication=yes".to_string()));
        assert!(args.contains(&"KbdInteractiveAuthentication=yes".to_string()));
        assert!(args.iter().any(|a| a == "me@box.local"));
        // Neither argv nor any explicit child env value contains the password.
        assert!(
            !args.iter().any(|a| a.contains("s3cret")),
            "password must never reach the argv: {args:?}"
        );
        let envs = env_map(&cmd);
        assert!(
            envs.values().all(|value| !value.contains("s3cret")),
            "password leaked into SSH process env: {envs:?}"
        );
        assert_eq!(
            envs.get("SSH_ASKPASS_REQUIRE").map(String::as_str),
            Some("force")
        );
        let secret_path_text = secret.path.to_string_lossy().into_owned();
        assert_eq!(
            envs.get(ASKPASS_SECRET_ENV).map(String::as_str),
            Some(secret_path_text.as_str())
        );
        assert_eq!(std::fs::read_to_string(&secret.path).unwrap(), "s3cret");
        let askpass = envs
            .get("SSH_ASKPASS")
            .expect("SSH_ASKPASS set for password auth");
        assert!(
            askpass.ends_with("askpass.sh"),
            "unexpected askpass path: {askpass}"
        );
        // Pooling still applies on the password path.
        assert_control_master(&args);
        let secret_path = secret.path.clone();
        drop(secret);
        assert!(
            !secret_path.exists(),
            "password file survived its command guard"
        );
    }

    /// Key/agent auth must NOT wire up the askpass env (it never prompts, so
    /// there is no password to feed).
    #[cfg(unix)]
    #[test]
    fn ssh_command_key_sets_no_askpass_env() {
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        let envs = env_map(&cmd);
        assert!(
            !envs.contains_key("SSH_ASKPASS"),
            "key/agent auth must not set SSH_ASKPASS"
        );
        assert!(!envs.contains_key(ASKPASS_SECRET_ENV));
    }

    /// The askpass helper is written executable (0700) and idempotently. It
    /// reads a unique secret file and therefore supports repeated prompts.
    #[cfg(unix)]
    #[test]
    fn askpass_script_written_executable_and_idempotent() {
        use std::os::unix::fs::PermissionsExt;
        let root = unix_test_dir("askpass-content");
        let dir = root.join("runtime");
        let path = askpass_script_path_in(&dir).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.starts_with("#!/bin/sh"),
            "missing shebang: {content}"
        );
        assert!(
            content.contains(ASKPASS_SECRET_ENV),
            "script must read the secret locator: {content}"
        );
        assert!(content.contains("cat \"$secret\""));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "askpass must be owner-only executable");
        // Idempotent: a second call returns the same path without error.
        assert_eq!(askpass_script_path_in(&dir).unwrap(), path);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn askpass_secret_is_unique_owner_only_repeatable_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;

        let root = unix_test_dir("askpass-secret");
        let dir = root.join("runtime");
        let helper = askpass_script_path_in(&dir).unwrap();
        let first = stage_password_secret_in(&dir, "first password").unwrap();
        let second = stage_password_secret_in(&dir, "second password").unwrap();
        assert_ne!(first.path, second.path, "password files were not unique");
        assert_eq!(
            std::fs::symlink_metadata(&first.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );

        for _ in 0..2 {
            let output = std::process::Command::new(&helper)
                .env(ASKPASS_SECRET_ENV, &first.path)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"first password");
            assert!(output.stderr.is_empty());
        }

        let first_path = first.path.clone();
        let second_path = second.path.clone();
        drop(first);
        drop(second);
        assert!(!first_path.exists());
        assert!(!second_path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn askpass_secret_bounded_cleanup_removes_unawaited_command_secret() {
        let root = unix_test_dir("askpass-secret-deadline");
        let dir = root.join("runtime");
        let secret = stage_password_secret_in(&dir, "deadline password").unwrap();
        let path = secret.path.clone();
        secret
            .schedule_bounded_cleanup(Duration::from_millis(20))
            .unwrap();

        for _ in 0..100 {
            if !path.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!path.exists(), "bounded password cleanup never ran");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn askpass_secret_cleanup_never_follows_a_replacement_symlink() {
        use std::os::unix::fs::symlink;

        let root = unix_test_dir("askpass-secret-replacement");
        let dir = root.join("runtime");
        let mut secret = stage_password_secret_in(&dir, "temporary").unwrap();
        let outside = root.join("outside");
        std::fs::write(&outside, "must survive").unwrap();
        std::fs::remove_file(&secret.path).unwrap();
        symlink(&outside, &secret.path).unwrap();

        let error = secret.remove_now().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "must survive");
        assert!(
            std::fs::symlink_metadata(&secret.path)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        // The safety behavior was asserted; disarm Drop so the deliberate
        // replacement does not produce a second warning in this test.
        secret.remove_on_drop = false;
        std::fs::remove_file(&secret.path).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn askpass_secret_cleanup_never_deletes_a_replacement_regular_file() {
        let root = unix_test_dir("askpass-secret-inode");
        let dir = root.join("runtime");
        let mut secret = stage_password_secret_in(&dir, "temporary").unwrap();
        let replacement = dir.join("replacement");
        std::fs::write(&replacement, "replacement must survive").unwrap();
        std::fs::rename(&replacement, &secret.path).unwrap();

        let error = secret.remove_now().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            std::fs::read_to_string(&secret.path).unwrap(),
            "replacement must survive"
        );

        secret.remove_on_drop = false;
        std::fs::remove_file(&secret.path).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn askpass_repairs_owner_controlled_directory_and_file_modes() {
        use std::os::unix::fs::PermissionsExt;

        let root = unix_test_dir("askpass-mode-repair");
        let dir = root.join("runtime");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let helper = dir.join("askpass.sh");
        std::fs::write(&helper, ASKPASS_SCRIPT).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(askpass_script_path_in(&dir).unwrap(), helper);
        let dir_mode = std::fs::symlink_metadata(&dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        let helper_mode = std::fs::symlink_metadata(&helper)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(dir_mode, 0o700, "askpass directory mode was not repaired");
        assert_eq!(helper_mode, 0o700, "askpass helper mode was not repaired");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn askpass_replaces_symlink_instead_of_reusing_its_target() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let root = unix_test_dir("askpass-symlink");
        let dir = root.join("runtime");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let outside = root.join("outside-helper");
        std::fs::write(&outside, ASKPASS_SCRIPT).unwrap();
        let helper = dir.join("askpass.sh");
        symlink(&outside, &helper).unwrap();

        assert_eq!(askpass_script_path_in(&dir).unwrap(), helper);
        let metadata = std::fs::symlink_metadata(&helper).unwrap();
        assert!(metadata.file_type().is_file());
        assert!(!metadata.file_type().is_symlink());
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o700);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), ASKPASS_SCRIPT);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn askpass_preparation_fails_when_helper_path_cannot_be_repaired() {
        let root = unix_test_dir("askpass-fail-closed");
        let dir = root.join("runtime");
        std::fs::create_dir_all(dir.join("askpass.sh")).unwrap();

        let error = askpass_script_path_in(&dir).unwrap_err();
        assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(dir.join("askpass.sh").is_dir());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn password_preflight_failure_command_is_immediate_and_noninteractive() {
        let mut command = password_auth_preflight_failure_command(&std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "unsafe helper directory",
        ));
        let output = tokio::time::timeout(Duration::from_secs(1), command.output())
            .await
            .expect("preflight failure command blocked")
            .unwrap();
        assert_eq!(output.status.code(), Some(255));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("cannot prepare noninteractive SSH password helper"));
        assert!(stderr.contains("unsafe helper directory"));
    }

    #[test]
    fn control_path_template_creates_dir_idempotently() {
        // Calling twice must not panic even though the dir already exists.
        let host = ssh_host(SshAuth::Agent);
        let first = control_path_template(&host);
        let second = control_path_template(&host);
        assert_eq!(first, second);
        // HOME is set in the test environment, so a path is available.
        let path = first.expect("control path available with HOME set");
        assert!(
            std::path::Path::new(&path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("cm-")
        );
        let dir = std::path::Path::new(&path).parent().expect("dir");
        assert!(dir.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn legacy_control_roots_cover_xdg_and_home_across_environment_changes() {
        use std::ffi::OsStr;

        let both = legacy_control_socket_dirs_from(
            Some(OsStr::new("/run/user/1000")),
            Some(OsStr::new("/Users/tester")),
        );
        assert_eq!(
            both,
            vec![
                PathBuf::from("/run/user/1000/agentum-ssh"),
                PathBuf::from("/Users/tester/.agentum/ssh"),
            ],
            "setting XDG must not hide a still-live HOME-root legacy master"
        );

        assert_eq!(
            legacy_control_socket_dirs_from(None, Some(OsStr::new("/Users/tester"))),
            vec![PathBuf::from("/Users/tester/.agentum/ssh")],
            "removing XDG must retain the historic HOME fallback"
        );
        assert_eq!(
            legacy_control_socket_dirs_from(Some(OsStr::new("/run/user/1000")), None),
            vec![PathBuf::from("/run/user/1000/agentum-ssh")],
            "an unavailable HOME must not hide the historic XDG root"
        );
        assert_eq!(
            both.iter().collect::<std::collections::HashSet<_>>().len(),
            both.len(),
            "historical roots were not deduplicated"
        );
        assert_eq!(
            legacy_control_socket_dirs_from(
                Some(OsStr::new("relative-xdg")),
                Some(OsStr::new("relative-home")),
            ),
            Vec::<PathBuf>::new(),
            "relative environment roots must never authorize socket cleanup"
        );
    }

    #[cfg(unix)]
    #[test]
    fn control_path_validates_real_owner_only_directory_for_every_auth_kind() {
        use std::os::unix::fs::PermissionsExt;

        for (label, auth) in [
            ("agent", SshAuth::Agent),
            (
                "key",
                SshAuth::Key {
                    path: "/tmp/key".into(),
                },
            ),
            (
                "password",
                SshAuth::Password {
                    password: "not exposed".into(),
                },
            ),
        ] {
            let root = unix_short_control_dir();
            let dir = root.join("runtime");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
            let path = control_path_template_in(&ssh_host(auth), "cm", &dir)
                .unwrap_or_else(|| panic!("{label} auth did not get a safe ControlPath"));
            assert!(
                std::path::Path::new(&path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("cm-")
            );
            assert_eq!(
                std::fs::symlink_metadata(&dir)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o700,
                "{label} auth left a permissive ControlPath directory"
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn control_path_rejects_symlink_directory_without_touching_target() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let root = unix_test_dir("control-dir-symlink");
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.join("runtime");
        symlink(&target, &link).unwrap();

        assert!(
            control_path_template_in(&ssh_host(SshAuth::Agent), "cm", &link).is_none(),
            "symlink ControlPath directory was trusted"
        );
        assert!(
            legacy_control_path_template_in("cm", &link).is_none(),
            "legacy cleanup trusted a symlink ControlPath directory"
        );
        assert_eq!(
            std::fs::symlink_metadata(&target)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o755,
            "validation followed and chmodded the symlink target"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn control_path_enforces_exact_long_home_socket_boundary() {
        // Worst-case `cms-` leaf is 20 bytes: four-byte prefix plus the
        // 16-hex digest. A 79-byte fallback directory therefore yields the
        // exact accepted 100-byte socket path; one more byte must disable mux.
        let host = ssh_host(SshAuth::Agent);
        let accepted_dir = unix_control_dir_with_len(79);
        let rejected_dir = unix_control_dir_with_len(80);
        let accepted = control_path_template_in(&host, "cms", &accepted_dir)
            .expect("exact 100-byte ControlPath was rejected");
        assert_eq!(accepted.len(), 100);
        assert!(control_path_template_in(&host, "cms", &rejected_dir).is_none());

        std::fs::remove_dir_all(accepted_dir).unwrap();
        std::fs::remove_dir_all(rejected_dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn legacy_control_path_accounts_for_percent_c_expansion_at_socket_boundary() {
        // `cms-%C` expands to four literal bytes plus OpenSSH's 40-hex hash.
        // Including the path separator, a 55-byte private directory therefore
        // lands exactly on our conservative 100-byte socket cap.
        let accepted_dir = unix_control_dir_with_len(55);
        let rejected_dir = unix_control_dir_with_len(56);
        let accepted = legacy_control_path_template_in("cms", &accepted_dir)
            .expect("exact 100-byte expanded legacy ControlPath was rejected");
        assert_eq!(accepted.len() - "%C".len() + 40, 100);
        assert!(legacy_control_path_template_in("cms", &rejected_dir).is_none());

        std::fs::remove_dir_all(accepted_dir).unwrap();
        std::fs::remove_dir_all(rejected_dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn old_host_value_still_resolves_the_original_cleanup_socket() {
        let root = unix_short_control_dir();
        let dir = root.join("runtime");
        let mut old = ssh_host(SshAuth::Agent);
        old.id = "00000000-0000-0000-0000-000000000010".parse().unwrap();
        let opened = control_path_template_in(&old, "cm", &dir).unwrap();

        let mut edited = old.clone();
        let HostKind::Ssh { hostname, .. } = &mut edited.kind else {
            unreachable!()
        };
        *hostname = "new-destination.local".into();
        let replacement = control_path_template_in(&edited, "cm", &dir).unwrap();
        let old_cleanup = control_path_template_in(&old, "cm", &dir).unwrap();
        assert_eq!(opened, old_cleanup);
        assert_ne!(opened, replacement);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timed_out_fake_ssh_child_is_explicitly_killed_and_reaped() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Stdio;

        let nonce = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(format!("agentum-fake-ssh-{nonce}"));
        std::fs::create_dir(&dir).unwrap();
        let fake_ssh = dir.join("ssh");
        std::fs::write(&fake_ssh, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&fake_ssh, std::fs::Permissions::from_mode(0o700)).unwrap();

        let child = spawn_output_child(Command::new(&fake_ssh)).unwrap();
        let pid = child.id().expect("fake ssh child has a process id");
        let err =
            child_output_with_timeout(child, Duration::from_millis(300), "fake ssh timed out")
                .await
                .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);

        let mut alive = true;
        for _ in 0..100 {
            alive = std::process::Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let _ = std::fs::remove_file(&fake_ssh);
        let _ = std::fs::remove_dir(&dir);
        assert!(!alive, "timed-out fake ssh process {pid} was left running");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn captured_child_drains_large_stdout_and_stderr_without_deadlock() {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(
            "i=0; while [ \"$i\" -lt 10000 ]; do \
             printf 'stdout-0123456789abcdef\\n'; \
             printf 'stderr-0123456789abcdef\\n' >&2; \
             i=$((i + 1)); done",
        );
        let child = spawn_output_child(command).unwrap();
        let output =
            child_output_with_timeout(child, Duration::from_secs(5), "large fake ssh timed out")
                .await
                .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.len() > 64 * 1024);
        assert!(output.stderr.len() > 64 * 1024);
        assert!(output.stdout.ends_with(b"stdout-0123456789abcdef\n"));
        assert!(output.stderr.ends_with(b"stderr-0123456789abcdef\n"));
    }

    fn local_host() -> Host {
        Host {
            id: agentum_core::LOCAL_HOST_ID,
            name: "local".into(),
            kind: HostKind::Local,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    #[test]
    fn local_forward_cmd_builds_dash_l_to_host_loopback() {
        // CDP lives on the host, so the Mac reaches it via a *local* (-L)
        // forward: the Mac listens on `mac_port` and ssh tunnels to the host's
        // `127.0.0.1:host_port`. Mirror of the reverse (-R) MCP tunnel.
        let cmd = ssh_control_local_forward_cmd(&ssh_host(SshAuth::Agent), 7000, 9222)
            .expect("ssh host yields a command");
        assert_eq!(cmd.as_std().get_program().to_string_lossy(), "ssh");
        let args = arg_strings(&cmd);
        assert!(args.contains(&"-O".to_string()), "missing -O: {args:?}");
        assert!(
            args.contains(&"forward".to_string()),
            "missing forward: {args:?}"
        );
        assert!(args.contains(&"-L".to_string()), "missing -L: {args:?}");
        // listen on Mac loopback:mac_port → host loopback:host_port.
        assert!(
            args.contains(&"127.0.0.1:7000:127.0.0.1:9222".to_string()),
            "wrong -L spec: {args:?}"
        );
        // Must NOT be a reverse forward.
        assert!(
            !args.contains(&"-R".to_string()),
            "-L builder emitted -R: {args:?}"
        );
        // Rides the warm Interactive master (cm-, not the streaming cms-).
        let control_path = args
            .iter()
            .find(|a| a.starts_with("ControlPath="))
            .expect("ControlPath present");
        assert!(
            control_path.contains("/cm-"),
            "not interactive master: {control_path}"
        );
        assert!(
            !control_path.contains("/cms-"),
            "must not use streaming master: {control_path}"
        );
        // Host identity must agree with how the master was opened.
        assert!(
            args.iter().any(|a| a == "2222"),
            "host port missing: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "me@box.local"),
            "user@host missing: {args:?}"
        );
    }

    #[test]
    fn control_exit_commands_target_all_host_masters() {
        let host = ssh_host(SshAuth::Agent);
        let cases = [
            (SshMux::Interactive, "/cm-"),
            (SshMux::Streaming, "/cms-"),
            (SshMux::Observer, "/cmo-"),
        ];
        for (mux, socket_marker) in cases {
            let cmd = ssh_control_exit_cmd(&host, mux).expect("ssh host yields exit command");
            let args = arg_strings(&cmd);
            assert_ignores_user_config(&args);
            assert!(args.contains(&"-T".to_string()), "missing -T: {args:?}");
            assert!(args.contains(&"-O".to_string()), "missing -O: {args:?}");
            assert!(args.contains(&"exit".to_string()), "missing exit: {args:?}");
            let control_path = args
                .iter()
                .find(|arg| arg.starts_with("ControlPath="))
                .expect("ControlPath present");
            assert!(
                control_path.contains(socket_marker),
                "wrong {mux:?} socket: {control_path}"
            );
            assert!(
                args.contains(&"2222".to_string()),
                "host port missing: {args:?}"
            );
            assert!(
                args.contains(&"me@box.local".to_string()),
                "host identity missing: {args:?}"
            );
        }
        assert!(ssh_control_exit_cmd(&host, SshMux::Off).is_none());
        assert!(ssh_control_exit_cmd(&local_host(), SshMux::Interactive).is_none());
    }

    #[test]
    fn legacy_control_exit_commands_target_only_exact_historic_host_sockets() {
        let host = ssh_host(SshAuth::Agent);
        for (mux, prefix, expected_leaf) in [
            (SshMux::Interactive, "cm", "cm-%C"),
            (SshMux::Streaming, "cms", "cms-%C"),
        ] {
            let expected_paths: Vec<_> = legacy_control_socket_dirs()
                .into_iter()
                .filter_map(|dir| legacy_control_path_template_in(prefix, &dir))
                .collect();
            let commands = ssh_legacy_control_exit_cmds(&host, mux);
            assert_eq!(commands.len(), expected_paths.len());
            for (cmd, expected_path) in commands.iter().zip(expected_paths) {
                assert_eq!(cmd.as_std().get_program().to_string_lossy(), "ssh");
                let args = arg_strings(cmd);
                assert_ignores_user_config(&args);
                assert!(args.contains(&"-T".to_string()), "missing -T: {args:?}");
                assert!(args.contains(&"-O".to_string()), "missing -O: {args:?}");
                assert!(args.contains(&"exit".to_string()), "missing exit: {args:?}");
                assert!(args.contains(&"2222".to_string()), "wrong port: {args:?}");
                assert!(
                    args.contains(&"me@box.local".to_string()),
                    "wrong endpoint: {args:?}"
                );

                let path_arg = args
                    .iter()
                    .find_map(|arg| arg.strip_prefix("ControlPath="))
                    .expect("ControlPath present");
                let path = Path::new(path_arg);
                assert_eq!(path, Path::new(&expected_path));
                assert_eq!(
                    path.file_name().and_then(|leaf| leaf.to_str()),
                    Some(expected_leaf)
                );
                assert!(
                    !path_arg.contains('*') && !path_arg.contains('?') && !path_arg.contains('['),
                    "legacy cleanup used a glob-like path: {path_arg}"
                );
            }
        }
        assert!(ssh_legacy_control_exit_cmds(&host, SshMux::Off).is_empty());
        assert!(ssh_legacy_control_exit_cmds(&local_host(), SshMux::Interactive).is_empty());
    }

    #[test]
    fn control_exit_status_is_idempotent_only_for_exact_absent_socket_errors() {
        assert!(classify_control_exit_output(true, Some(0), b"ignored", b"ignored").is_ok());
        for stderr in [
            b"Control socket connect(/tmp/cm): No such file or directory\n".as_slice(),
            b"Control socket connect(/tmp/cm): Connection refused\r\n".as_slice(),
        ] {
            assert!(
                classify_control_exit_output(false, Some(255), b"", stderr).is_ok(),
                "already-absent master should be idempotent: {}",
                String::from_utf8_lossy(stderr)
            );
        }

        for (status, stdout, stderr) in [
            (
                Some(255),
                b"".as_slice(),
                b"Master refused termination request: Permission denied".as_slice(),
            ),
            (
                Some(255),
                b"unexpected stdout".as_slice(),
                b"Control socket connect(/tmp/cm): No such file or directory".as_slice(),
            ),
            (
                Some(1),
                b"".as_slice(),
                b"Control socket connect(/tmp/cm): No such file or directory".as_slice(),
            ),
            (
                Some(255),
                b"".as_slice(),
                b"Control socket connect(/tmp/cm): Permission denied".as_slice(),
            ),
            (
                Some(255),
                b"".as_slice(),
                b"warning\nControl socket connect(/tmp/cm): Connection refused".as_slice(),
            ),
        ] {
            let error = classify_control_exit_output(false, status, stdout, stderr)
                .expect_err("unexpected control-exit failure was ignored");
            assert_eq!(error.kind(), std::io::ErrorKind::Other);
        }
    }

    #[tokio::test]
    async fn close_control_masters_is_a_bounded_noop_for_local_hosts() {
        ssh_close_control_masters(&local_host(), Duration::ZERO)
            .await
            .unwrap();
        ssh_retire_legacy_control_masters(&local_host(), Duration::ZERO)
            .await
            .unwrap();
    }

    #[test]
    fn reverse_forward_and_cancel_use_the_same_full_spec() {
        let host = ssh_host(SshAuth::Agent);
        let forward =
            ssh_control_forward_cmd(&host, 8990, 50736).expect("ssh host yields a forward command");
        let cancel =
            ssh_control_cancel_cmd(&host, 8990, 50736).expect("ssh host yields a cancel command");
        let spec = "127.0.0.1:8990:127.0.0.1:50736".to_string();

        let forward_args = arg_strings(&forward);
        assert_ignores_user_config(&forward_args);
        assert!(
            forward_args.contains(&"forward".to_string()),
            "missing forward operation: {forward_args:?}"
        );
        assert!(
            forward_args.contains(&"-R".to_string()),
            "missing reverse-forward flag: {forward_args:?}"
        );
        assert!(
            forward_args.contains(&spec),
            "forward must pass the full reverse-forward spec: {forward_args:?}"
        );

        let cancel_args = arg_strings(&cancel);
        assert_ignores_user_config(&cancel_args);
        assert!(
            cancel_args.contains(&"cancel".to_string()),
            "missing cancel operation: {cancel_args:?}"
        );
        assert!(
            cancel_args.contains(&"-R".to_string()),
            "missing reverse-forward flag: {cancel_args:?}"
        );
        assert!(
            cancel_args.contains(&spec),
            "OpenSSH rejects a listen-side-only reverse cancel: {cancel_args:?}"
        );
    }

    #[test]
    fn local_cancel_cmd_uses_full_local_forward_spec() {
        // OpenSSH rejects a listen-side-only `-O cancel -L` ("Bad local
        // forwarding specification"); both forward directions need the SAME
        // full spec used to arm them. Verified against OpenSSH 10.0p2.
        let cmd = ssh_control_local_cancel_cmd(&ssh_host(SshAuth::Agent), 7000, 9222)
            .expect("ssh host yields a command");
        let args = arg_strings(&cmd);
        assert!(args.contains(&"-O".to_string()), "missing -O: {args:?}");
        assert!(
            args.contains(&"cancel".to_string()),
            "missing cancel: {args:?}"
        );
        assert!(args.contains(&"-L".to_string()), "missing -L: {args:?}");
        assert!(
            args.contains(&"127.0.0.1:7000:127.0.0.1:9222".to_string()),
            "cancel must pass the full local-forward spec: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "me@box.local"),
            "user@host missing: {args:?}"
        );
    }

    #[test]
    fn local_forward_and_cancel_none_for_local_host() {
        // No tunnel for a local host — there is no ssh master to attach to.
        assert!(ssh_control_forward_cmd(&local_host(), 8990, 50736).is_none());
        assert!(ssh_control_cancel_cmd(&local_host(), 8990, 50736).is_none());
        assert!(ssh_control_local_forward_cmd(&local_host(), 7000, 9222).is_none());
        assert!(ssh_control_local_cancel_cmd(&local_host(), 7000, 9222).is_none());
    }
}
