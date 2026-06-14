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

package-cli:
    cargo build --release -p rcd-cli
    @echo "Binary at target/release/rcd"

package-mod:
    cargo build --release -p rcd-ffi
    @echo "Native library at target/release/"
    @ls -la target/release/librcd_ffi.* 2>/dev/null || ls -la target/release/rcd_ffi.* 2>/dev/null || echo "Check target/release/ for the library"
