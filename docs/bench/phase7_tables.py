#!/usr/bin/env python3
"""Derive every number in docs/bench/README.md's "M1b Phase 7" section from
m1b-phase7-refresh-all.csv and m1b-phase7-refresh-all-builds.csv.

The CSV is written by scripts/bench-refresh-all.py (M1b Phase 7 spec §6): one
row per timed refresh of every view, `batch` 0 being the warm-up. Each value
below is the median [min, max] over the measured batches of every round of a
case. It reads only the two CSVs next to it and uses only the standard
library. A failed self-check exits with status 1.

Usage: python3 docs/bench/phase7_tables.py
"""
import collections
import csv
import statistics
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DATA = "m1b-phase7-refresh-all.csv"
BUILDS_CSV = "m1b-phase7-refresh-all-builds.csv"

BUILDS = ["before", "after"]
LOOP_SHAPES = ["app_cached", "reprepared"]
FUNCTION_SHAPES = ["refresh_all", "refresh_all_continue"]
SHAPES = LOOP_SHAPES + FUNCTION_SHAPES + ["reprepared_with_checks"]
MEASURED_BATCHES = 5
VIEW_CASES = ["views=10 batch=1", "views=10 batch=1000", "views=200 batch=1", "views=200 batch=1000"]
SKIP_CASES = ["skip write=one", "skip write=none"]
# Views of the skip cases per base table: `write=one` writes the first table
# only, so its 100 views are behind and the other 100 are not.
SKIP_VIEWS_PER_TABLE = 100


def load(name):
    with open(HERE / name, newline="") as f:
        return list(csv.DictReader(f))


def require(ok, what):
    """Stop with status 1 when a self-check of the committed data fails."""
    if not ok:
        print(f"phase7_tables.py: self-check failed: {what}", file=sys.stderr)
        sys.exit(1)
    print(f"self-check passed: {what}")


def section(title):
    print()
    print(f"== {title} ==")


def med_range(xs):
    return statistics.median(xs), min(xs), max(xs)


def fmt_mr(m, digits=3):
    return f"{m[0]:,.{digits}f} [{m[1]:,.{digits}f}, {m[2]:,.{digits}f}]"


def overlap(a, b):
    return a[1] <= b[2] and b[1] <= a[2]


def ratio_note(a, b):
    """`a / b` of the medians, with whether the two ranges overlap."""
    r = a[0] / b[0]
    return f"{r:.3f}x" + (", ranges overlap" if overlap(a, b) else "")


rows = load(DATA)
builds = load(BUILDS_CSV)

# ---------------------------------------------------------------------------
# Self-checks
# ---------------------------------------------------------------------------

section("Self-checks")
require([b["label"] for b in builds] == BUILDS, f"the builds CSV lists exactly {BUILDS}")
require(all(len(b["commit"]) == 40 for b in builds), "every build names a full commit")
require({r["build"] for r in rows} == set(BUILDS), "the data holds exactly the labelled builds")
require({r["shape"] for r in rows} == set(SHAPES), f"the data holds exactly the shapes {SHAPES}")
require({r["case"] for r in rows} == set(VIEW_CASES + SKIP_CASES), "the data holds exactly the expected cases")
require(
    not any(r["build"] == "before" and r["shape"] not in LOOP_SHAPES for r in rows),
    "the before build, which has no ivm_refresh_all, has only the loop shapes",
)

runs = collections.defaultdict(lambda: collections.defaultdict(list))
case_of = {}
for r in rows:
    key = (r["build"], r["case"], r["shape"])
    runs[key][int(r["round"])].append(r)
    case_of[r["case"]] = r

rounds = {rnd for per_round in runs.values() for rnd in per_round}
require(rounds == set(range(1, max(rounds) + 1)), f"rounds 1..{max(rounds)} are present")
expected_keys = {(b, c, s) for c in VIEW_CASES + SKIP_CASES for s in SHAPES for b in BUILDS
                 if b == "after" or s in LOOP_SHAPES}
require(set(runs) == expected_keys, "every (build, case, shape) the driver runs is present")
require(
    all(set(per_round) == rounds for per_round in runs.values()),
    "every (build, case, shape) ran in every round",
)
require(
    all(sorted(int(r["batch"]) for r in rs) == list(range(MEASURED_BATCHES + 1))
        for per_round in runs.values() for rs in per_round.values()),
    f"every run has one warm-up and {MEASURED_BATCHES} measured batches",
)


def refreshed_in(case):
    """How many views have new deltas in each batch of `case`: all of them
    in the view cases; in the skip cases only the written table's views."""
    c = case_of[case]
    return {"all": int(c["views"]), "one": SKIP_VIEWS_PER_TABLE, "none": 0}[c["write"]]


def expected_result(case, shape):
    """What the function returns in `case` (spec §2, §4)."""
    views = int(case_of[case]["views"])
    refreshed = refreshed_in(case)
    if shape == "refresh_all":
        return str(refreshed)
    return f"refreshed {refreshed}, skipped {views - refreshed}, failed 0"


def results_match():
    for (build, case, shape), per_round in runs.items():
        for rs in per_round.values():
            for r in rs:
                want = expected_result(case, shape) if shape in FUNCTION_SHAPES else ""
                if r["result"] != want:
                    print(f"  {build} {case} {shape} round {r['round']} batch {r['batch']}: "
                          f"{r['result']!r}, expected {want!r}", file=sys.stderr)
                    return False
    return True


require(results_match(), "every call returned the expected counts (refreshed, skipped, failed 0)")


def stats(build, case, shape, warm=False):
    per_round = runs[(build, case, shape)]
    xs = [float(r["refresh_ms"]) for rs in per_round.values() for r in rs
          if (int(r["batch"]) == 0) == warm]
    return med_range(xs)


def round_medians(build, case, shape):
    return [statistics.median(float(r["refresh_ms"]) for r in rs if int(r["batch"]) > 0)
            for _, rs in sorted(runs[(build, case, shape)].items())]


def views_of(case):
    return int(case_of[case]["views"])


# ---------------------------------------------------------------------------
# Setup facts from the data
# ---------------------------------------------------------------------------

section("Builds and load")
for b in builds:
    print(f"{b['label']}: {b['commit'][:7]} ({b['commit']}), SQLite {b['sqlite']}, sha256 {b['sha256'][:12]}")
loads = [float(r["load1"]) for r in rows]
print(f"rounds: {max(rounds)}; measured batches per case: {MEASURED_BATCHES * max(rounds)}")
print(f"1-minute load average at the start of each case: {min(loads):.2f} to {max(loads):.2f} "
      f"(median {statistics.median(loads):.2f})")

# ---------------------------------------------------------------------------
# The shapes side by side
# ---------------------------------------------------------------------------

section("Refresh of every view, ms, median [min, max], by case and shape")
print("| case | shape | before | after |")
print("|---|---|---:|---:|")
for case in VIEW_CASES + SKIP_CASES:
    for shape in SHAPES:
        before = fmt_mr(stats("before", case, shape)) if shape in LOOP_SHAPES else "absent"
        print(f"| {case} | {shape} | {before} | {fmt_mr(stats('after', case, shape))} |")

section("Warm-up (first refresh after CREATE), ms, median [min, max] over rounds, after build")
print("| case | " + " | ".join(SHAPES) + " |")
print("|---|" + "---:|" * len(SHAPES))
for case in VIEW_CASES + SKIP_CASES:
    print(f"| {case} | " + " | ".join(fmt_mr(stats("after", case, s, warm=True)) for s in SHAPES) + " |")

# ---------------------------------------------------------------------------
# ivm_refresh_all against the app-cached loop
# ---------------------------------------------------------------------------

section("ivm_refresh_all against the app-cached loop (after build)")
print("Ratio of medians (above 1 means the function is slower); extra µs per view "
      "is the difference of medians over the view count; per-round ratios are "
      "each round's median over the other's.")
print("| case | app_cached ms | refresh_all ms | ratio | extra µs/view | per-round ratios | continue ms | continue ratio |")
print("|---|---:|---:|---:|---:|---:|---:|---:|")
for case in VIEW_CASES + SKIP_CASES:
    cached = stats("after", case, "app_cached")
    fn = stats("after", case, "refresh_all")
    cont = stats("after", case, "refresh_all_continue")
    per_round = [f / c for f, c in zip(round_medians("after", case, "refresh_all"),
                                        round_medians("after", case, "app_cached"))]
    extra = (fn[0] - cached[0]) / views_of(case) * 1000
    print(f"| {case} | {fmt_mr(cached)} | {fmt_mr(fn)} | {ratio_note(fn, cached)} | "
          f"{extra:+.1f} | {min(per_round):.3f}–{max(per_round):.3f}x | {fmt_mr(cont)} | "
          f"{ratio_note(cont, cached)} |")

# ---------------------------------------------------------------------------
# Where the function's extra cost goes
# ---------------------------------------------------------------------------

section("Per-view cost of the statements the function prepares on every call (after build), µs per view")
print("re-prepare = reprepared - app_cached (one INSERT INTO v(v) prepared and finalized);")
print("checks = reprepared_with_checks - reprepared (the skip count's statements, prepared afresh, from Python);")
print("function overhead = refresh_all - app_cached; rest = refresh_all - reprepared_with_checks.")
print("app_cached and refresh_all run on a connection with Python's statement cache on, the two")
print("reprepared shapes on one with it off (cached_statements=0); only same-setup differences")
print("(function overhead, checks) compare like with like.")
print("| case | re-prepare | checks | function overhead | rest |")
print("|---|---:|---:|---:|---:|")
for case in VIEW_CASES + SKIP_CASES:
    n = views_of(case)
    m = {s: stats("after", case, s)[0] for s in SHAPES}

    def per_view(a, b):
        return (m[a] - m[b]) / n * 1000

    print(f"| {case} | {per_view('reprepared', 'app_cached'):+.1f} | "
          f"{per_view('reprepared_with_checks', 'reprepared'):+.1f} | "
          f"{per_view('refresh_all', 'app_cached'):+.1f} | "
          f"{per_view('refresh_all', 'reprepared_with_checks'):+.1f} |")

section("Re-prepare cost per view in the before build, µs per view (reprepared - app_cached)")
for case in VIEW_CASES + SKIP_CASES:
    m = {s: stats("before", case, s)[0] for s in LOOP_SHAPES}
    print(f"{case}: {(m['reprepared'] - m['app_cached']) / views_of(case) * 1000:+.1f}")

# ---------------------------------------------------------------------------
# Skipping
# ---------------------------------------------------------------------------

section("The early return (spec §4): before against after, same shape")
print("| case | shape | before ms | after ms | before / after | saved per idle view µs |")
print("|---|---|---:|---:|---:|---:|")
for case in SKIP_CASES:
    idle = views_of(case) - refreshed_in(case)
    for shape in LOOP_SHAPES:
        b, a = stats("before", case, shape), stats("after", case, shape)
        print(f"| {case} | {shape} | {fmt_mr(b)} | {fmt_mr(a)} | {ratio_note(b, a)} | "
              f"{(b[0] - a[0]) / idle * 1000:.1f} |")

section("Idle view cost, µs per view, write=none (every view idle)")
case = "skip write=none"
for build in BUILDS:
    for shape in SHAPES:
        if build == "before" and shape not in LOOP_SHAPES:
            continue
        print(f"{build} {shape}: {stats(build, case, shape)[0] / views_of(case) * 1000:.1f}")

section("ivm_refresh_all (after) against the loops of the before build, in the skip cases")
for case in SKIP_CASES:
    fn = stats("after", case, "refresh_all")
    for shape in LOOP_SHAPES:
        b = stats("before", case, shape)
        print(f"{case}: before {shape} {fmt_mr(b)} / after refresh_all {fmt_mr(fn)} = {ratio_note(b, fn)}")

# ---------------------------------------------------------------------------
# The control: refresh work no build changes
# ---------------------------------------------------------------------------

section("Control: the loop shapes in the view cases, where every view is behind (after / before)")
print("Every view has new deltas in these cases, so the early return never fires and "
      "both builds do the same refresh work.")
ctrl = []
for case in VIEW_CASES:
    for shape in LOOP_SHAPES:
        b, a = stats("before", case, shape), stats("after", case, shape)
        ctrl.append(a[0] / b[0])
        print(f"{case} {shape}: before {fmt_mr(b)}, after {fmt_mr(a)}, after/before {ratio_note(a, b)}")
print(f"after/before over the control cells: {min(ctrl):.3f}–{max(ctrl):.3f}x")

section("reprepared / app_cached, ratio of medians (the two loops' connection setups differ)")
for build in BUILDS:
    for case in VIEW_CASES + SKIP_CASES:
        r, c = stats(build, case, "reprepared"), stats(build, case, "app_cached")
        print(f"{build} {case}: {ratio_note(r, c)}")
