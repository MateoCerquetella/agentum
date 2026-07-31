# Project Overview

## Purpose

- Agentum is a self-hosted control plane for AI coding agents. It keeps agent
  processes alive in tmux, exposes one authenticated HTTP/WebSocket backend,
  and provides a native Tauri desktop client.
- Agentum SDD owns a durable specification-to-Ready lifecycle with isolated
  attempts, approvals, evidence, independent review, and explicit delivery.

## Boundaries

- This repository contains the desktop app and shared Rust backend crates. The
  terminal UI/CLI lives in the separate `agentum-tui` repository.
- The daemon is API-only; the React/Vite UI is embedded by the Tauri desktop
  shell rather than served as a web dashboard.
- Ready is not delivery. Commits, pushes, merges, tracker writes, and releases
  occur only through an explicit delivery/promotion workflow.
- Provider processes do not own SDD state and must not write ambient agent
  configuration into customer repositories.

## Evidence

- `README.md` documents product purpose, workflow, architecture, and layout.
- `CLAUDE.md` is the repository contribution and architecture guide.
- `Cargo.toml` lists the Rust workspace; `crates/agentum-desktop/ui/package.json`
  defines the desktop frontend toolchain.
- `docs/AGENTUM_SDD.md` defines the authoritative SDD behavior and boundaries.
