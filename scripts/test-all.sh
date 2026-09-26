#!/usr/bin/env bash
# Build the extension, then run every workspace test. The mutation-gate
# protocol (docs/mutation-gates.md) uses this rather than a bare `cargo test`,
# so a mutation in core, the SQL front end or the extension reaches the
# extension tests.
#
# The build must succeed before anything else can be tested, so it still
# stops the script immediately on failure. The two test invocations that
# follow do not stop each other: under a plain `set -e`, the extension's own
# unit tests failing would skip the workspace invocation entirely and
# truncate the gate count exactly when a mutation reaches the extension —
# the one case this script exists to catch (docs/mutation-gates.md's "How to
# use it"). So both cargo invocations always run, and the script's own exit
# status is non-zero if either did fail.
set -uo pipefail
cd "$(dirname "$0")/.."

scripts/build-extension.sh || exit 1

status=0
cargo test --manifest-path crates/ivmlite-sqlite/Cargo.toml --locked || status=1
cargo test --workspace --locked --no-fail-fast "$@" || status=1
exit "$status"
