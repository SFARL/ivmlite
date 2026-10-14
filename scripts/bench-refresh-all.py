#!/usr/bin/env python3
"""Measure the call shapes of a refresh of every view (M1b Phase 7 spec §6).

Usage:
    python3 scripts/bench-refresh-all.py BEFORE.dylib AFTER.dylib \
        --before-commit SHA --after-commit SHA [--rounds N] \
        [--out CSV] [--builds-out CSV]

BEFORE has no `ivm_refresh_all`, so only the per-view loop shapes run on it.
Every case runs in a fresh in-memory database, on the m0 shape: 100,000 base
rows in 1,000 groups, amounts in [0, 200), and the matrix's views
`SELECT region, SUM(amount), COUNT(*) FROM t WHERE amount > k GROUP BY region`
with `k = (i * 7) % 150`. The rows and batches come from Python's own seeded
generator, not from `ivmlite-workload`'s, so they are the same shape as the
matrix's, not the same rows.

Each case applies one warm-up batch and refreshes every view, then five
measured batches. Only the refresh of every view is timed; each batch's
writes run in one transaction before it. After the last batch every view is
compared with SQLite's own evaluation of its query, and a mismatch stops the
run. Builds alternate per case (and per round), so drift hits both alike.

Shapes, each refreshing every view, in `name` order:
- `app_cached`: one `INSERT INTO v(v) VALUES ('refresh')` per view, with the
  same SQL text every time, on a connection whose statement cache holds every
  view's statement, so each is prepared once and reused;
- `reprepared`: the same statements on a connection with
  `cached_statements=0`, so each is prepared and finalized on every call;
- `refresh_all`: `SELECT ivm_refresh_all()`;
- `refresh_all_continue`: `SELECT ivm_refresh_all('continue')`;
- `reprepared_with_checks`: `reprepared`, plus, before each view's
  statement, the statements the function runs for that view to count skips
  (spec §4): its base tables from `__ivm_dep`, then per table its watermark
  and the delta table's `MAX(__ivm_seq)`, each prepared afresh, as the
  function does. It is the function's per-view statements issued from Python,
  without its savepoint.

Cases:
- `views`: 10 and 200 views over one table, at batch 1 and batch 1,000;
- `skip`: 200 views over two tables of 100,000 rows each (100 views per
  table); `write=one` writes a one-row batch to the first table only,
  `write=none` writes nothing.

Writes one CSV row per timed refresh (`batch` 0 is the warm-up) and a builds
CSV mapping each label to its commit, the library's SHA-256 and the SQLite
version. Python 3.12+ and the standard library only.
"""
import argparse
import csv
import hashlib
import os
import random
import sqlite3
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ENTRYPOINT = "sqlite3_ivmlite_init"

BASE_ROWS = 100_000
GROUPS = 1_000
AMOUNT_MAX = 200
DELETE_RATIO = 0.33
SEED = 47034
BATCH_SEED = SEED ^ 0x5EED
THRESHOLD_STRIDE = 7
THRESHOLD_MODULUS = 150
MEASURED_BATCHES = 5
# Holds every view's refresh statement and the batch's own statements, so
# `app_cached` never misses (Python's default is 128).
APP_CACHE = 1024

VIEW_COUNTS = [10, 200]
BATCH_SIZES = [1, 1_000]
SKIP_VIEWS = 200
SKIP_TABLES = 2
SKIP_BATCH = 1

LOOP_SHAPES = ["app_cached", "reprepared"]
AFTER_SHAPES = LOOP_SHAPES + ["refresh_all", "refresh_all_continue", "reprepared_with_checks"]
BUILDS = ["before", "after"]

FIELDS = [
    "build", "case", "views", "tables", "batch_size", "write", "shape",
    "round", "batch", "refresh_ms", "result", "load1",
]


def table_ddl(table):
    return (
        f'CREATE TABLE "{table}"(id INTEGER PRIMARY KEY, '
        "region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT"
    )


def view_query(table, threshold):
    return (
        f'SELECT region, SUM(amount), COUNT(*) FROM "{table}" '
        f"WHERE amount > {threshold} GROUP BY region"
    )


class Table:
    """One base table and its batch generator: deletes hit distinct base
    rows, inserts take fresh ids, so no row is touched twice."""

    def __init__(self, name, seed):
        self.name = name
        self.rng = random.Random(seed)
        self.next_id = BASE_ROWS
        self.deleted = set()

    def rows(self, seed):
        rng = random.Random(seed)
        for i in range(BASE_ROWS):
            g = i if i < GROUPS else rng.randrange(GROUPS)
            yield (i, f"r{g}", rng.randrange(AMOUNT_MAX))

    def batch(self, size):
        ops = []
        for _ in range(size):
            if self.rng.random() < DELETE_RATIO and len(self.deleted) < BASE_ROWS:
                rid = self.rng.randrange(BASE_ROWS)
                while rid in self.deleted:
                    rid = self.rng.randrange(BASE_ROWS)
                self.deleted.add(rid)
                ops.append(("delete", rid))
            else:
                ops.append(("insert", (self.next_id, f"r{self.rng.randrange(GROUPS)}",
                                       self.rng.randrange(AMOUNT_MAX))))
                self.next_id += 1
        return ops


def connect(lib, shape):
    cache = 0 if shape.startswith("reprepared") else APP_CACHE
    conn = sqlite3.connect(":memory:", isolation_level=None, cached_statements=cache)
    conn.enable_load_extension(True)
    conn.load_extension(lib, entrypoint=ENTRYPOINT)
    conn.enable_load_extension(False)
    return conn


def build_case(conn, tables, views):
    """Load the base tables, then create `views` views spread evenly over
    them. Returns [(view name, table, threshold)] in name order."""
    for t_index, t in enumerate(tables):
        conn.execute(table_ddl(t.name))
        conn.execute("BEGIN")
        conn.executemany(
            f'INSERT INTO "{t.name}" VALUES (?, ?, ?)', t.rows(SEED + t_index)
        )
        conn.execute("COMMIT")
    per_table = views // len(tables)
    specs = []
    for t_index, t in enumerate(tables):
        for i in range(per_table):
            name = f"v{t_index * per_table + i:03d}"
            threshold = (i * THRESHOLD_STRIDE) % THRESHOLD_MODULUS
            sql = view_query(t.name, threshold).replace("'", "''")
            conn.execute(f'CREATE VIRTUAL TABLE "{name}" USING ivm(\'{sql}\')')
            specs.append((name, t.name, threshold))
    return sorted(specs)


def apply_batch(conn, table, ops):
    conn.execute("BEGIN")
    for kind, arg in ops:
        if kind == "delete":
            conn.execute(f'DELETE FROM "{table.name}" WHERE id = ?', (arg,))
        else:
            conn.execute(f'INSERT INTO "{table.name}" VALUES (?, ?, ?)', arg)
    conn.execute("COMMIT")


def refresher(conn, shape, specs):
    """A function that refreshes every view in `shape` and returns the
    function's result, or None for a per-view loop."""
    stmts = [f'INSERT INTO "main"."{n}"("{n}") VALUES (\'refresh\')' for n, _, _ in specs]

    def loop():
        for s in stmts:
            conn.execute(s)

    def with_checks():
        for (name, _, _), s in zip(specs, stmts):
            deps = [r[0] for r in conn.execute(
                'SELECT tbl FROM "main"."__ivm_dep" WHERE view = ?1 ORDER BY tbl', (name,))]
            for tbl in deps:
                conn.execute(
                    'SELECT applied_seq FROM "main"."__ivm_progress" '
                    "WHERE view = ?1 AND tbl = ?2", (name, tbl)).fetchone()
                conn.execute(
                    f'SELECT MAX("__ivm_seq") FROM "main"."__ivm_delta_{tbl}"').fetchone()
            conn.execute(s)

    def call(sql):
        return lambda: conn.execute(sql).fetchone()[0]

    return {
        "app_cached": loop,
        "reprepared": loop,
        "refresh_all": call("SELECT ivm_refresh_all()"),
        "refresh_all_continue": call("SELECT ivm_refresh_all('continue')"),
        "reprepared_with_checks": with_checks,
    }[shape]


def verify(conn, specs, label):
    for name, table, threshold in specs:
        got = conn.execute(f'SELECT * FROM "{name}" ORDER BY 1').fetchall()
        want = conn.execute(view_query(table, threshold) + " ORDER BY 1").fetchall()
        if got != want:
            sys.exit(f"bench-refresh-all.py: {label}: view {name} does not match its query")


def run_case(lib, build, case, shape, rnd, writer):
    tables = [Table(f"t{i}", BATCH_SEED + i) for i in range(case["tables"])]
    conn = connect(lib, shape)
    load1 = f"{os.getloadavg()[0]:.2f}"
    specs = build_case(conn, tables, case["views"])
    refresh = refresher(conn, shape, specs)
    label = f"{build} {case['name']} {shape} round {rnd}"
    times = []
    for batch in range(MEASURED_BATCHES + 1):
        if case["write"] == "all" or case["write"] == "one":
            apply_batch(conn, tables[0], tables[0].batch(case["batch_size"]))
        start = time.perf_counter()
        result = refresh()
        ms = (time.perf_counter() - start) * 1000
        if batch > 0:
            times.append(ms)
        writer.writerow({
            "build": build, "case": case["name"], "views": case["views"],
            "tables": case["tables"], "batch_size": case["batch_size"],
            "write": case["write"], "shape": shape, "round": rnd, "batch": batch,
            "refresh_ms": f"{ms:.3f}", "result": "" if result is None else result,
            "load1": load1,
        })
    verify(conn, specs, label)
    conn.close()
    times.sort()
    print(f"{label}: median {times[len(times) // 2]:.3f} ms", file=sys.stderr, flush=True)


def cases():
    out = []
    for views in VIEW_COUNTS:
        for batch in BATCH_SIZES:
            out.append({"name": f"views={views} batch={batch}", "views": views,
                        "tables": 1, "batch_size": batch, "write": "all"})
    for write in ["one", "none"]:
        out.append({"name": f"skip write={write}", "views": SKIP_VIEWS,
                    "tables": SKIP_TABLES,
                    "batch_size": SKIP_BATCH if write == "one" else 0, "write": write})
    return out


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def loadavg():
    return " / ".join(f"{x:.2f}" for x in os.getloadavg())


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("before")
    p.add_argument("after")
    p.add_argument("--before-commit", required=True)
    p.add_argument("--after-commit", required=True)
    p.add_argument("--rounds", type=int, default=1)
    p.add_argument("--out", default=ROOT / "docs/bench/m1b-phase7-refresh-all.csv")
    p.add_argument("--builds-out", default=ROOT / "docs/bench/m1b-phase7-refresh-all-builds.csv")
    args = p.parse_args()
    libs = {"before": str(Path(args.before).resolve()), "after": str(Path(args.after).resolve())}
    commits = {"before": args.before_commit, "after": args.after_commit}

    print(f"load average at start: {loadavg()}", file=sys.stderr, flush=True)
    wall = time.perf_counter()
    with open(args.out, "w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS)
        writer.writeheader()
        for rnd in range(1, args.rounds + 1):
            for i, case in enumerate(cases()):
                order = BUILDS if (i + rnd) % 2 == 0 else BUILDS[::-1]
                for shape in AFTER_SHAPES:
                    for build in order:
                        if build == "before" and shape not in LOOP_SHAPES:
                            continue
                        run_case(libs[build], build, case, shape, rnd, writer)
                        f.flush()
    with open(args.builds_out, "w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["label", "commit", "sha256", "sqlite"])
        for build in BUILDS:
            writer.writerow([build, commits[build], sha256(libs[build]), sqlite3.sqlite_version])
    print(f"load average at end: {loadavg()}", file=sys.stderr)
    print(f"wall time: {time.perf_counter() - wall:.2f} s", file=sys.stderr)


if __name__ == "__main__":
    main()
