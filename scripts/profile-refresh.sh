#!/usr/bin/env bash
# Profile a steady refresh workload (Phase 5 spec §2 and §6): build the
# release extension, run scripts/profile-refresh.py's workload (10 views over
# 100,000 rows in 1,000 groups, every view refreshed after each 1,000-row
# insert) for 30 s, sample it for 10 s with macOS `sample`, and print the
# inclusive sample counts of the ivmlite functions under `view::refresh`,
# plus `view::refresh` and `view::apply` per source line.
#
# The release build carries debug line tables (CARGO_PROFILE_RELEASE_DEBUG),
# which name each frame's source line and do not change the generated code.
#
# macOS only: the report reads `sample`'s call-graph format. On Linux the
# equivalent is `perf record -g` on the driver's pid; that path is not
# implemented here.
#
# Environment overrides:
#   PYTHON          a python3 >= 3.12 whose sqlite3 can load extensions
#                   (default: python3)
#   RUN_SECONDS     how long the workload runs once ready (default: 30)
#   SAMPLE_DELAY    seconds between ready and sampling (default: 10)
#   SAMPLE_SECONDS  how long to sample (default: 10)
#   OUT_DIR         where the raw sample and driver output go
#                   (default: a new temporary directory)
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "$(uname)" != "Darwin" ]; then
    echo "$0: needs macOS \`sample\`; on Linux, run \`perf record -g -p <pid>\`" \
        "against \`$0\`'s driver (scripts/profile-refresh.py drive) by hand" >&2
    exit 1
fi

python="${PYTHON:-python3}"
run_seconds="${RUN_SECONDS:-30}"
sample_delay="${SAMPLE_DELAY:-10}"
sample_seconds="${SAMPLE_SECONDS:-10}"
out_dir="${OUT_DIR:-$(mktemp -d)}"
mkdir -p "$out_dir"

echo "building the release extension..." >&2
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only \
    cargo build --release --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml
lib="crates/ivmlite-sqlite/target/release/libivmlite_sqlite.dylib"

driver_out="$out_dir/driver.txt"
sample_out="$out_dir/sample.txt"
"$python" scripts/profile-refresh.py drive "$lib" --seconds "$run_seconds" \
    >"$driver_out" &
driver=$!
trap 'kill "$driver" 2>/dev/null || true' EXIT

echo "setting up the workload..." >&2
until grep -q '^ready ' "$driver_out" 2>/dev/null; do
    if ! kill -0 "$driver" 2>/dev/null; then
        echo "$0: the driver exited before it was ready" >&2
        exit 1
    fi
    sleep 0.2
done
sleep "$sample_delay"
echo "sampling for ${sample_seconds} s..." >&2
sample "$driver" "$sample_seconds" -file "$sample_out" >/dev/null
wait "$driver"
trap - EXIT

cat "$driver_out" >&2
echo "raw sample: $sample_out" >&2
echo
"$python" scripts/profile-refresh.py report "$sample_out" \
    --source crates/ivmlite-sqlite/src/view.rs
