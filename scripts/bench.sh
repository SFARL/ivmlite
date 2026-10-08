#!/usr/bin/env bash
# Build the extension and the bench in release mode, then run the bench.
# Arguments are passed to ivmlite-bench (e.g. `matrix`, `confirm`, ...).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml
cargo run --release -p ivmlite-bench --locked -- "$@"
