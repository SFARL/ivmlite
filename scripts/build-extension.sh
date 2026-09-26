#!/usr/bin/env bash
# Build the ivmlite SQLite extension (a cdylib outside the Cargo workspace;
# see crates/ivmlite-sqlite/Cargo.toml for why). The extension tests in
# ivmlite-test load crates/ivmlite-sqlite/target/debug/libivmlite_sqlite.*.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml
