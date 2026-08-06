# Commands

## Setup

- Install: `cargo install --path crates/agentum-tui --locked --force`
- Run from source: `cargo run -p agentum-tui`

## Run, test, and build

- Format check: `cargo fmt --all -- --check`
- Compile check: `cargo check --workspace --all-targets`
- Workspace tests: `cargo test --workspace`
- Focused tests: `cargo test -p <crate-name>`

## Verification evidence

- Unit tests live beside Rust modules under `#[cfg(test)]`.
- Cross-component tests live under `crates/agentum-server/tests/`.
- Real-host SSH evidence must be credential-redacted and recorded separately
  from deterministic tests.
