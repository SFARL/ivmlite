#!/usr/bin/env bash
# Task 2 (spec §6): build the extension at one or more commits, each in its
# own git worktree with its own CARGO_TARGET_DIR (never the main checkout's
# target/ — that is reserved for the release bench itself), and run
# `ivmlite-bench ablation` against each build. Rows from every build
# accumulate into one output CSV, with the header written only once.
#
# Usage: scripts/bench-ablation.sh <label>=<rev> [<label>=<rev> ...]
#
# Example (spec §6's three builds):
#   scripts/bench-ablation.sh before-4=<sha-before-§4> after-4=<sha-after-§4> after-5=<sha-after-§5>
#
# Environment overrides (used by the Task 2 smoke test, so it never touches
# the real workload file or the real output CSV):
#   WORKLOAD  the workload file passed to `ivmlite-bench ablation --workload`
#             (default: workloads/m0-baseline.toml)
#   OUT       the output CSV rows are appended to (default:
#             docs/bench/m1b-phase4-ablation.csv)
set -euo pipefail
cd "$(dirname "$0")/.."
repo_root="$(pwd)"

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <label>=<rev> [<label>=<rev> ...]" >&2
    exit 1
fi

workload="${WORKLOAD:-workloads/m0-baseline.toml}"
out="${OUT:-docs/bench/m1b-phase4-ablation.csv}"

if [ "$(uname)" = "Darwin" ]; then
    lib_name="libivmlite_sqlite.dylib"
else
    lib_name="libivmlite_sqlite.so"
fi

# Step 1: build the release bench once, from the current tree. This is the
# ordinary workspace build (crates/ivmlite-bench is a workspace member), so it
# uses the main checkout's own target/ — only the extension itself must never
# land there (crates/ivmlite-sqlite is built separately below, per worktree).
echo "building the release bench..." >&2
cargo build --release -p ivmlite-bench --locked
bench_bin="$repo_root/target/release/ivmlite-bench"

# Worktrees still pending removal; the trap below removes whatever is left
# here if the script exits early (an error, or Ctrl-C).
pending_worktrees=()

cleanup() {
    for wt in "${pending_worktrees[@]:-}"; do
        [ -n "$wt" ] || continue
        git worktree remove --force "$wt" >/dev/null 2>&1 || true
    done
}
trap cleanup EXIT

mkdir -p "$(dirname "$out")"
header_written=0
if [ -f "$out" ]; then
    header_written=1
fi

for pair in "$@"; do
    label="${pair%%=*}"
    rev="${pair#*=}"
    if [ "$label" = "$pair" ] || [ -z "$label" ] || [ -z "$rev" ]; then
        echo "invalid argument (want <label>=<rev>): $pair" >&2
        exit 1
    fi

    # Step 2: a detached checkout of $rev, under its own directory, so it
    # cannot collide with another label's worktree or with the main checkout.
    wt="${TMPDIR:-/tmp}/ivmlite-ablation-$label"
    rm -rf "$wt"
    git worktree add --detach "$wt" "$rev"
    pending_worktrees+=("$wt")

    echo "building the extension for $label ($rev)..." >&2
    CARGO_TARGET_DIR="$wt/target" cargo build --release --locked \
        --manifest-path "$wt/crates/ivmlite-sqlite/Cargo.toml"
    lib="$wt/target/release/$lib_name"

    # Step 3: run the ablation and append its rows to $out, writing the
    # header only for the very first build.
    tmp_csv="$(mktemp)"
    "$bench_bin" ablation --extension "$lib" --label "$label" --workload "$workload" >"$tmp_csv"
    if [ "$header_written" -eq 0 ]; then
        cat "$tmp_csv" >>"$out"
        header_written=1
    else
        tail -n +2 "$tmp_csv" >>"$out"
    fi
    rm -f "$tmp_csv"

    # Step 4: remove the worktree now that this label is done, rather than
    # waiting for every label to finish — a later label's failure still
    # leaves nothing of this one behind. Clear it from pending_worktrees so
    # the exit trap does not try (and fail) to remove it a second time.
    git worktree remove --force "$wt"
    remaining=()
    for w in "${pending_worktrees[@]}"; do
        [ "$w" = "$wt" ] || remaining+=("$w")
    done
    pending_worktrees=("${remaining[@]:-}")
done

echo "wrote $out" >&2
