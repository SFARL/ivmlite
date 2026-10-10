#!/usr/bin/env python3
"""Driver and report for scripts/profile-refresh.sh (Phase 5 spec §2, §6).

`drive EXTENSION [--seconds S]` runs the steady refresh workload: ten views of
the m0 shape over an `orders` table of 100,000 rows in 1,000 groups, in an
in-memory database. Every batch inserts 1,000 new rows, then refreshes every
view. Once the views exist, it prints `ready <pid>` on stdout, so a sampler
attaches to a process already in its steady state, and runs batches for S
seconds. It then prints the median and range of the refresh time per batch
(every view, the inserts excluded) and of the insert time per batch, and
checks every view against its query.

`report SAMPLE_FILE [--source FILE]` reads the call graph that macOS `sample`
wrote and prints the inclusive sample counts under `view::refresh`:
  - per `ivmlite_sqlite::` / `ivmlite_core::` function, each sample counted
    once per function even under recursion;
  - per source line of `view::refresh` and `view::apply`, which separates
    the calls one function makes (e.g. staging inserts from the arming
    statement), with the line's text read from `--source`.

`drive` needs a Python whose sqlite3 module can load extensions, 3.12 or
later (for `load_extension`'s `entrypoint`).
"""

import argparse
import os
import random
import re
import sqlite3
import statistics
import sys
import time
from collections import Counter

SEED = 47034
BASE_ROWS = 100_000
GROUPS = 1_000
AMOUNT_MAX = 200
VIEWS = 10
BATCH = 1_000
# The m0 workload's view thresholds: view i keeps amount > (i * 7) % 150.
THRESHOLD_STRIDE = 7
THRESHOLD_MODULUS = 150

REFRESH = "ivmlite_sqlite::view::refresh"
# Functions whose samples are also broken down by source line.
BY_LINE = (REFRESH, "ivmlite_sqlite::view::apply")
OWN_CRATES = ("ivmlite_sqlite::", "ivmlite_core::")
TOP = 25


# ---- drive -----------------------------------------------------------------

def rows(rng, first_id, count):
    return [(i, f"r{rng.randrange(GROUPS)}", rng.randrange(AMOUNT_MAX))
            for i in range(first_id, first_id + count)]


def view_sql(v, table):
    threshold = (v * THRESHOLD_STRIDE) % THRESHOLD_MODULUS
    return (f"SELECT region, SUM(amount), COUNT(*) FROM {table} "
            f"WHERE amount > {threshold} GROUP BY region")


def setup(extension):
    conn = sqlite3.connect(":memory:", isolation_level=None)
    conn.enable_load_extension(True)
    conn.load_extension(extension, entrypoint="sqlite3_ivmlite_init")
    conn.enable_load_extension(False)
    conn.execute(
        "CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, "
        "amount INTEGER NOT NULL) STRICT")
    rng = random.Random(SEED)
    conn.execute("BEGIN")
    conn.executemany("INSERT INTO orders VALUES (?, ?, ?)",
                     rows(rng, 1, BASE_ROWS))
    conn.execute("COMMIT")
    for v in range(VIEWS):
        sql = view_sql(v, '"orders"')
        conn.execute(f"CREATE VIRTUAL TABLE mv_{v} USING ivm('{sql}')")
    return conn, rng


def summary(label, samples_ms):
    return (f"{label}: median {statistics.median(samples_ms):.2f} ms/batch "
            f"(min {min(samples_ms):.2f}, max {max(samples_ms):.2f}, "
            f"{len(samples_ms)} batches)")


def verify(conn):
    """Fail unless every view equals its query over the base table."""
    for v in range(VIEWS):
        want = sorted(conn.execute(view_sql(v, "orders")))
        got = sorted(conn.execute(f"SELECT * FROM mv_{v}"))
        if got != want:
            sys.exit(f"mv_{v} disagrees with its query")


def drive(args):
    conn, rng = setup(args.extension)
    refreshes = [f"INSERT INTO mv_{v}(mv_{v}) VALUES ('refresh')"
                 for v in range(VIEWS)]
    print(f"ready {os.getpid()}", flush=True)
    next_id = BASE_ROWS + 1
    insert_ms, refresh_ms = [], []
    deadline = time.monotonic() + args.seconds
    while time.monotonic() < deadline:
        batch = rows(rng, next_id, BATCH)
        next_id += BATCH
        t0 = time.perf_counter()
        conn.execute("BEGIN")
        conn.executemany("INSERT INTO orders VALUES (?, ?, ?)", batch)
        conn.execute("COMMIT")
        t1 = time.perf_counter()
        for sql in refreshes:
            conn.execute(sql)
        t2 = time.perf_counter()
        insert_ms.append((t1 - t0) * 1e3)
        refresh_ms.append((t2 - t1) * 1e3)
    print(summary("refresh", refresh_ms), flush=True)
    print(summary("insert", insert_ms), flush=True)
    verify(conn)


# ---- report ----------------------------------------------------------------

# One call-graph node: tree-drawing characters, the inclusive count, the
# frame, and (with debug line tables) the frame's `file:line`.
NODE = re.compile(r"^(?P<indent>[ +!:|]*)(?P<count>\d+) (?P<frame>.*)$")
LOCATION = re.compile(r"\]\s+(?P<file>[\w.\-]+):(?P<line>\d+)\s*$")
HASH = re.compile(r"::h[0-9a-f]{16}$")
ESCAPES = {"$LT$": "<", "$GT$": ">", "$u20$": " ", "$RF$": "&", "$C$": ",",
           "$u7b$": "{", "$u7d$": "}", "$BP$": "*", "$LP$": "(", "$RP$": ")"}


def demangle(symbol):
    """Turn a legacy-mangled Rust symbol as `sample` prints it into a path."""
    name = HASH.sub("", symbol)
    if name.startswith("_$"):
        name = name[1:]
    for escape, char in ESCAPES.items():
        name = name.replace(escape, char)
    return name.replace("..", "::")


def call_graph(path):
    """Yield (depth, count, function, location) for every node."""
    inside = False
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            if line.startswith("Call graph:"):
                inside = True
                continue
            if not inside:
                continue
            if not line.strip():
                return
            m = NODE.match(line.rstrip("\n"))
            if not m:
                continue
            frame = m["frame"]
            function = demangle(frame.split("  (in ", 1)[0].strip())
            loc = LOCATION.search(frame)
            location = (loc["file"], int(loc["line"])) if loc else None
            yield len(m["indent"]), int(m["count"]), function, location


def is_own(function):
    return any(c in function for c in OWN_CRATES)


def tally(path):
    """Inclusive counts under the outermost `view::refresh` frames."""
    total = 0
    by_function = Counter()
    by_line = Counter()
    ancestors = []  # (depth, function)
    for depth, count, function, location in call_graph(path):
        while ancestors and ancestors[-1][0] >= depth:
            ancestors.pop()
        above = {f for _, f in ancestors}
        under_refresh = REFRESH in above
        if function == REFRESH and not under_refresh:
            total += count
        if (function == REFRESH or under_refresh) and is_own(function) \
                and function not in above:
            by_function[function] += count
            if function in BY_LINE and location:
                by_line[(function, location)] += count
        ancestors.append((depth, function))
    return total, by_function, by_line


def source_line(source, location):
    if not source or os.path.basename(source) != location[0]:
        return ""
    with open(source, encoding="utf-8") as f:
        lines = f.read().splitlines()
    n = location[1]
    return lines[n - 1].strip() if 0 < n <= len(lines) else ""


def report(args):
    total, by_function, by_line = tally(args.sample)
    if total == 0:
        sys.exit(f"no {REFRESH} frame in {args.sample}")
    print(f"{REFRESH}: {total} samples (= 100%)\n")
    print("inclusive samples per function under view::refresh:")
    for function, count in by_function.most_common(TOP):
        print(f"{count:8d} {100 * count / total:5.1f}%  {function}")
    for owner in BY_LINE:
        print(f"\ninclusive samples per source line of {owner}:")
        lines = sorted(((c, loc) for (f, loc), c in by_line.items()
                        if f == owner), reverse=True)
        for count, location in lines:
            text = source_line(args.source, location)
            print(f"{count:8d} {100 * count / total:5.1f}%  "
                  f"{location[0]}:{location[1]}  {text}")


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="command", required=True)
    d = sub.add_parser("drive", help="run the steady refresh workload")
    d.add_argument("extension", help="path to libivmlite_sqlite.{dylib,so}")
    d.add_argument("--seconds", type=float, default=30.0,
                   help="how long to run batches once ready (default 30)")
    r = sub.add_parser("report", help="summarize a macOS `sample` file")
    r.add_argument("sample", help="the file `sample -file` wrote")
    r.add_argument("--source", help="view.rs, to print each line's text")
    args = p.parse_args()
    if args.command == "drive":
        drive(args)
    else:
        report(args)


if __name__ == "__main__":
    main()
