#!/usr/bin/env bash
# Build the extension, then run every workspace test. The mutation-gate
# protocol (docs/mutation-gates.md) uses this rather than a bare `cargo test`,
# so a mutation in core, the SQL front end or the extension reaches the
# extension tests.
set -euo pipefail
cd "$(dirname "$0")/.."
scripts/build-extension.sh
cargo test --manifest-path crates/ivmlite-sqlite/Cargo.toml --locked
cargo test --workspace --locked --no-fail-fast "$@"
