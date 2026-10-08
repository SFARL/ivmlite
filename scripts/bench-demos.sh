#!/usr/bin/env bash
# Build the extension at each named revision and run this tree's demo
# benchmarks, `fluxflow_demo` and `demand_case_bench`, against each build
# through `--extension`, so only the extension differs between labels
# (M1b Phase 5 spec §7). Each revision gets a detached worktree and a
# separate Cargo target, as in scripts/bench-ablation.sh.
#
# Each demo runs at its documented scale (docs/demos/*.md) with its default
# repeats. The demo CSVs begin with `#` provenance lines; those are dropped,
# one header is kept, and every row gains a leading label column. A second
# CSV records the commit behind each label.
#
# Usage: scripts/bench-demos.sh <label>=<rev> [<label>=<rev> ...]
#
# Environment overrides, used by smoke tests:
#   DEMOS       newline-separated demo runs, each `<example> [arguments...]`
#               (default: the four demos at their documented scale)
#   OUT         demo CSV (default: docs/bench/m1b-phase5-demos.csv)
#   BUILDS_OUT  label-to-commit CSV (default: OUT with `-builds.csv`)
set -euo pipefail
cd "$(dirname "$0")/.."
repo_root="$(pwd)"

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <label>=<rev> [<label>=<rev> ...]" >&2
    exit 1
fi

# The source-scale commands of docs/demos/{fluxflow,noop,zcash,kener}.md,
# without the repeat count, so each demo uses its default.
default_demos="fluxflow_demo 1500000 500
demand_case_bench noop_gravity_witness 518400 200
demand_case_bench zcash_transparent_balance 500000 500
demand_case_bench kener_quarter_hour_rollup 4100000 500"
demos="${DEMOS:-$default_demos}"
out="${OUT:-docs/bench/m1b-phase5-demos.csv}"
builds_out="${BUILDS_OUT:-${out%.csv}-builds.csv}"

if [ "$(uname)" = "Darwin" ]; then
    lib_name="libivmlite_sqlite.dylib"
else
    lib_name="libivmlite_sqlite.so"
fi

# Validate every argument before any build, so a typo fails fast.
labels=()
commits=()
for pair in "$@"; do
    label="${pair%%=*}"
    rev="${pair#*=}"
    if [ "$label" = "$pair" ] || [ -z "$label" ] || [ -z "$rev" ]; then
        echo "invalid argument (want <label>=<rev>): $pair" >&2
        exit 1
    fi
    if [[ ! "$label" =~ ^[A-Za-z0-9._-]+$ ]]; then
        echo "invalid label (use letters, digits, dot, underscore or hyphen): $label" >&2
        exit 1
    fi
    commit="$(git rev-parse --verify --quiet "$rev^{commit}")" || {
        echo "not a commit: $rev" >&2
        exit 1
    }
    labels+=("$label")
    commits+=("$commit")
done

echo "building the demo benchmarks..." >&2
cargo build --release --locked -p ivmlite-test --example fluxflow_demo --example demand_case_bench
examples_dir="$repo_root/target/release/examples"

pending_worktrees=()
pending_roots=()
temporary_files=()

cleanup() {
    for wt in "${pending_worktrees[@]:-}"; do
        [ -n "$wt" ] || continue
        git worktree remove --force "$wt" >/dev/null 2>&1 || true
    done
    for root in "${pending_roots[@]:-}"; do
        [ -n "$root" ] || continue
        rmdir "$root" >/dev/null 2>&1 || true
    done
    for file in "${temporary_files[@]:-}"; do
        [ -n "$file" ] || continue
        rm -f "$file"
    done
}
trap cleanup EXIT

mkdir -p "$(dirname "$out")" "$(dirname "$builds_out")"
out_tmp="$(mktemp "$(dirname "$out")/.$(basename "$out" .csv).XXXXXX")"
builds_tmp="$(mktemp "$(dirname "$builds_out")/.$(basename "$builds_out" .csv).XXXXXX")"
temporary_files+=("$out_tmp" "$builds_tmp")
echo "label,commit" >"$builds_tmp"
header=""

for index in "${!labels[@]}"; do
    label="${labels[$index]}"
    commit="${commits[$index]}"
    echo "$label,$commit" >>"$builds_tmp"

    scratch_root="$(mktemp -d "${TMPDIR:-/tmp}/ivmlite-demos-${label}.XXXXXX")"
    wt="$scratch_root/worktree"
    pending_roots+=("$scratch_root")
    git worktree add --detach "$wt" "$commit"
    pending_worktrees+=("$wt")

    echo "building the extension for $label ($commit)..." >&2
    CARGO_TARGET_DIR="$wt/target" cargo build --release --locked \
        --manifest-path "$wt/crates/ivmlite-sqlite/Cargo.toml"
    lib="$wt/target/release/$lib_name"

    while IFS= read -r demo; do
        [ -n "$demo" ] || continue
        read -r -a words <<<"$demo"
        run_csv="$(mktemp)"
        temporary_files+=("$run_csv")
        started="$(date +%s)"
        "$examples_dir/${words[0]}" "${words[@]:1}" --extension "$lib" >"$run_csv"
        echo "$label: $demo took $(($(date +%s) - started)) s" >&2

        run_header="$(grep -v '^#' "$run_csv" | head -n 1)"
        if [ -z "$header" ]; then
            header="$run_header"
            echo "label,$header" >>"$out_tmp"
        elif [ "$run_header" != "$header" ]; then
            echo "$label: $demo printed a different header: $run_header" >&2
            exit 1
        fi
        grep -v '^#' "$run_csv" | tail -n +2 | awk -v label="$label" '{ print label "," $0 }' >>"$out_tmp"
        rm -f "$run_csv"
    done <<<"$demos"

    git worktree remove --force "$wt"
    rmdir "$scratch_root"
    remaining_worktrees=()
    for item in "${pending_worktrees[@]}"; do
        [ "$item" = "$wt" ] || remaining_worktrees+=("$item")
    done
    pending_worktrees=("${remaining_worktrees[@]:-}")
    remaining_roots=()
    for item in "${pending_roots[@]}"; do
        [ "$item" = "$scratch_root" ] || remaining_roots+=("$item")
    done
    pending_roots=("${remaining_roots[@]:-}")
done

mv "$out_tmp" "$out"
mv "$builds_tmp" "$builds_out"
chmod 0644 "$out" "$builds_out"
temporary_files=()
echo "wrote $out" >&2
echo "wrote $builds_out" >&2
