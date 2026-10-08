#!/usr/bin/env bash
# Build the extension at each named revision and measure both the fixed
# performance-ablation cells (the m0 workload's [ablation] section) and
# ivmlite's write amplification at the write-amp workload's
# ablation_view_counts. Each revision gets a detached worktree and a separate
# Cargo target.
#
# The write-amp run measures all three write-amp engines, but only the
# ivmlite rows are kept: their extra_us_per_row column is computed against
# that run's no_maintenance rows, which are not written to the output.
#
# Usage: scripts/bench-ablation.sh <label>=<rev> [<label>=<rev> ...]
#
# Environment overrides, used by smoke tests:
#   WORKLOAD       ablation workload (default: workloads/m0-baseline.toml)
#   WRITE_WORKLOAD write-amplification workload (default: workloads/write-amp.toml)
#   OUT            performance CSV (default: docs/bench/m1b-phase4-ablation.csv)
#   WRITE_AMP_OUT  write-amplification CSV
#                  (default: docs/bench/m1b-phase4-ablation-write-amp.csv)
set -euo pipefail
cd "$(dirname "$0")/.."
repo_root="$(pwd)"

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <label>=<rev> [<label>=<rev> ...]" >&2
    exit 1
fi

workload="${WORKLOAD:-workloads/m0-baseline.toml}"
write_workload="${WRITE_WORKLOAD:-workloads/write-amp.toml}"
out="${OUT:-docs/bench/m1b-phase4-ablation.csv}"
write_amp_out="${WRITE_AMP_OUT:-docs/bench/m1b-phase4-ablation-write-amp.csv}"

if [ "$(uname)" = "Darwin" ]; then
    lib_name="libivmlite_sqlite.dylib"
else
    lib_name="libivmlite_sqlite.so"
fi

echo "building the release bench..." >&2
cargo build --release -p ivmlite-bench --locked
bench_bin="$repo_root/target/release/ivmlite-bench"

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

mkdir -p "$(dirname "$out")" "$(dirname "$write_amp_out")"
out_tmp="$(mktemp "$(dirname "$out")/.$(basename "$out" .csv).XXXXXX")"
write_amp_out_tmp="$(mktemp "$(dirname "$write_amp_out")/.$(basename "$write_amp_out" .csv).XXXXXX")"
temporary_files+=("$out_tmp" "$write_amp_out_tmp")
header_written=0
write_amp_header_written=0

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

    scratch_root="$(mktemp -d "${TMPDIR:-/tmp}/ivmlite-ablation-${label}.XXXXXX")"
    wt="$scratch_root/worktree"
    pending_roots+=("$scratch_root")
    git worktree add --detach "$wt" "$rev"
    pending_worktrees+=("$wt")

    echo "building the extension for $label ($rev)..." >&2
    CARGO_TARGET_DIR="$wt/target" cargo build --release --locked \
        --manifest-path "$wt/crates/ivmlite-sqlite/Cargo.toml"
    lib="$wt/target/release/$lib_name"

    run_csv="$(mktemp)"
    temporary_files+=("$run_csv")
    "$bench_bin" ablation --extension "$lib" --label "$label" --workload "$workload" >"$run_csv"
    if [ "$header_written" -eq 0 ]; then
        cat "$run_csv" >>"$out_tmp"
        header_written=1
    else
        tail -n +2 "$run_csv" >>"$out_tmp"
    fi
    rm -f "$run_csv"

    write_amp_csv="$(mktemp)"
    temporary_files+=("$write_amp_csv")
    "$bench_bin" write-amp --extension "$lib" --workload "$write_workload" --ablation-views >"$write_amp_csv"
    if [ "$write_amp_header_written" -eq 0 ]; then
        awk -F, -v label="$label" '
            NR == 1 { print "label," $0; next }
            $1 == "ivmlite" { print label "," $0 }
        ' "$write_amp_csv" >>"$write_amp_out_tmp"
        write_amp_header_written=1
    else
        awk -F, -v label="$label" '
            NR > 1 && $1 == "ivmlite" { print label "," $0 }
        ' "$write_amp_csv" >>"$write_amp_out_tmp"
    fi
    rm -f "$write_amp_csv"

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
mv "$write_amp_out_tmp" "$write_amp_out"
chmod 0644 "$out" "$write_amp_out"
temporary_files=()
echo "wrote $out" >&2
echo "wrote $write_amp_out" >&2
