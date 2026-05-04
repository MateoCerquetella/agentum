#!/usr/bin/env python3
"""
amux server watchdog — monitors health, auto-restarts, and spawns Claude to
diagnose + fix persistent failures.

Works on both macOS (launchd) and Linux/Docker (systemctl/kill fallback).
"""

import json, os, signal, subprocess, sys, time, urllib.request, ssl
from pathlib import Path
from datetime import datetime

IS_MACOS = sys.platform == "darwin"
IS_DOCKER = Path("/.dockerenv").exists()

AMUX_PORT = int(os.environ.get("AMUX_PORT", 8822))
# Cloud runs --no-tls, so use http there
_SCHEME = "https" if IS_MACOS else "http"
HEALTH_URL = os.environ.get("WATCHDOG_HEALTH_URL", f"{_SCHEME}://localhost:{AMUX_PORT}/health")
LAUNCHD_LABEL = "com.amux.server"
LOG_FILE = Path.home() / ".amux" / "logs" / "watchdog.log"
SERVER_LOG = Path.home() / ".amux" / "logs" / "server.log"
AMUX_DIR = Path(os.environ.get("WATCHDOG_AMUX_DIR", str(Path.home() / "Dev" / "amux")))
SERVER_SCRIPT = AMUX_DIR / "amux-server.py"

CHECK_INTERVAL = int(os.environ.get("WATCHDOG_INTERVAL", 30))
UNHEALTHY_THRESHOLD = 3      # consecutive failures before action
CPU_THRESHOLD = 80.0          # percent — sustained high CPU triggers alert
MEMORY_THRESHOLD = 2048       # MB — RSS above this triggers alert
CLAUDE_COOLDOWN = 900         # seconds — don't re-invoke Claude within this window

_ssl_ctx = ssl.create_default_context()
_ssl_ctx.check_hostname = False
_ssl_ctx.verify_mode = ssl.CERT_NONE

_consecutive_failures = 0
_consecutive_cpu_high = 0
_last_claude_invocation = 0


def log(msg: str):
    line = f"{datetime.now().strftime('%Y-%m-%d %H:%M:%S')} [watchdog] {msg}\n"
    sys.stderr.write(line)
    try:
        LOG_FILE.parent.mkdir(parents=True, exist_ok=True)
        with open(LOG_FILE, "a") as f:
            f.write(line)
    except Exception:
        pass


def health_check() -> dict | None:
    """Hit /health and return parsed JSON, or None on failure."""
    try:
        req = urllib.request.Request(HEALTH_URL)
        resp = urllib.request.urlopen(req, timeout=10, context=_ssl_ctx)
        return json.loads(resp.read())
    except Exception as e:
        log(f"health check failed: {e}")
        return None


def _find_server_pid() -> int | None:
    """Find the amux-server.py process PID."""
    try:
        out = subprocess.check_output(
            ["pgrep", "-f", "amux-server.py"], timeout=5, stderr=subprocess.DEVNULL
        ).decode().strip()
        pids = [int(p) for p in out.splitlines() if p.strip()]
        # Filter out our own PID
        pids = [p for p in pids if p != os.getpid()]
        return pids[0] if pids else None
    except Exception:
        return None


def restart_server():
    """Restart via launchctl (macOS) or kill + let supervisor respawn (Linux/Docker)."""
    if IS_MACOS:
        log("restarting amux server via launchctl")
        try:
            subprocess.run(
                ["launchctl", "kickstart", "-k", f"gui/{os.getuid()}/{LAUNCHD_LABEL}"],
                capture_output=True, timeout=15,
            )
        except Exception as e:
            log(f"launchctl restart failed: {e}")
            return False
    else:
        # Linux / Docker — kill the process; docker restart policy or systemd will respawn
        pid = _find_server_pid()
        if pid:
            log(f"killing server pid {pid} (supervisor will respawn)")
            try:
                os.kill(pid, signal.SIGTERM)
                time.sleep(2)
                # Force kill if still alive
                try:
                    os.kill(pid, 0)
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            except Exception as e:
                log(f"kill failed: {e}")
                return False
        else:
            log("server pid not found — cannot restart")
            return False

    # Wait for restart
    for i in range(6):
        time.sleep(5)
        if health_check():
            log("server restarted successfully")
            return True
    log("server still unhealthy after restart")
    return False


def collect_diagnostics() -> str:
    """Gather diagnostic context for Claude."""
    parts = ["# amux watchdog diagnostics\n"]
    parts.append(f"## environment\nplatform={sys.platform} docker={IS_DOCKER} port={AMUX_PORT}\n")

    # Last 100 lines of server log
    try:
        lines = SERVER_LOG.read_text().splitlines()[-100:]
        parts.append("## server.log (last 100 lines)\n```\n" + "\n".join(lines) + "\n```\n")
    except Exception as e:
        parts.append(f"## server.log: error reading — {e}\n")

    # Process info
    try:
        ps = subprocess.check_output(
            ["ps", "-Ao", "pid,%cpu,%mem,etime,command"],
            timeout=5,
        ).decode()
        amux_lines = [l for l in ps.splitlines() if "amux-server" in l]
        parts.append("## amux processes\n```\n" + "\n".join(amux_lines) + "\n```\n")
    except Exception:
        pass

    # Recent errors from log
    try:
        lines = SERVER_LOG.read_text().splitlines()
        errors = [l for l in lines[-500:] if "ERROR" in l or "Traceback" in l or "Exception" in l][-20:]
        if errors:
            parts.append("## recent errors\n```\n" + "\n".join(errors) + "\n```\n")
    except Exception:
        pass

    # System resources — platform-aware
    if IS_MACOS:
        try:
            vm = subprocess.check_output(["vm_stat"], timeout=5).decode()
            parts.append("## vm_stat\n```\n" + vm + "\n```\n")
        except Exception:
            pass
    else:
        try:
            meminfo = Path("/proc/meminfo").read_text()
            parts.append("## /proc/meminfo (summary)\n```\n" + "\n".join(meminfo.splitlines()[:10]) + "\n```\n")
        except Exception:
            pass
        try:
            loadavg = Path("/proc/loadavg").read_text().strip()
            parts.append(f"## load average\n`{loadavg}`\n")
        except Exception:
            pass

    return "\n".join(parts)


def invoke_claude(issue: str, diagnostics: str):
    """Spawn Claude Code to diagnose, fix, and push."""
    global _last_claude_invocation
    now = time.time()
    if now - _last_claude_invocation < CLAUDE_COOLDOWN:
        log(f"skipping Claude invocation — cooldown ({int(CLAUDE_COOLDOWN - (now - _last_claude_invocation))}s remaining)")
        return
    _last_claude_invocation = now

    prompt = f"""The amux server watchdog detected an issue:

**Issue:** {issue}

{diagnostics}

Please:
1. Analyze the diagnostics above and identify the root cause
2. If it's a code bug in amux-server.py, fix it
3. Verify the fix with `python3 -c "import ast; ast.parse(open('amux-server.py').read())"`
4. If you made changes, commit and push:
   - `git add amux-server.py`
   - `git commit -m "fix: <concise description>"`
   - `git push origin main`
5. The server auto-restarts on file save, so changes go live immediately

If the issue is environmental (not a code bug), just log your findings — don't make unnecessary code changes.
"""

    log(f"invoking Claude Code to diagnose: {issue}")
    try:
        result = subprocess.run(
            ["claude", "--print", "--dangerously-skip-permissions", "-p", prompt],
            capture_output=True, text=True, timeout=300, cwd=str(AMUX_DIR),
        )
        log(f"Claude exited {result.returncode}")
        if result.stdout:
            for line in result.stdout.strip().splitlines()[-10:]:
                log(f"  claude: {line}")
        if result.returncode != 0 and result.stderr:
            for line in result.stderr.strip().splitlines()[-5:]:
                log(f"  claude stderr: {line}")
    except subprocess.TimeoutExpired:
        log("Claude invocation timed out (5m)")
    except FileNotFoundError:
        log("claude CLI not found in PATH")
    except Exception as e:
        log(f"Claude invocation error: {e}")


def run():
    global _consecutive_failures, _consecutive_cpu_high
    log(f"watchdog starting — platform={'macos' if IS_MACOS else 'linux'} docker={IS_DOCKER} url={HEALTH_URL} interval={CHECK_INTERVAL}s")

    while True:
        data = health_check()

        if data is None:
            _consecutive_failures += 1
            _consecutive_cpu_high = 0
            log(f"unhealthy ({_consecutive_failures}/{UNHEALTHY_THRESHOLD})")

            if _consecutive_failures >= UNHEALTHY_THRESHOLD:
                log("threshold reached — attempting restart")
                if restart_server():
                    _consecutive_failures = 0
                else:
                    diag = collect_diagnostics()
                    invoke_claude("server unresponsive after restart attempt", diag)
                    _consecutive_failures = 0
        else:
            _consecutive_failures = 0
            cpu = data.get("cpu_percent", 0)
            mem = data.get("memory_mb", 0)

            if cpu > CPU_THRESHOLD:
                _consecutive_cpu_high += 1
                log(f"high CPU: {cpu}% ({_consecutive_cpu_high}/3)")
                if _consecutive_cpu_high >= 3:
                    diag = collect_diagnostics()
                    invoke_claude(f"sustained high CPU ({cpu}%) for {_consecutive_cpu_high * CHECK_INTERVAL}s", diag)
                    _consecutive_cpu_high = 0
            else:
                _consecutive_cpu_high = 0

            if mem > MEMORY_THRESHOLD:
                log(f"high memory: {mem}MB (threshold {MEMORY_THRESHOLD}MB)")
                diag = collect_diagnostics()
                invoke_claude(f"high memory usage: {mem}MB", diag)

        time.sleep(CHECK_INTERVAL)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda s, f: (log("received SIGTERM, exiting"), sys.exit(0)))
    try:
        run()
    except KeyboardInterrupt:
        log("stopped")
