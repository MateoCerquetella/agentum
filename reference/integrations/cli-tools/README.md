# CLI Tools in amux Sessions

This directory documents how to install and use CLI tools inside amux sessions.

CLI tools complement MCP integrations — use MCP for structured read/write access to external services, and CLI tools for one-off commands, scripting, or when no MCP server exists.

## How it works

- amux sessions run inside tmux panes — they inherit your full shell environment
- Tools installed on your Mac are available immediately (no container, no VM)
- Credentials stored in `~/.amux/server.env` are injected into every session at startup

## Available CLI tools

| Tool | Directory | Use case |
|------|-----------|----------|
| [Google Cloud CLI (`gcloud`)](#google-cloud-cli) | [google/](./google/) | GCS, BigQuery, GKE, etc. |

---

## Google Cloud CLI

See [`google/`](./google/) for full setup instructions.

**TL;DR:**
```bash
# Install
brew install --cask google-cloud-sdk

# Authenticate
gcloud auth login
gcloud auth application-default login

# Use in any session
gcloud storage ls gs://my-bucket
```

---

## Adding a new CLI tool

1. Create `integrations/cli-tools/<tool-name>/README.md`
2. Document: install command, auth setup, required env vars, example usage
3. If the tool needs API keys, add them to `~/.amux/server.env`
4. Update the table above
