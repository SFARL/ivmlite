#!/usr/bin/env python3
"""Derive every number in docs/bench/README.md's "M1b Phase 6" section from
the committed CSVs in this directory.

The README's Phase 6 tables and the figures quoted in its prose are copied
from this script's output, so they can be re-derived after any change to the
data. It reads only the CSVs next to it (Phase 6's, plus Phase 5's matrix,
confirmation and demos for the per-cell change) and uses only the standard
library.

Phase 6 measures each cell's steady state (M1b Phase 6 spec §6): one warm-up
batch, reported as `first_refresh_ms`, then five measured batches, reported as
medians. Phase 4 and Phase 5 measured one batch, the first refresh after
`CREATE`, so their numbers are not comparable column for column with these.

Usage: python3 docs/bench/phase6_tables.py
"""
import collections
import csv
import statistics
from pathlib import Path

HERE = Path(__file__).resolve().parent

# Phase 4 spec §3.5's confirmation rule, restated here only to check that the
# committed confirmation CSV holds exactly the cells the rule selects.
NEAR_1X = (0.7, 1.4)
NEAR_2X = (1.4, 2.8)

# The ablation's builds, in commit order; each change is measured against
# the build before it (Phase 6 spec §7).
BUILDS = ["before", "stmtcache", "snapshot", "progidx"]
CHANGES = {
    "stmtcache": "§3 statement cache",
    "snapshot": "§4 shared catalog snapshot",
    "progidx": "§5 progress index (format 5)",
}
DEMO_BUILDS = ["before", "after"]

# Phase 6 spec §7's acceptance rule: a change is kept only if it makes its
# metric at least this much lower (time saved) in each cell it targets.
ACCEPT_SAVED = 0.05
# (change, metric, cells) per spec §7. A cell is (views, base_rows, batch,
# groups); the 10-view batch=1 cell is the supplementary ablation run.
ACCEPTANCE = [
    ("stmtcache", "maintain", [(10, 100000, 1, 1000), (200, 100000, 1, 1000)]),
    ("snapshot", "first_refresh", [(200, 100000, 1, 1000)]),
    ("progidx", "maintain", [(200, 100000, 1, 1000)]),
]


def load(name):
    with open(HERE / name, newline="") as f:
        return list(csv.DictReader(f))


def cell_key(r):
    return (int(r["views"]), int(r["base_rows"]), int(r["batch_size"]), int(r["group_cardinality"]))


def total(r):
    """A run's apply + maintain: the steady state's own median of each
    batch's sum when the CSV has that column (M1b Phase 6 spec §6), else the
    single batch's apply_ms + maintain_ms of a Phase 4 or Phase 5 CSV."""
    if r.get("apply_plus_maintain_ms"):
        return float(r["apply_plus_maintain_ms"])
    return float(r["apply_ms"]) + float(r["maintain_ms"])


def med_range(xs):
    return statistics.median(xs), min(xs), max(xs)


def fmt_cell(k):
    v, b, n, g = k
    return f"(views={v}, base_rows={b}, batch={n}, groups={g})"


def fmt_mr(m, unit="", digits=3):
    return f"{m[0]:,.{digits}f}{unit} [{m[1]:,.{digits}f}, {m[2]:,.{digits}f}]"


def overlap(a, b):
    """Whether two (median, min, max) ranges overlap."""
    return a[1] <= b[2] and b[1] <= a[2]


def used_pages(r, point):
    return int(r[f"{point}_pages"]) - int(r[f"{point}_free"])


def section(title):
    print()
    print(f"== {title} ==")


def by_cell(matrix):
    cells = collections.defaultdict(dict)
    for r in matrix:
        cells[cell_key(r)][r["engine"]] = r
    return cells


def speedups(cells):
    return {k: total(e["naive_recompute"]) / total(e["ivmlite"]) for k, e in cells.items()}


def by_repeat(confirm):
    reps = collections.defaultdict(lambda: collections.defaultdict(dict))
    for r in confirm:
        reps[cell_key(r)][int(r["repeat"])][r["engine"]] = r
    return reps


# ---------------------------------------------------------------------------
# Ablation
# ---------------------------------------------------------------------------


METRICS = {
    "total": total,
    "apply": lambda r: float(r["apply_ms"]),
    "maintain": lambda r: float(r["maintain_ms"]),
    "first_refresh": lambda r: float(r["first_refresh_ms"]),
    "bootstrap": lambda r: float(r["bootstrap_ms"]),
}


def ablation_stats(abl, engine):
    runs = collections.defaultdict(list)
    for r in abl:
        if r["engine"] == engine:
            runs[(r["label"], cell_key(r))].append(r)
    cells = sorted({k for _, k in runs})
    stats = {
        (b, k): {m: med_range([f(r) for r in runs[(b, k)]]) for m, f in METRICS.items()}
        for b in BUILDS
        for k in cells
    }
    return runs, cells, stats


def ablation(abl, builds, sources):
    section("Ablation builds (m1b-phase6-ablation-builds.csv)")
    for b in builds:
        print(f"  {b['label']}: {b['commit']} ({b['build']})")
    print(f"ablation CSVs: {', '.join(sources)}")
    labels = {r["label"] for r in abl}
    print(f"labels in the ablation CSVs match the build list: {labels == {b['label'] for b in builds}}")
    print(f"build order in this script matches the build list: {BUILDS == [b['label'] for b in builds]}")
    print(f"cells: {len({cell_key(r) for r in abl})}; builds: {len(builds)}; "
          f"repeats per (build, cell, engine): "
          f"{sorted(set(collections.Counter((r['label'], cell_key(r), r['engine']) for r in abl).values()))}")

    runs, cells, stats = ablation_stats(abl, "ivmlite")

    section("Ablation, ivmlite per build: median [min, max] over the repeats, ms")
    print("build | n | steady apply + maintain | steady maintain | maintain per view us | first refresh | first refresh per view us | apply | bootstrap")
    for k in cells:
        print(fmt_cell(k))
        for b in BUILDS:
            s = stats[(b, k)]
            print(
                f"  {b:9} n={len(runs[(b, k)])} | {fmt_mr(s['total'])} | {fmt_mr(s['maintain'])} | "
                f"{1000 * s['maintain'][0] / k[0]:,.1f} | {fmt_mr(s['first_refresh'])} | "
                f"{1000 * s['first_refresh'][0] / k[0]:,.1f} | {fmt_mr(s['apply'])} | {fmt_mr(s['bootstrap'])}"
            )

    section("Ablation, each change against the build before it (median ratio prev/next, >1 = faster)")
    print("change | cell | steady maintain (ranges overlap -> verdict) | first refresh (overlap) | "
          "steady apply + maintain (overlap) | bootstrap")
    for prev, nxt in zip(BUILDS, BUILDS[1:]):
        print(f"{nxt}: {CHANGES[nxt]}")
        for k in cells:
            p, n = stats[(prev, k)], stats[(nxt, k)]
            parts = []
            for m in ("maintain", "first_refresh", "total"):
                r = p[m][0] / n[m][0]
                ov = overlap(p[m], n[m])
                verdict = "INDISTINGUISHABLE" if ov else ("faster" if r > 1 else "SLOWER")
                parts.append(f"{r:.3f}x ({100 * (1 - 1 / r):+.1f}% saved, {verdict})")
            parts.append(f"{p['bootstrap'][0] / n['bootstrap'][0]:.3f}x")
            print(f"  {fmt_cell(k)} | " + " | ".join(parts))

    section("Ablation, before -> progidx, the whole phase (median ratio, >1 = faster)")
    for k in cells:
        p, n = stats[("before", k)], stats[("progidx", k)]
        print(
            f"  {fmt_cell(k)} | steady maintain {p['maintain'][0] / n['maintain'][0]:.2f}x "
            f"(overlap {overlap(p['maintain'], n['maintain'])}) | first refresh "
            f"{p['first_refresh'][0] / n['first_refresh'][0]:.2f}x | steady apply + maintain "
            f"{p['total'][0] / n['total'][0]:.2f}x | apply {p['apply'][0] / n['apply'][0]:.3f}x | "
            f"bootstrap {p['bootstrap'][0] / n['bootstrap'][0]:.3f}x"
        )

    section("Acceptance (Phase 6 spec §7): each change must save at least 5% in its target cells")
    for change, metric, targets in ACCEPTANCE:
        prev = BUILDS[BUILDS.index(change) - 1]
        for k in targets:
            if (prev, k) not in stats:
                print(f"  {change} {metric} {fmt_cell(k)}: NOT MEASURED")
                continue
            p, n = stats[(prev, k)][metric], stats[(change, k)][metric]
            saved = 1 - n[0] / p[0]
            ok = saved >= ACCEPT_SAVED
            print(
                f"  {change} ({CHANGES[change]}), {metric}, {fmt_cell(k)}: {fmt_mr(p)} -> {fmt_mr(n)} ms; "
                f"{100 * saved:.1f}% saved; ranges overlap {overlap(p, n)} -> {'MET' if ok else 'MISSED'}"
            )

    section("Ablation, apply share of steady apply + maintain (median over repeats)")
    for k in cells:
        parts = []
        for b in BUILDS:
            share = statistics.median(float(r["apply_ms"]) / total(r) for r in runs[(b, k)])
            parts.append(f"{b} {100 * share:.1f}%")
        print(f"  {fmt_cell(k)}: " + ", ".join(parts))

    section("Ablation, bootstrapped and maintained used pages (ivmlite; maintained = after the sixth batch)")
    for k in cells:
        parts = []
        for b in BUILDS:
            seen = {(used_pages(r, "bootstrapped"), used_pages(r, "maintained")) for r in runs[(b, k)]}
            parts.append(f"{b} {sorted(seen)}")
        print(f"  {fmt_cell(k)} (bootstrapped, maintained): " + "; ".join(parts))

    section("Ablation drift control: naive_recompute, which no build changes (medians per build, max/min)")
    _, ncells, nstats = ablation_stats(abl, "naive_recompute")
    spreads = []
    for k in ncells:
        parts = []
        for m in ("total", "first_refresh"):
            meds = [nstats[(b, k)][m][0] for b in BUILDS]
            spreads.append(max(meds) / min(meds))
            parts.append(f"{m}: " + ", ".join(f"{b} {x:,.3f}" for b, x in zip(BUILDS, meds))
                         + f"; max/min {max(meds) / min(meds):.3f}x")
        print(f"  {fmt_cell(k)}: " + " | ".join(parts))
    print(f"drift control spread over every cell and both metrics: max/min up to {max(spreads):.3f}x")


# ---------------------------------------------------------------------------
# The matrix and the confirmation
# ---------------------------------------------------------------------------


def exploration(cells, speedup):
    section("Exploration matrix (m1b-phase6.csv), speedup over naive_recompute, steady state")
    print(f"cells: {len(cells)}; speedup > 2: {sum(s > 2 for s in speedup.values())}; "
          f"speedup < 1: {sum(s < 1 for s in speedup.values())}")
    for base in sorted({k[1] for k in cells}):
        ks = [k for k in cells if k[1] == base]
        ss = [speedup[k] for k in ks]
        print(
            f"base_rows={base}: {len(ks)} cells, >2: {sum(s > 2 for s in ss)}, "
            f"<1: {sum(s < 1 for s in ss)}, min {min(ss):.3f}x, max {max(ss):.3f}x"
        )
    print("cells below 2x:")
    for k in sorted(k for k in cells if speedup[k] < 2):
        print(f"  {fmt_cell(k)}: {speedup[k]:.3f}x")
    beats = sorted(k for k, e in cells.items() if total(e["ivmlite"]) < total(e["hand_written_trigger"]))
    print(f"exploration cells where ivmlite beats hand_written_trigger: {len(beats)}")
    for k in beats:
        e = cells[k]
        print(f"  {fmt_cell(k)}: ivmlite/trigger {total(e['ivmlite']) / total(e['hand_written_trigger']):.3f}x")


def confirmation(confirm, cells, speedup):
    section("Confirmation (m1b-phase6-confirm.csv)")
    reps = by_repeat(confirm)
    selected = {
        k
        for k, s in speedup.items()
        if NEAR_1X[0] <= s <= NEAR_1X[1] or NEAR_2X[0] <= s <= NEAR_2X[1]
    }
    beats = {k for k, e in cells.items() if total(e["ivmlite"]) < total(e["hand_written_trigger"])}
    print(f"confirmed cells: {len(reps)}; repeats per cell: {sorted({len(v) for v in reps.values()})}")
    print(f"rule-selected cells: {len(selected | beats)} ({len(selected)} by speedup, "
          f"{len(beats - selected)} more by beating the trigger); matches the CSV: {(selected | beats) == set(reps)}")
    print("cell | exploration speedup | confirmed speedup median [min, max] | ivmlite/trigger median [min, max]")
    for k in sorted(reps):
        rs = reps[k]
        sp = [total(rs[i]["naive_recompute"]) / total(rs[i]["ivmlite"]) for i in sorted(rs)]
        tr = [total(rs[i]["ivmlite"]) / total(rs[i]["hand_written_trigger"]) for i in sorted(rs)]
        m = med_range(sp)
        straddles = " STRADDLES 1x" if m[1] < 1 < m[2] else ""
        straddles += " STRADDLES 2x" if m[1] < 2 < m[2] else ""
        print(f"  {fmt_cell(k)} | {speedup[k]:.3f}x | {fmt_mr(m, 'x')}{straddles} | {fmt_mr(med_range(tr), 'x', 2)}")
    wins = [
        (k, i)
        for k, rs in reps.items()
        for i, e in rs.items()
        if total(e["ivmlite"]) < total(e["hand_written_trigger"])
    ]
    print(f"confirmation repeats where ivmlite beats hand_written_trigger: {len(wins)}")
    return reps


def bars(cells, reps):
    section("§10.2 bars against hand-written triggers, steady state")
    if reps:
        print("Expected: ivmlite / hand_written_trigger steady apply + maintain, confirmed cells, sorted by median:")
        rows = []
        for k, rs in reps.items():
            tr = [total(rs[i]["ivmlite"]) / total(rs[i]["hand_written_trigger"]) for i in sorted(rs)]
            rows.append((med_range(tr), k))
        for m, k in sorted(rows):
            print(f"  {fmt_cell(k)}: {fmt_mr(m, 'x', 2)}")
        meds = [m[0] for m, _ in rows]
        print(f"confirmed-cell medians: min {min(meds):.2f}x, median {statistics.median(meds):.2f}x, "
              f"max {max(meds):.2f}x; within 2x: {sum(m <= 2 for m in meds)}, within 1.5x: "
              f"{sum(m <= 1.5 for m in meds)}, below 1x: {sum(m < 1 for m in meds)} of {len(meds)}")

    ratio = {k: total(e["ivmlite"]) / total(e["hand_written_trigger"]) for k, e in cells.items()}
    print("Exploration (one run per cell), ivmlite / trigger steady apply + maintain by batch size:")
    for batch in sorted({k[2] for k in cells}):
        rs = [ratio[k] for k in cells if k[2] == batch]
        print(f"  batch={batch}: {len(rs)} cells, median {statistics.median(rs):.2f}x, "
              f"min {min(rs):.2f}x, max {max(rs):.2f}x, within 2x: {sum(r <= 2 for r in rs)}, "
              f"within 1.5x: {sum(r <= 1.5 for r in rs)}, below 1x: {sum(r < 1 for r in rs)}")
    for batch in (1, 1000):
        print(f"Exploration, ivmlite / trigger by views at batch={batch} (median [min, max]):")
        for v in sorted({k[0] for k in cells}):
            rs = [ratio[k] for k in cells if k[0] == v and k[2] == batch]
            print(f"  views={v}: {len(rs)} cells, {fmt_mr(med_range(rs), 'x', 2)}")
    print("Exploration cells within 2x of the trigger:")
    for k in sorted(k for k in cells if ratio[k] <= 2):
        print(f"  {fmt_cell(k)}: {ratio[k]:.2f}x")
    print("Exploration, maintain share of ivmlite's steady apply + maintain, by batch size (median [min, max]):")
    for batch in sorted({k[2] for k in cells}):
        sh = [float(cells[k]["ivmlite"]["maintain_ms"]) / total(cells[k]["ivmlite"]) for k in cells if k[2] == batch]
        print(f"  batch={batch}: {100 * statistics.median(sh):.1f}% [{100 * min(sh):.1f}, {100 * max(sh):.1f}]")
    print("Exploration, ivmlite steady refresh per view (maintain_ms / views, us), batch=1 (median [min, max]):")
    for v in sorted({k[0] for k in cells}):
        xs = [1000 * float(cells[k]["ivmlite"]["maintain_ms"]) / v for k in cells if k[0] == v and k[2] == 1]
        print(f"  views={v}: {len(xs)} cells, {fmt_mr(med_range(xs), ' us', 1)}")
    lower = sum(1 for e in cells.values() if float(e["ivmlite"]["apply_ms"]) < float(e["hand_written_trigger"]["apply_ms"]))
    print(f"exploration cells where ivmlite's apply_ms alone is below the trigger's: {lower} of {len(cells)}")

    section("First refresh beside the steady state (ivmlite, exploration)")
    print("ivmlite first_refresh_ms / steady maintain_ms, by batch size (median [min, max]):")
    for batch in sorted({k[2] for k in cells}):
        xs = [float(cells[k]["ivmlite"]["first_refresh_ms"]) / float(cells[k]["ivmlite"]["maintain_ms"])
              for k in cells if k[2] == batch]
        print(f"  batch={batch}: {fmt_mr(med_range(xs), 'x', 2)}")
    print("ivmlite (apply + first refresh) / trigger steady apply + maintain, by batch size (median [min, max]):")
    for batch in sorted({k[2] for k in cells}):
        xs = [(float(cells[k]["ivmlite"]["apply_ms"]) + float(cells[k]["ivmlite"]["first_refresh_ms"]))
              / total(cells[k]["hand_written_trigger"]) for k in cells if k[2] == batch]
        print(f"  batch={batch}: {fmt_mr(med_range(xs), 'x', 2)}")


# ---------------------------------------------------------------------------
# Phase 5 -> Phase 6
# ---------------------------------------------------------------------------


def phase_change(p5, p6, sp5, sp6):
    section("Phase 5 -> Phase 6 per cell (m1b-phase5.csv vs m1b-phase6.csv, one run each)")
    print("CAVEAT: Phase 5 timed one batch, the first refresh after CREATE; Phase 6's columns are "
          "steady-state medians over five batches. 'P6 first' is Phase 6's apply + first_refresh_ms, "
          "the nearest Phase 6 analogue of Phase 5's single batch.")
    common = sorted(set(p5) & set(p6))
    print(f"cells in both matrices: {len(common)} (Phase 5 {len(p5)}, Phase 6 {len(p6)})")
    print("cell | ivmlite P5 ms | P6 steady ms (P5/P6) | P6 first ms (P5/P6 first) | speedup P5 -> P6 | ivmlite/trigger P5 -> P6")
    steady, first = {}, {}
    for k in common:
        a, b = p5[k]["ivmlite"], p6[k]["ivmlite"]
        f6 = float(b["apply_ms"]) + float(b["first_refresh_ms"])
        steady[k] = total(a) / total(b)
        first[k] = total(a) / f6
        t5 = total(a) / total(p5[k]["hand_written_trigger"])
        t6 = total(b) / total(p6[k]["hand_written_trigger"])
        print(
            f"  {fmt_cell(k)} | {total(a):,.3f} | {total(b):,.3f} ({steady[k]:.2f}x) | {f6:,.3f} ({first[k]:.2f}x) | "
            f"{sp5[k]:.3f}x -> {sp6[k]:.3f}x | {t5:.2f}x -> {t6:.2f}x"
        )
    for name, ratio in (("steady", steady), ("first", first)):
        rs = list(ratio.values())
        print(f"ivmlite P5/P6 {name} over {len(rs)} cells: median {statistics.median(rs):.2f}x, "
              f"min {min(rs):.2f}x, max {max(rs):.2f}x; below 1x: {sum(r < 1 for r in rs)}")
    print("by views and batch: median ivmlite P5/P6 steady (first)")
    for v in sorted({k[0] for k in common}):
        parts = []
        for batch in sorted({k[2] for k in common}):
            ks = [k for k in common if k[0] == v and k[2] == batch]
            if ks:
                parts.append(f"batch={batch} {statistics.median(steady[k] for k in ks):.2f}x "
                             f"({statistics.median(first[k] for k in ks):.2f}x) n={len(ks)}")
        print(f"  views={v}: " + ", ".join(parts))
    print("cells where Phase 6's steady ivmlite is slower than Phase 5's single batch (P5/P6 < 1):")
    for k in sorted(k for k in common if steady[k] < 1):
        print(f"  {fmt_cell(k)}: {steady[k]:.3f}x")
    print("drift control, the unchanged engines' apply + maintain P5/P6 (median [min, max]); "
          "their protocol also changed from one batch to five:")
    for eng in ("no_maintenance", "hand_written_trigger", "naive_recompute"):
        xs = [total(p5[k][eng]) / total(p6[k][eng]) for k in common if total(p6[k][eng]) > 0 and total(p5[k][eng]) > 0]
        print(f"  {eng}: {fmt_mr(med_range(xs), 'x')} over {len(xs)} cells")
    print("naive_recompute P5 single batch / P6 first_refresh_ms (the same one recompute, median [min, max]): "
          + fmt_mr(med_range([float(p5[k]["naive_recompute"]["maintain_ms"]) / float(p6[k]["naive_recompute"]["first_refresh_ms"])
                              for k in common]), "x"))
    print("speedup surface counts, P5 -> P6:")
    for base in sorted({k[1] for k in common}):
        ks = [k for k in common if k[1] == base]
        print(
            f"  base_rows={base}: >2 {sum(sp5[k] > 2 for k in ks)} -> {sum(sp6[k] > 2 for k in ks)}, "
            f"<1 {sum(sp5[k] < 1 for k in ks)} -> {sum(sp6[k] < 1 for k in ks)}"
        )
    print("bootstrap ms, ivmlite P5/P6 (median [min, max]): "
          + fmt_mr(med_range([float(p5[k]['ivmlite']['bootstrap_ms']) / float(p6[k]['ivmlite']['bootstrap_ms']) for k in common]), "x"))


def confirmed_change(c5, c6):
    section("Phase 5 -> Phase 6, cells confirmed in both phases (five repeats each; P5 = first refresh, P6 = steady)")
    r5, r6 = by_repeat(c5), by_repeat(c6)
    common = sorted(set(r5) & set(r6))
    print(f"cells confirmed in both: {len(common)} (Phase 5 {len(r5)}, Phase 6 {len(r6)})")
    print("cell | ivmlite apply + maintain ms P5 | P6 | P5/P6 medians | ivmlite/trigger P5 -> P6")
    for k in common:
        a = med_range([total(r5[k][i]["ivmlite"]) for i in r5[k]])
        b = med_range([total(r6[k][i]["ivmlite"]) for i in r6[k]])
        t5 = med_range([total(r5[k][i]["ivmlite"]) / total(r5[k][i]["hand_written_trigger"]) for i in r5[k]])
        t6 = med_range([total(r6[k][i]["ivmlite"]) / total(r6[k][i]["hand_written_trigger"]) for i in r6[k]])
        print(f"  {fmt_cell(k)} | {fmt_mr(a)} | {fmt_mr(b)} | {a[0] / b[0]:.2f}x | "
              f"{fmt_mr(t5, 'x', 2)} -> {fmt_mr(t6, 'x', 2)}")


# ---------------------------------------------------------------------------
# Demos
# ---------------------------------------------------------------------------


def demos(rows, builds):
    section("Demo builds (m1b-phase6-demos-builds.csv)")
    for b in builds:
        print(f"  {b['label']}: {b['commit']} (harness {b['harness_commit']}, uncommitted harness changes: "
              f"{b['harness_dirty']}, SQLite {b['sqlite']})")
    print(f"labels match: {({r['label'] for r in rows}) == {b['label'] for b in builds}}")
    d = {(r["label"], r["case"], r["mode"]): r for r in rows}
    cases = list(dict.fromkeys(r["case"] for r in rows))

    def mr(r, metric):
        stem = metric[: -len("_ms")]
        return float(r[metric]), float(r[f"{stem}_min_ms"]), float(r[f"{stem}_max_ms"])

    section("Demos, per case, build and mode: median [min, max] ms over the repeats")
    for case in cases:
        r0 = d[("before", case, "ivmlite")]
        print(f"{case}: {r0['base_rows']} rows, batch {r0['batch_size']}, {r0['repeats']} repeats")
        modes = [r["mode"] for r in rows if r["label"] == "before" and r["case"] == case]
        for label in DEMO_BUILDS:
            for mode in modes:
                r = d[(label, case, mode)]
                print(
                    f"  {label:6} {mode:20} maintain {fmt_mr(mr(r, 'maintain_ms'))} | "
                    f"apply {fmt_mr(mr(r, 'apply_ms'))} | end_to_end {fmt_mr(mr(r, 'end_to_end_ms'))} | "
                    f"first_refresh {fmt_mr(mr(r, 'first_refresh_ms'))} | bootstrap {fmt_mr(mr(r, 'bootstrap_ms'))} | "
                    f"kib {float(r['database_kib']):,.0f}"
                )

    section("Demos, ivmlite before -> after (median ratio before/after, >1 = faster)")
    print("case | steady-state refresh (maintain) | end to end | apply | first refresh | bootstrap | ranges overlap (maintain, e2e)")
    for case in cases:
        b, a = d[("before", case, "ivmlite")], d[("after", case, "ivmlite")]
        parts = []
        for metric in ("maintain_ms", "end_to_end_ms", "apply_ms", "first_refresh_ms", "bootstrap_ms"):
            parts.append(f"{mr(b, metric)[0]:,.3f} -> {mr(a, metric)[0]:,.3f} ({mr(b, metric)[0] / mr(a, metric)[0]:.2f}x)")
        ov = (overlap(mr(b, "maintain_ms"), mr(a, "maintain_ms")), overlap(mr(b, "end_to_end_ms"), mr(a, "end_to_end_ms")))
        print(f"  {case} | " + " | ".join(parts) + f" | {ov}")

    section("Demos, speedup of ivmlite against indexed_recompute (indexed / ivmlite, medians)")
    print("case | build | end to end | apply + maintain | maintain alone")
    for case in cases:
        for label in DEMO_BUILDS:
            iv, ix = d[(label, case, "ivmlite")], d[(label, case, "indexed_recompute")]
            print(
                f"  {case} | {label} | {float(ix['end_to_end_ms']) / float(iv['end_to_end_ms']):.2f}x | "
                f"{float(ix['apply_plus_maintain_ms']) / float(iv['apply_plus_maintain_ms']):.2f}x | "
                f"{float(ix['maintain_ms']) / float(iv['maintain_ms']):.2f}x"
            )

    section("Demos, ivmlite against handwritten_trigger where the demo has one (ivmlite / trigger, medians)")
    for case in cases:
        if ("before", case, "handwritten_trigger") not in d:
            continue
        for label in DEMO_BUILDS:
            iv, tr = d[(label, case, "ivmlite")], d[(label, case, "handwritten_trigger")]
            print(
                f"  {case} | {label} | end to end {float(iv['end_to_end_ms']) / float(tr['end_to_end_ms']):.2f}x | "
                f"apply + maintain {float(iv['apply_plus_maintain_ms']) / float(tr['apply_plus_maintain_ms']):.2f}x | "
                f"ivmlite apply {float(iv['apply_ms']):,.3f} ms + maintain {float(iv['maintain_ms']):,.3f} ms "
                f"against the trigger's apply {float(tr['apply_ms']):,.3f} ms"
            )

    section("Demos, drift control: the modes no build changes, after/before median end to end")
    spreads = []
    for case in cases:
        parts = []
        for mode in ("no_maintenance", "unindexed_recompute", "indexed_recompute", "handwritten_trigger"):
            if ("before", case, mode) in d:
                b, a = float(d[("before", case, mode)]["end_to_end_ms"]), float(d[("after", case, mode)]["end_to_end_ms"])
                spreads.append(a / b)
                parts.append(f"{mode} {a / b:.3f}x")
        print(f"  {case}: " + ", ".join(parts))
    print(f"demo drift control range: {min(spreads):.3f}..{max(spreads):.3f}x")


def main():
    sources = ["m1b-phase6-ablation.csv", "m1b-phase6-ablation-views10-batch1.csv"]
    abl = [r for s in sources for r in load(s)]
    ablation(abl, load("m1b-phase6-ablation-builds.csv"), sources)
    p6 = by_cell(load("m1b-phase6.csv"))
    sp6 = speedups(p6)
    exploration(p6, sp6)
    reps = confirmation(load("m1b-phase6-confirm.csv"), p6, sp6)
    bars(p6, reps)
    p5 = by_cell(load("m1b-phase5.csv"))
    phase_change(p5, p6, speedups(p5), sp6)
    confirmed_change(load("m1b-phase5-confirm.csv"), load("m1b-phase6-confirm.csv"))
    demos(load("m1b-phase6-demos.csv"), load("m1b-phase6-demos-builds.csv"))


if __name__ == "__main__":
    main()
