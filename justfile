set shell := ["bash", "-uc"]

default:
    @just --list

# Run the daemon locally
dev:
    cargo run -- serve --port 8822

# Production build
build:
    cargo build --release

# Lint + format check
check:
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings

# Run all tests
test:
    cargo test --all

# Apply formatter
fmt:
    cargo fmt --all
