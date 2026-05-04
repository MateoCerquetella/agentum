set shell := ["bash", "-uc"]

default:
    @just --list

# Run dev server with cargo-watch + svelte dev (phase 3+)
dev:
    cargo run -- serve --port 8822

# Production build: build web (phase 3+), then cargo release
build:
    cargo build --release

# Release artifacts via cargo-dist (phase 10)
release:
    cargo dist build

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
