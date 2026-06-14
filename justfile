default:
    @just --list

build:
    cargo build --workspace

test:
    cargo test --workspace

check:
    cargo clippy --workspace -- -D warnings
    cargo fmt --check

ci: check test build
