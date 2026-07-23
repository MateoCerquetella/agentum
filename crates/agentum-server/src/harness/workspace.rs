//! Host-aware workspace I/O for harness execution.
//!
//! A remote worktree path is never touched through `std::fs`/`tokio::fs` on
//! the daemon. All sequential-driver contract files and gate commands flow
//! through this value, which is pinned to the [`HarnessScope`] resolved at the
//! HTTP boundary.

use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use agentum_core::{HarnessScope, Host, HostKind};

use crate::AppState;

/// Remote files do not expose a portable append syscall through the current
/// SSH transport. Serialize the read+atomic-replace fallback so concurrent
/// role/phase decisions cannot lose each other's entries.
static REMOTE_APPEND_GATE: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

#[derive(Clone)]
pub(crate) struct HarnessWorkspace {
    scope: HarnessScope,
    host: Option<Host>,
}

impl std::fmt::Debug for HarnessWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HarnessWorkspace")
            .field("scope", &self.scope)
            .field("remote", &self.is_remote())
            .finish_non_exhaustive()
    }
}

impl HarnessWorkspace {
    pub(crate) fn local(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            scope: HarnessScope::local_path(path.to_string_lossy().into_owned()),
            host: None,
        }
    }

    pub(crate) fn scoped(scope: HarnessScope, host: Host) -> Self {
        Self {
            scope,
            host: Some(host),
        }
    }

    pub(crate) fn restored(scope: HarnessScope, host: Option<Host>) -> Self {
        Self { scope, host }
    }

    pub(crate) fn scope(&self) -> &HarnessScope {
        &self.scope
    }

    pub(crate) fn root(&self) -> PathBuf {
        PathBuf::from(&self.scope.path)
    }

    pub(crate) fn is_remote(&self) -> bool {
        self.scope
            .host_id
            .is_some_and(|host_id| host_id != agentum_core::LOCAL_HOST_ID)
            || self
                .host
                .as_ref()
                .is_some_and(|host| matches!(host.kind, HostKind::Ssh { .. }))
    }

    pub(crate) fn host(&self) -> Option<&Host> {
        self.host.as_ref()
    }

    /// Return the host snapshot captured when the scope was resolved, after
    /// checking that the scope and snapshot still describe one machine.
    ///
    /// A restored remote scope without a host must never fall through to local
    /// filesystem/process APIs. Likewise, a mismatched `host_id` is corrupt
    /// scope, not permission to reinterpret the path on another machine.
    fn bound_host(&self) -> anyhow::Result<Option<&Host>> {
        match (&self.host, self.scope.host_id) {
            (Some(host), Some(host_id)) if host.id != host_id => anyhow::bail!(
                "harness host binding mismatch: scope is {host_id}, snapshot is {}",
                host.id
            ),
            (None, Some(host_id)) if host_id != agentum_core::LOCAL_HOST_ID => {
                anyhow::bail!("remote harness host is unavailable: {host_id}")
            }
            (Some(host), None) if !matches!(host.kind, HostKind::Local) => {
                anyhow::bail!("remote harness scope is missing host identity")
            }
            (host, _) => Ok(host.as_ref()),
        }
    }

    /// Resolve the process/session host without allowing an in-place host edit
    /// to split one run across two machines. Workspace I/O remains pinned to
    /// the captured snapshot; before spawning we verify the catalog still has
    /// the same connection binding and then return that same snapshot.
    pub(crate) async fn execution_host(&self, state: &AppState) -> anyhow::Result<Host> {
        let expected = match self.bound_host()? {
            Some(host) => host.clone(),
            None => state
                .store
                .get_host(agentum_core::LOCAL_HOST_ID)
                .await?
                .ok_or_else(|| anyhow::anyhow!("local host is missing"))?,
        };
        let current = state
            .store
            .get_host(expected.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("harness host is missing: {}", expected.id))?;
        if !same_host_binding(&expected, &current) {
            anyhow::bail!(
                "harness host connection changed while the run was active; stop and re-register the run"
            );
        }
        Ok(expected)
    }

    pub(crate) fn join(&self, relative: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
        let relative = relative.as_ref();
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            anyhow::bail!(
                "harness workspace paths must be relative and may not traverse: {}",
                relative.display()
            );
        }
        Ok(self.root().join(relative))
    }

    fn validate_path(&self, path: &Path) -> anyhow::Result<()> {
        let root = self.root();
        if root.as_os_str().is_empty()
            || root
                .components()
                .any(|part| matches!(part, Component::ParentDir))
            || !path.starts_with(&root)
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            anyhow::bail!("path escapes harness workspace: {}", path.display());
        }
        Ok(())
    }

    /// Resolve both root and target on the SSH host before a mutation. This
    /// catches an in-worktree symlink whose parent points outside the worktree;
    /// lexical prefix checks alone cannot.
    async fn validate_remote_mutation(&self, path: &Path) -> anyhow::Result<()> {
        self.validate_path(path)?;
        if !self.is_remote() {
            return Ok(());
        }
        // POSIX `cd -P`/`pwd -P` works on Linux and BSD/macOS hosts. Walk to
        // the nearest existing directory first so new scaffold directories do
        // not require GNU `realpath -m`.
        let script = r#"
[ -L "$HARNESS_TARGET" ] && exit 42
resolve_path() {
  p=$1
  suffix=
  while [ ! -d "$p" ]; do
    base=$(basename "$p") || exit 1
    suffix="/$base$suffix"
    next=$(dirname "$p") || exit 1
    [ "$next" != "$p" ] || exit 1
    p=$next
  done
  physical=$(cd -P "$p" 2>/dev/null && pwd -P) || exit 1
  printf '%s%s\n' "$physical" "$suffix"
}

resolve_path "$HARNESS_ROOT"
resolve_path "$HARNESS_TARGET"
"#;
        let resolved = self
            .run(
                "sh",
                &["-c".into(), script.into()],
                &[
                    ("HARNESS_ROOT".into(), self.scope.path.clone()),
                    ("HARNESS_TARGET".into(), path.to_string_lossy().into_owned()),
                ],
            )
            .await?;
        if !resolved.success {
            anyhow::bail!("could not resolve remote harness path containment");
        }
        let stdout = String::from_utf8_lossy(&resolved.stdout);
        let mut lines = stdout.lines();
        let root = lines.next().unwrap_or_default().trim().to_string();
        let target = lines.next().unwrap_or_default().trim().to_string();
        if root.is_empty() || target.is_empty() {
            anyhow::bail!("remote harness path containment returned no path");
        }
        let prefix = format!("{}/", root.trim_end_matches('/'));
        if target != root && !target.starts_with(&prefix) {
            anyhow::bail!("remote path escapes harness workspace: {target}");
        }
        Ok(())
    }

    pub(crate) async fn exists(&self, path: &Path) -> anyhow::Result<bool> {
        self.validate_path(path)?;
        match self.bound_host()? {
            Some(host) => {
                Ok(crate::host_runtime::path_exists(host, &path.to_string_lossy()).await?)
            }
            None => Ok(tokio::fs::try_exists(path).await.unwrap_or(false)),
        }
    }

    pub(crate) async fn is_dir(&self, path: &Path) -> anyhow::Result<bool> {
        self.validate_path(path)?;
        match self.bound_host()? {
            Some(host) => {
                Ok(crate::host_runtime::path_is_dir(host, &path.to_string_lossy()).await?)
            }
            None => Ok(tokio::fs::metadata(path)
                .await
                .map(|m| m.is_dir())
                .unwrap_or(false)),
        }
    }

    pub(crate) async fn is_file(&self, path: &Path) -> anyhow::Result<bool> {
        self.validate_path(path)?;
        match self.bound_host()? {
            Some(host) => {
                Ok(crate::host_runtime::path_is_file(host, &path.to_string_lossy()).await?)
            }
            None => Ok(tokio::fs::metadata(path)
                .await
                .map(|m| m.is_file())
                .unwrap_or(false)),
        }
    }

    pub(crate) async fn try_read(&self, path: &Path) -> anyhow::Result<Option<String>> {
        self.validate_path(path)?;
        match self.bound_host()? {
            Some(host) => {
                Ok(crate::host_runtime::read_remote_file(host, &path.to_string_lossy()).await?)
            }
            None => match tokio::fs::read_to_string(path).await {
                Ok(body) => Ok(Some(body)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
        }
    }

    pub(crate) async fn read(&self, path: &Path) -> anyhow::Result<String> {
        self.try_read(path)
            .await?
            .ok_or_else(|| anyhow::anyhow!("file not found: {}", path.display()))
    }

    pub(crate) async fn mkdir_all(&self, path: &Path) -> anyhow::Result<()> {
        self.validate_remote_mutation(path).await?;
        match self.bound_host()? {
            Some(host) => Ok(crate::host_runtime::mkdir_p(host, &path.to_string_lossy()).await?),
            None => Ok(tokio::fs::create_dir_all(path).await?),
        }
    }

    pub(crate) async fn write(&self, path: &Path, content: &str) -> anyhow::Result<()> {
        self.validate_remote_mutation(path).await?;
        if let Some(parent) = path.parent() {
            self.mkdir_all(parent).await?;
        }
        match self.bound_host()? {
            Some(host) => {
                Ok(
                    crate::host_runtime::write_remote_file(host, &path.to_string_lossy(), content)
                        .await?,
                )
            }
            None => Ok(tokio::fs::write(path, content).await?),
        }
    }

    pub(crate) async fn remove_file(&self, path: &Path) -> anyhow::Result<()> {
        self.validate_remote_mutation(path).await?;
        match self.bound_host()? {
            Some(host) => {
                Ok(crate::host_runtime::remove_file(host, &path.to_string_lossy()).await?)
            }
            None => match tokio::fs::remove_file(path).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
        }
    }

    pub(crate) async fn append_line(&self, path: &Path, line: &str) -> anyhow::Result<()> {
        self.validate_remote_mutation(path).await?;
        if !self.is_remote() {
            use tokio::io::AsyncWriteExt;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await?;
            file.write_all(line.as_bytes()).await?;
            file.flush().await?;
            return Ok(());
        }

        let _guard = REMOTE_APPEND_GATE.lock().await;
        let mut body = self.try_read(path).await?.unwrap_or_default();
        body.push_str(line);
        self.write(path, &body).await
    }

    pub(crate) async fn run(
        &self,
        program: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> anyhow::Result<crate::host_runtime::HostCommandOutput> {
        if let Some(host) = self.bound_host()? {
            return Ok(crate::host_runtime::command_in_dir(
                host,
                &self.scope.path,
                program,
                args,
                env,
            )
            .await?);
        }
        let output = tokio::process::Command::new(program)
            .args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(&self.scope.path)
            .output()
            .await?;
        Ok(crate::host_runtime::HostCommandOutput {
            success: output.status.success(),
            code: output.status.code(),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }

    /// Fail closed before an SSH harness mutates its backlog or reports a run
    /// as started. The selected agent, shell, Agentum MCP switch/endpoint, and
    /// reverse tunnel are all required. Issue-driven routes additionally
    /// require an authenticated remote `gh` CLI.
    pub(crate) async fn strict_remote_preflight(
        &self,
        state: &AppState,
        agent_tool: Option<&str>,
        qa_agent_tool: Option<&str>,
        require_gh: bool,
    ) -> anyhow::Result<()> {
        if !self.is_remote() {
            return Ok(());
        }
        let host = self.execution_host(state).await?;
        if !self.is_dir(&self.root()).await? {
            anyhow::bail!("remote worktree does not exist: {}", self.scope.path);
        }
        let readiness = crate::host_runtime::readiness(&host).await;
        if !readiness.ok {
            anyhow::bail!("remote host is not ready: {}", readiness.message);
        }
        for tool in required_agent_tools(agent_tool, qa_agent_tool) {
            let installed = readiness
                .agents
                .iter()
                .find(|agent| agent.id == tool)
                .is_some_and(|agent| agent.installed);
            if !installed {
                anyhow::bail!("agent `{tool}` is not installed on the remote host");
            }
            if !crate::mcp_provision::tool_supports_mcp(tool)
                && crate::mcp_provision::agent_mcp_file(tool).is_none()
            {
                anyhow::bail!("agent `{tool}` cannot be provisioned with the Agentum MCP");
            }
        }
        let shell = self
            .run(
                "sh",
                &["-c".into(), "command -v bash >/dev/null 2>&1".into()],
                &[],
            )
            .await?;
        if !shell.success {
            anyhow::bail!("bash is required by remote harness gate scripts");
        }
        if require_gh {
            let gh = self
                .run("gh", &["auth".into(), "status".into()], &[])
                .await?;
            if !gh.success {
                anyhow::bail!("remote `gh` is missing or unauthenticated: {}", gh.stderr);
            }
        }
        if !state
            .store
            .setting_get_bool(crate::routes::mcp::MCP_ENABLED_SETTING, true)
            .await?
        {
            anyhow::bail!("Agentum MCP is disabled; remote harness execution requires it");
        }
        let local_port = crate::mcp_provision::local_mcp_port(state).ok_or_else(|| {
            anyhow::anyhow!("remote harness requires an embedded Agentum API endpoint")
        })?;
        crate::host_runtime::ensure_reverse_tunnel(&host, local_port)
            .await
            .map_err(|e| anyhow::anyhow!("remote Agentum MCP tunnel preflight failed: {e}"))?;
        Ok(())
    }
}

fn same_host_binding(expected: &Host, current: &Host) -> bool {
    expected.id == current.id && expected.kind == current.kind
}

fn required_agent_tools<'a>(
    primary: Option<&'a str>,
    explicit_qa: Option<&'a str>,
) -> Vec<&'a str> {
    let mut tools = Vec::with_capacity(2);
    if let Some(tool) = primary.filter(|tool| !tool.trim().is_empty()) {
        tools.push(tool);
    }
    if let Some(tool) = explicit_qa.filter(|tool| !tool.trim().is_empty())
        && !tools.contains(&tool)
    {
        tools.push(tool);
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_core::{LOCAL_HOST_ID, SshAuth};
    use time::OffsetDateTime;

    #[test]
    fn relative_join_rejects_absolute_and_traversal_paths() {
        let workspace = HarnessWorkspace::local("/tmp/project");
        assert_eq!(
            workspace.join(".agentum-harness/specs/s1").unwrap(),
            PathBuf::from("/tmp/project/.agentum-harness/specs/s1")
        );
        assert!(workspace.join("/etc/passwd").is_err());
        assert!(workspace.join("../outside").is_err());
        assert!(workspace.join("specs/../../outside").is_err());
    }

    #[tokio::test]
    async fn local_append_preserves_all_decision_lines() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = HarnessWorkspace::local(dir.path());
        let log = workspace.join("decisions.md").unwrap();
        workspace.append_line(&log, "- first\n").await.unwrap();
        workspace.append_line(&log, "- second\n").await.unwrap();
        assert_eq!(workspace.read(&log).await.unwrap(), "- first\n- second\n");
    }

    fn host(id: uuid::Uuid, hostname: &str) -> Host {
        Host {
            id,
            name: "remote".into(),
            kind: HostKind::Ssh {
                user: "dev".into(),
                hostname: hostname.into(),
                port: 22,
                auth: SshAuth::Agent,
            },
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    #[test]
    fn host_binding_ignores_display_metadata_but_rejects_connection_retargeting() {
        let id = uuid::Uuid::new_v4();
        let expected = host(id, "alpha.example");
        let mut renamed = expected.clone();
        renamed.name = "renamed".into();
        renamed.last_seen_at = Some(OffsetDateTime::UNIX_EPOCH);
        assert!(same_host_binding(&expected, &renamed));
        assert!(!same_host_binding(&expected, &host(id, "beta.example")));
    }

    #[test]
    fn restored_remote_scope_without_host_never_becomes_local() {
        let remote_id = uuid::Uuid::new_v4();
        let workspace = HarnessWorkspace::restored(
            HarnessScope {
                worktree_id: Some("repo::/srv/repo/wt".into()),
                repo_id: Some("repo".into()),
                host_id: Some(remote_id),
                path: "/srv/repo/wt".into(),
            },
            None,
        );
        assert!(workspace.is_remote());
        assert!(workspace.bound_host().is_err());

        let local = HarnessWorkspace::restored(
            HarnessScope {
                host_id: Some(LOCAL_HOST_ID),
                path: "/tmp/local".into(),
                ..HarnessScope::default()
            },
            None,
        );
        assert!(local.bound_host().unwrap().is_none());
    }

    #[test]
    fn preflight_checks_primary_and_distinct_explicit_qa_agents() {
        assert_eq!(
            required_agent_tools(Some("claude"), Some("codex")),
            vec!["claude", "codex"]
        );
        assert_eq!(
            required_agent_tools(Some("claude"), Some("claude")),
            vec!["claude"]
        );
        assert!(required_agent_tools(None, None).is_empty());
    }
}
