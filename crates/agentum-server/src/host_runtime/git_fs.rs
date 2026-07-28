//! Host-aware git, gh, and filesystem operations.
use super::*;

// ───────────────────────── host-aware git / fs ─────────────────────────
// Generic command plumbing so the repo/worktree/git routes run their git
// (and the few fs touches around it) on the *repo's host*: directly when
// the host is `Local`, over `ssh` when it's `Ssh`. The SSH form always
// wraps in `sh -c` for the same reason every other remote path does — the
// login shell may be fish/zsh, which reject the bash/POSIX `&&`/`cd` we
// build here. See `fs::list_remote_dir`.

/// `git worktree add` (and a clone-from-scratch checkout) can take far
/// longer than the 12s probe budget, so host-aware git gets its own,
/// roomier timeout. Still bounded so a hung remote can't wedge a request.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
/// Long-running repository commands may include full builds and browser suites.
/// Do not inherit the short git transport budget.
const REPOSITORY_COMMAND_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Captured output of a command run on a host. Unlike [`ssh_stdout`], a
/// non-zero exit is NOT an error — callers inspect `success`/`stderr`
/// themselves, because git uses exit codes to signal *expected* states
/// (a branch that "already exists", a path absent at a revision, …).
#[derive(Debug)]
pub struct HostCommandOutput {
    pub success: bool,
    /// Process exit code, when known. For SSH this is the *remote* command's
    /// code (ssh forwards it). `None` if the process was signalled. Callers
    /// that branch on specific codes (e.g. `git check-ignore`: 0/1/≥2) need
    /// this; most only read `success`.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl HostCommandOutput {
    /// stdout as lossy UTF-8 (callers trim as needed).
    pub fn stdout_string(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Run an arbitrary argv in `cwd` on `host` and capture its output. This is
/// the host-aware command seam used for typed verification commands.
/// Arguments and environment values are shell-quoted independently on SSH;
/// environment keys are restricted to portable identifier characters.
pub async fn command_in_dir(
    host: &Host,
    cwd: &str,
    program: &str,
    args: &[String],
    env: &[(String, String)],
) -> Result<HostCommandOutput> {
    match &host.kind {
        HostKind::Local => {
            let out = Command::new(program)
                .args(args)
                .envs(env.iter().map(|(k, v)| (k, v)))
                .current_dir(cwd)
                .output()
                .await?;
            Ok(HostCommandOutput {
                success: out.status.success(),
                code: out.status.code(),
                stdout: out.stdout,
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            })
        }
        HostKind::Ssh { .. } => {
            let mut inner = format!("cd {} && env", q(cwd)?);
            for (key, value) in env {
                if key.is_empty()
                    || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    || key.as_bytes()[0].is_ascii_digit()
                {
                    return Err(HostRuntimeError::Quote);
                }
                inner.push(' ');
                inner.push_str(key);
                inner.push('=');
                inner.push_str(&q(value)?);
            }
            inner.push(' ');
            inner.push_str(&q(program)?);
            for arg in args {
                inner.push(' ');
                inner.push_str(&q(arg)?);
            }
            let script = format!("sh -c {}", q(&inner)?);
            let out = ssh_output(host, &script, REPOSITORY_COMMAND_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            Ok(HostCommandOutput {
                success: out.status.success(),
                code: out.status.code(),
                stdout: out.stdout,
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            })
        }
    }
}

/// Run `git <args>` with `cwd` as the working directory on `host`.
/// Local → `git -C <cwd> <args>`; SSH → `sh -c 'cd <cwd> && git <args>'`
/// with every token shell-quoted. A non-zero git exit is reported via
/// `success`, not as an `Err` (only transport/timeout failures error).
pub async fn git_in_dir(host: &Host, cwd: &str, args: &[&str]) -> Result<HostCommandOutput> {
    match &host.kind {
        HostKind::Local => {
            let out = Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .output()
                .await?;
            Ok(HostCommandOutput {
                success: out.status.success(),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                stdout: out.stdout,
            })
        }
        HostKind::Ssh { .. } => {
            let mut inner = format!("cd {} && git", q(cwd)?);
            for a in args {
                inner.push(' ');
                inner.push_str(&q(a)?);
            }
            let script = format!("sh -c {}", q(&inner)?);
            let out = ssh_output(host, &script, GIT_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            Ok(HostCommandOutput {
                success: out.status.success(),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                stdout: out.stdout,
            })
        }
    }
}

/// Run `gh <args>` with `cwd` as the working directory on `host` — the
/// host-aware analogue of [`git_in_dir`], for the remote GitHub-issue path
/// (spec 018 S3 / AC-6). Local → `gh` in `cwd`; SSH → `sh -c 'cd <cwd> && gh
/// <args>'` with every token shell-quoted. A non-zero `gh` exit is reported via
/// `success`, not as an `Err` (only transport/timeout failures error), so the
/// caller can surface `gh`'s stderr as a typed error instead of a 500.
///
/// `gh` runs on the remote with that host's own auth/PATH (the same way the
/// local `TaskSink::Github` uses the repo's local `gh` auth) — agentum does not
/// forward credentials.
pub async fn gh_in_dir(host: &Host, cwd: &str, args: &[&str]) -> Result<HostCommandOutput> {
    match &host.kind {
        HostKind::Local => {
            // Honor `AGENTUM_GH_BIN` (defaults to "gh") — the SAME test seam
            // `task_sink::gh_bin()` exposes, so a fake `gh` stubs the local issue
            // fetch in the start-work live test (spec 008 F1 §B.3). Production is
            // byte-identical: the var is unset, so this is exactly `gh`.
            let program = std::env::var("AGENTUM_GH_BIN").unwrap_or_else(|_| "gh".into());
            let mut command = Command::new(program);
            command.current_dir(cwd).args(args).kill_on_drop(true);
            let out = timeout(GIT_TIMEOUT, command.output())
                .await
                .map_err(|_| HostRuntimeError::Timeout)??;
            Ok(HostCommandOutput {
                success: out.status.success(),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                stdout: out.stdout,
            })
        }
        HostKind::Ssh { .. } => {
            let mut inner = format!("cd {} && gh", q(cwd)?);
            for a in args {
                inner.push(' ');
                inner.push_str(&q(a)?);
            }
            let script = format!("sh -c {}", q(&inner)?);
            let out = ssh_output(host, &script, GIT_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            Ok(HostCommandOutput {
                success: out.status.success(),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                stdout: out.stdout,
            })
        }
    }
}

/// True when `cwd` is inside a git work tree on `host`
/// (`git rev-parse --is-inside-work-tree`). Host-aware replacement for
/// `crate::git::is_git_repo`, which is local-only.
pub async fn is_git_repo(host: &Host, cwd: &str) -> bool {
    git_in_dir(host, cwd, &["rev-parse", "--is-inside-work-tree"])
        .await
        .map(|o| o.success)
        .unwrap_or(false)
}

/// `mkdir -p <path>` on `host`. The worktree routes need the
/// `.claude/worktrees` parent to exist before `git worktree add`.
pub async fn mkdir_p(host: &Host, path: &str) -> Result<()> {
    match &host.kind {
        HostKind::Local => {
            tokio::fs::create_dir_all(path).await?;
            Ok(())
        }
        HostKind::Ssh { .. } => {
            let script = format!("sh -c {}", q(&format!("mkdir -p {}", q(path)?))?);
            ssh_checked(host, &script).await
        }
    }
}

/// Read a file's raw bytes from `host`, or `None` when it doesn't exist.
/// Used for the `worktree` revision of a git diff (the on-disk content,
/// which may differ from index/HEAD). SSH reads via `cat`; a missing file
/// exits non-zero → `None`, mirroring the local `NotFound` branch.
pub async fn read_file_bytes(host: &Host, abs_path: &str) -> Result<Option<Vec<u8>>> {
    match &host.kind {
        HostKind::Local => match tokio::fs::read(abs_path).await {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        },
        HostKind::Ssh { .. } => {
            let script = format!("sh -c {}", q(&format!("cat {}", q(abs_path)?))?);
            let out = ssh_output(host, &script, GIT_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            Ok(out.status.success().then_some(out.stdout))
        }
    }
}

/// Read one worktree-relative regular file without letting the relative path,
/// or a symlink placed in the worktree, escape `root`. This is deliberately a
/// separate API from [`read_file_bytes`]: the latter is the authenticated file
/// editor's exact-path capability, while this function enforces repository
/// containment for Git routes.
pub async fn read_file_bytes_beneath(
    host: &Host,
    root: &str,
    relative: &str,
) -> Result<Option<Vec<u8>>> {
    validate_beneath_relative(relative)?;
    match &host.kind {
        HostKind::Local => read_local_file_beneath(Path::new(root), Path::new(relative)),
        HostKind::Ssh { .. } => {
            let script = remote_beneath_script(root, relative, RemoteBeneathOperation::Read)?;
            let out = ssh_output(host, &script, GIT_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            match out.status.code() {
                Some(0) => Ok(Some(out.stdout)),
                Some(66) => Ok(None),
                Some(44 | 65 | 69) => Err(HostRuntimeError::UnsafePath(relative.into())),
                status => Err(HostRuntimeError::NonZero {
                    status,
                    stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
                }),
            }
        }
    }
}

/// Whether `abs_path` exists on `host` (`test -e` over SSH). Used by the
/// diff route to decide whether an empty `git diff` means "untracked file"
/// (so it can synthesise a diff) versus "no change".
pub async fn path_exists(host: &Host, abs_path: &str) -> Result<bool> {
    match &host.kind {
        HostKind::Local => Ok(tokio::fs::try_exists(abs_path).await.unwrap_or(false)),
        HostKind::Ssh { .. } => {
            let script = format!("sh -c {}", q(&format!("test -e {}", q(abs_path)?))?);
            let out = ssh_output(host, &script, SSH_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            Ok(out.status.success())
        }
    }
}

/// Test one worktree-relative entry without following a symlink outside the
/// registered checkout. The final entry itself may be a symlink (Git models
/// symlinks as entries), but no parent link is traversed.
pub async fn path_exists_beneath(host: &Host, root: &str, relative: &str) -> Result<bool> {
    validate_beneath_relative(relative)?;
    match &host.kind {
        HostKind::Local => path_exists_local_beneath(Path::new(root), Path::new(relative)),
        HostKind::Ssh { .. } => {
            let script = remote_beneath_script(root, relative, RemoteBeneathOperation::Exists)?;
            let out = ssh_output(host, &script, SSH_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            match out.status.code() {
                Some(0) => Ok(true),
                Some(66) => Ok(false),
                Some(44 | 65 | 69) => Err(HostRuntimeError::UnsafePath(relative.into())),
                status => Err(HostRuntimeError::NonZero {
                    status,
                    stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
                }),
            }
        }
    }
}

pub(crate) fn validate_beneath_relative(relative: &str) -> Result<()> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(HostRuntimeError::UnsafePath(relative.into()));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RemoteBeneathOperation {
    Read,
    Exists,
}

/// Build the fixed remote operation used for repository-scoped file access.
///
/// POSIX shell cannot express `openat(O_NOFOLLOW)`. Instead, each parent is
/// entered and then checked using its descriptor-backed working directory. A
/// read opens the final file exactly once, verifies the resulting descriptor's
/// procfs target is still beneath the pinned physical root, and reads that same
/// descriptor. Hosts without `/proc/<pid>/fd` fail closed with exit 69. The
/// existence operation uses one `ls -d`/`lstat`-style lookup of the final entry
/// and therefore never dereferences it.
pub(crate) fn remote_beneath_script(
    root: &str,
    relative: &str,
    operation: RemoteBeneathOperation,
) -> Result<String> {
    validate_beneath_relative(relative)?;
    let mut components = Path::new(relative).components().peekable();
    let mut parents = Vec::new();
    let leaf = loop {
        let Some(std::path::Component::Normal(component)) = components.next() else {
            return Err(HostRuntimeError::UnsafePath(relative.into()));
        };
        if components.peek().is_none() {
            break component
                .to_str()
                .ok_or_else(|| HostRuntimeError::UnsafePath(relative.into()))?;
        }
        parents.push(
            component
                .to_str()
                .ok_or_else(|| HostRuntimeError::UnsafePath(relative.into()))?,
        );
    };

    let mut inner = format!(
        "root=$(cd -- {} 2>/dev/null && pwd -P) || exit 44\ncd -- \"$root\" || exit 44\n",
        q(root)?
    );
    for parent in parents {
        inner.push_str(&format!(
            "cd -- {} 2>/dev/null || exit 66\n\
             physical=$(pwd -P) || exit 65\n\
             if [ \"$root\" != / ]; then\n\
               case \"$physical\" in \"$root\"|\"$root\"/*) ;; *) exit 65 ;; esac\n\
             fi\n",
            q(parent)?
        ));
    }
    inner.push_str(&format!("leaf={}\n", q(leaf)?));
    match operation {
        RemoteBeneathOperation::Exists => {
            // `-d` asks ls to inspect the directory entry itself. In
            // particular, a final symlink is reported without following it.
            inner.push_str("ls -d -- \"$leaf\" >/dev/null 2>&1 || exit 66\n");
        }
        RemoteBeneathOperation::Read => {
            inner.push_str(
                "exec 3< \"$leaf\" 2>/dev/null || exit 66\n\
                 fd_path=/proc/$$/fd/3\n\
                 resolved=$(readlink -- \"$fd_path\" 2>/dev/null) || exit 69\n\
                 if [ \"$root\" != / ]; then\n\
                   case \"$resolved\" in \"$root\"|\"$root\"/*) ;; *) exit 65 ;; esac\n\
                 fi\n\
                 [ -f \"$fd_path\" ] || exit 65\n\
                 cat <&3\n",
            );
        }
    }
    Ok(format!("sh -c {}", q(&inner)?))
}

#[cfg(unix)]
pub(crate) fn open_local_directory_chain_nofollow(path: &Path) -> Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};

    if !path.is_absolute() {
        return Err(HostRuntimeError::UnsafePath(path.display().to_string()));
    }
    let mut directory = std::fs::File::open(Path::new(std::path::MAIN_SEPARATOR_STR))?;
    for component in path.components() {
        let std::path::Component::Normal(component) = component else {
            if matches!(component, std::path::Component::RootDir) {
                continue;
            }
            return Err(HostRuntimeError::UnsafePath(path.display().to_string()));
        };
        let component = CString::new(component.as_encoded_bytes())
            .map_err(|_| HostRuntimeError::UnsafePath(path.display().to_string()))?;
        // SAFETY: both arguments remain live. O_DIRECTORY and O_NOFOLLOW
        // reject every linked/non-directory ancestor in the pinned chain.
        let raw = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                component.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if raw < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: openat returned a fresh owned descriptor.
        directory = unsafe { std::fs::File::from_raw_fd(raw) };
    }
    Ok(directory)
}

#[cfg(windows)]
pub(crate) fn open_local_directory_chain_nofollow(path: &Path) -> Result<std::fs::File> {
    use cap_primitives::ambient_authority;

    if !path.is_absolute() {
        return Err(HostRuntimeError::UnsafePath(path.display().to_string()));
    }
    let mut anchor = std::path::PathBuf::new();
    let mut children = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) if children.is_empty() => {
                anchor.push(prefix.as_os_str());
            }
            std::path::Component::RootDir if children.is_empty() => {
                anchor.push(Path::new(std::path::MAIN_SEPARATOR_STR));
            }
            std::path::Component::Normal(child) => children.push(child.to_owned()),
            _ => return Err(HostRuntimeError::UnsafePath(path.display().to_string())),
        }
    }
    let mut directory = cap_primitives::fs::open_ambient_dir(&anchor, ambient_authority())?;
    for child in children {
        directory = cap_primitives::fs::open_dir_nofollow(&directory, Path::new(&child))?;
    }
    Ok(directory)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_local_directory_chain_nofollow(path: &Path) -> Result<std::fs::File> {
    Err(HostRuntimeError::UnsafePath(format!(
        "safe local file access is unsupported: {}",
        path.display()
    )))
}

fn open_local_parent_beneath(
    root: &Path,
    relative: &Path,
) -> Result<(std::fs::File, std::ffi::OsString)> {
    let canonical_root = root.canonicalize()?;
    let mut directory = open_local_directory_chain_nofollow(&canonical_root)?;
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let std::path::Component::Normal(name) = component else {
            return Err(HostRuntimeError::UnsafePath(relative.display().to_string()));
        };
        if components.peek().is_none() {
            return Ok((directory, name.to_owned()));
        }
        directory = cap_primitives::fs::open_dir_nofollow(&directory, Path::new(name))?;
    }
    Err(HostRuntimeError::UnsafePath(relative.display().to_string()))
}

fn read_local_file_beneath(root: &Path, relative: &Path) -> Result<Option<Vec<u8>>> {
    use std::io::Read as _;

    let (directory, name) = open_local_parent_beneath(root, relative)?;
    let mut options = cap_primitives::fs::OpenOptions::new();
    options
        .read(true)
        ._cap_fs_ext_follow(cap_primitives::fs::FollowSymlinks::No);
    let mut file = match cap_primitives::fs::open(&directory, Path::new(&name), &options) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() {
        return Err(HostRuntimeError::UnsafePath(relative.display().to_string()));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

fn path_exists_local_beneath(root: &Path, relative: &Path) -> Result<bool> {
    let (directory, name) = match open_local_parent_beneath(root, relative) {
        Ok(value) => value,
        Err(HostRuntimeError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    match cap_primitives::fs::stat(
        &directory,
        Path::new(&name),
        cap_primitives::fs::FollowSymlinks::No,
    ) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

async fn path_test(host: &Host, flag: &str, abs_path: &str) -> Result<bool> {
    match &host.kind {
        HostKind::Local => {
            let metadata = tokio::fs::metadata(abs_path).await;
            Ok(match (flag, metadata) {
                ("-d", Ok(m)) => m.is_dir(),
                ("-f", Ok(m)) => m.is_file(),
                (_, Err(e)) if e.kind() == std::io::ErrorKind::NotFound => false,
                (_, Err(e)) => return Err(e.into()),
                _ => false,
            })
        }
        HostKind::Ssh { .. } => {
            let script = format!("sh -c {}", q(&format!("test {flag} {}", q(abs_path)?))?);
            let out = ssh_output(host, &script, SSH_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            Ok(out.status.success())
        }
    }
}

/// Whether `abs_path` is a directory on `host`.
pub async fn path_is_dir(host: &Host, abs_path: &str) -> Result<bool> {
    path_test(host, "-d", abs_path).await
}

/// Whether `abs_path` is a regular file on `host`.
pub async fn path_is_file(host: &Host, abs_path: &str) -> Result<bool> {
    path_test(host, "-f", abs_path).await
}

/// Remove one exact file on `host`. A missing file is a successful no-op.
pub async fn remove_file(host: &Host, abs_path: &str) -> Result<()> {
    match &host.kind {
        HostKind::Local => match tokio::fs::remove_file(abs_path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        },
        HostKind::Ssh { .. } => {
            let script = format!("sh -c {}", q(&format!("rm -f -- {}", q(abs_path)?))?);
            ssh_checked(host, &script).await
        }
    }
}

/// Playwright's browser install can download ~150 MB; the readiness `SSH_TIMEOUT`
/// is far too short, and even `BOOTSTRAP_TIMEOUT` (180s) is tight on a slow link.
const BROWSER_INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

/// POSIX script that prints the first of `candidates` found on `PATH` (or
/// nothing). `command -v` keeps it portable across the host's login shell
/// (fish/zsh/bash). Pure so the probe shape is unit-testable.
pub(crate) fn which_first_script(candidates: &[&str]) -> String {
    let names = candidates.join(" ");
    format!(
        "for b in {names}; do if command -v \"$b\" >/dev/null 2>&1; then printf %s \"$b\"; exit 0; fi; done"
    )
}

/// Return the first of `candidates` on the host's `PATH`, or `None`. One round
/// trip. `candidates` must be plain binary names (no shell metacharacters) — they
/// are embedded directly in the probe loop.
pub async fn which_first(host: &Host, candidates: &[&str]) -> Result<Option<String>> {
    let script = which_first_script(candidates);
    let out = match &host.kind {
        HostKind::Local => {
            let o = Command::new("sh")
                .arg("-c")
                .arg(&script)
                .output()
                .await
                .map_err(map_ssh_io)?;
            String::from_utf8_lossy(&o.stdout).into_owned()
        }
        HostKind::Ssh { .. } => ssh_stdout(host, &format!("sh -c {}", q(&script)?)).await?,
    };
    let name = out.trim();
    Ok((!name.is_empty()).then(|| name.to_string()))
}

/// Best-effort install of Chromium on the host via Playwright (`npx playwright
/// install chromium`). Login shell (`sh -lc`) so node/npx on a user PATH (nvm,
/// fnm) resolve. Returns the combined output tail; errors on a non-zero exit so
/// the caller can surface a stated reason. Needs node/npx on the host.
pub async fn install_host_chromium(host: &Host) -> Result<String> {
    let cmd = "npx --yes playwright install chromium";
    match &host.kind {
        HostKind::Local => {
            let o = Command::new("sh")
                .arg("-lc")
                .arg(cmd)
                .output()
                .await
                .map_err(map_ssh_io)?;
            let tail = String::from_utf8_lossy(&o.stdout).into_owned();
            if o.status.success() {
                Ok(tail)
            } else {
                Err(HostRuntimeError::NonZero {
                    status: o.status.code(),
                    stderr: String::from_utf8_lossy(&o.stderr).trim().to_string(),
                })
            }
        }
        HostKind::Ssh { .. } => {
            let out = ssh_output(
                host,
                &format!("sh -lc {}", q(cmd)?),
                BROWSER_INSTALL_TIMEOUT,
            )
            .await
            .map_err(map_ssh_io)?;
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            } else {
                Err(HostRuntimeError::NonZero {
                    status: out.status.code(),
                    stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                })
            }
        }
    }
}

/// Build the inner POSIX script that writes `content` to `$HOME/<rel_path>` on
/// the host, creating the parent dir and keeping the file owner-only. `$HOME` is
/// left for the remote `sh` to expand (the login shell may be fish/zsh, so this
/// inner script is base64-piped to `sh` by [`write_home_relative_file`], never
/// run directly). `content` rides as base64 so any payload writes verbatim.
/// `rel_path` must be a caller-controlled safe slug path (embedded in quotes).
pub(crate) fn marker_inner_script(rel_path: &str, content: &str) -> Result<String> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(content);
    let mkdir = match rel_path.rsplit_once('/') {
        Some((parent, _)) => format!("mkdir -p \"$HOME/{parent}\"; "),
        None => String::new(),
    };
    Ok(format!(
        "umask 077; {mkdir}printf %s {b64} | base64 -d > \"$HOME/{rel_path}\"",
        b64 = q(&b64)?,
    ))
}

/// Write `content` to `$HOME/<rel_path>` on `host` (the local home, or the SSH
/// host's home), owner-only, creating parents. Unlike [`write_remote_file`] this
/// resolves the host's `$HOME` so callers can drop a marker without knowing the
/// absolute home path. Used for the host-browser per-worktree port marker.
pub async fn write_home_relative_file(host: &Host, rel_path: &str, content: &str) -> Result<()> {
    match &host.kind {
        HostKind::Local => {
            let home = std::env::var("HOME")
                .map_err(|_| HostRuntimeError::Bootstrap("no HOME for local marker".into()))?;
            // The local branch of `write_remote_file` mkdir-p's + chmods 600.
            write_remote_file(host, &format!("{home}/{rel_path}"), content).await
        }
        HostKind::Ssh { .. } => {
            let inner = marker_inner_script(rel_path, content)?;
            let inner_b64 = base64::engine::general_purpose::STANDARD.encode(&inner);
            // Only base64 chars in the outer command, so fish/zsh/bash run it the
            // same; the decoded inner runs under `sh`, where `$HOME` expands.
            let remote = format!("printf %s {} | base64 -d | sh", q(&inner_b64)?);
            ssh_checked(host, &remote).await
        }
    }
}
