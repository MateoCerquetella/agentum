# Commands

## Setup

- Desktop UI dependencies: `bun install --frozen-lockfile` from
  `crates/agentum-desktop/ui`.
- CI pins Rust 1.94.1 and Bun 1.3.14. Platform desktop builds also require the
  native Tauri/WebKit and Sherpa/ONNX dependencies documented in
  `.github/workflows/ci.yml`.

## Run, test, and build

- Backend development server: `just dev`.
- Rust production build: `just build`.
- Rust format/lint: `cargo fmt --all -- --check` and
  `cargo clippy --all-targets --all-features -- -D warnings`.
- Rust tests: `cargo test --all` (CI uses `--locked`).
- UI development: `bun run dev` from `crates/agentum-desktop/ui`.
- UI verification: `bunx tsc --noEmit`, `bunx vitest run`, and
  `bun run build:check`.

## Verification evidence

- `scripts/check-sdd-boundary.sh --boundary-only` enforces repository and SDD
  source boundaries.
- CI policy runs the Python artifact/migration/demo/golden/release/restricted
  contract suite listed in `.github/workflows/ci.yml`.
- Release tags run locked cross-platform Rust/UI builds, provider conformance,
  restricted-content scanning, signing, and version/tag authority checks.
