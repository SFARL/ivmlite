#!/usr/bin/env python3
"""Derive every number in docs/bench/README.md's "M1b Phase 5" section from
the committed CSVs in this directory.

The README's Phase 5 tables and the figures quoted in its prose are copied
from this script's output, so they can be re-derived after any change to the
data. It reads only the CSVs next to it (Phase 5's, plus Phase 4's matrix and
confirmation for the per-cell change) and uses only the standard library.

Usage: python3 docs/bench/phase5_tables.py
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
# the build before it (Phase 5 spec §7).
BUILDS = ["before", "schemaver", "setapply", "stage", "profiled"]
CHANGES = {
    "schemaver": "§3 schema-version-gated checks",
    "setapply": "§4 set-based apply",
    "stage": "§5 bootstrap empties its stage",
    "profiled": "§6 multi-row staging",
}
DEMO_BUILDS = ["before", "after"]

OPS = ["insert", "delete", "update", "replace_rowid", "replace_unique", "replace_two"]
MODES = ["false", "true"]


def load(name):
    with open(HERE / name, newline="") as f:
        return list(csv.DictReader(f))


def cell_key(r):
    return (int(r["views"]), int(r["base_rows"]), int(r["batch_size"]), int(r["group_cardinality"]))


def total(r):
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


def ablation(abl, builds):
    section("Ablation builds (m1b-phase5-ablation-builds.csv)")
    for b in builds:
        print(f"  {b['label']}: {b['commit']} ({b['build']})")
    labels = {r["label"] for r in abl}
    print(f"labels in the ablation CSV match the build list: {labels == {b['label'] for b in builds}}")
    print(f"build order in this script matches the build list: {BUILDS == [b['label'] for b in builds]}")

    runs = collections.defaultdict(list)
    for r in abl:
        if r["engine"] == "ivmlite":
            runs[(r["label"], cell_key(r))].append(r)
    cells = sorted({k for _, k in runs})

    section("Ablation, ivmlite apply + maintain per build (m1b-phase5-ablation.csv)")
    print("build | n | apply + maintain ms | apply ms | maintain ms | bootstrap ms")
    stats = {}
    for k in cells:
        print(fmt_cell(k))
        for b in BUILDS:
            rs = runs[(b, k)]
            tot = med_range([total(r) for r in rs])
            app = med_range([float(r["apply_ms"]) for r in rs])
            mnt = med_range([float(r["maintain_ms"]) for r in rs])
            boot = med_range([float(r["bootstrap_ms"]) for r in rs])
            stats[(b, k)] = {"total": tot, "apply": app, "maintain": mnt, "bootstrap": boot}
            print(
                f"  {b:9} n={len(rs)} | {fmt_mr(tot)} | {fmt_mr(app)} | {fmt_mr(mnt)} | {fmt_mr(boot)}"
            )

    section("Ablation, each change against the build before it (median ratio, >1 = faster)")
    print("change | cell | apply + maintain prev/next | ranges overlap | maintain prev/next | bootstrap prev/next")
    for prev, nxt in zip(BUILDS, BUILDS[1:]):
        print(f"{nxt}: {CHANGES[nxt]}")
        for k in cells:
            p, n = stats[(prev, k)], stats[(nxt, k)]
            tr = p["total"][0] / n["total"][0]
            mr = p["maintain"][0] / n["maintain"][0]
            br = p["bootstrap"][0] / n["bootstrap"][0]
            ov = overlap(p["total"], n["total"])
            verdict = "INDISTINGUISHABLE" if ov else ("faster" if tr > 1 else "SLOWER")
            print(
                f"  {fmt_cell(k)} | {tr:.3f}x ({100 * (1 - 1 / tr):+.1f}% time saved) | {ov} -> {verdict} | "
                f"{mr:.3f}x | {br:.3f}x"
            )
    print("before -> profiled, the whole phase:")
    for k in cells:
        p, n = stats[("before", k)], stats[("profiled", k)]
        print(
            f"  {fmt_cell(k)} | apply + maintain {p['total'][0] / n['total'][0]:.2f}x | "
            f"maintain {p['maintain'][0] / n['maintain'][0]:.2f}x | apply {p['apply'][0] / n['apply'][0]:.3f}x | "
            f"bootstrap {p['bootstrap'][0] / n['bootstrap'][0]:.3f}x | ranges overlap: {overlap(p['total'], n['total'])} | "
            f"maintain per view {p['maintain'][0] / k[0]:.3f} -> {n['maintain'][0] / k[0]:.3f} ms"
        )

    section("Ablation, apply share of apply + maintain (median over repeats)")
    for k in cells:
        parts = []
        for b in BUILDS:
            share = statistics.median(float(r["apply_ms"]) / total(r) for r in runs[(b, k)])
            parts.append(f"{b} {100 * share:.1f}%")
        print(f"  {fmt_cell(k)}: " + ", ".join(parts))

    section("Ablation, bootstrapped and maintained used pages (ivmlite)")
    for k in cells:
        parts = []
        for b in BUILDS:
            seen = {(used_pages(r, "bootstrapped"), used_pages(r, "maintained")) for r in runs[(b, k)]}
            parts.append(f"{b} {sorted(seen)}")
        print(f"  {fmt_cell(k)} (bootstrapped, maintained): " + "; ".join(parts))

    section("Ablation, naive_recompute apply + maintain per build (a drift control: no build changes it)")
    nruns = collections.defaultdict(list)
    for r in abl:
        if r["engine"] == "naive_recompute":
            nruns[(r["label"], cell_key(r))].append(total(r))
    for k in cells:
        meds = [statistics.median(nruns[(b, k)]) for b in BUILDS]
        print(
            f"  {fmt_cell(k)}: medians " + ", ".join(f"{b} {m:,.3f}" for b, m in zip(BUILDS, meds))
            + f"; max/min {max(meds) / min(meds):.3f}x"
        )


def ablation_write_amp(aw):
    section("Ablation write cost, us per written row (m1b-phase5-ablation-write-amp.csv, one run each)")
    us = {(r["label"], int(r["views"]), r["op"], r["recursive_triggers"]): float(r["us_per_row"]) for r in aw}
    views = sorted({k[1] for k in us})
    print("views mode op | " + " | ".join(BUILDS) + " | profiled/before")
    ratios = collections.defaultdict(list)
    for v in views:
        for m in MODES:
            for op in OPS:
                vals = [us[(b, v, op, m)] for b in BUILDS]
                ratios[v].append(vals[-1] / vals[0])
                print(f"  {v:3} {m:5} {op:14} | " + " | ".join(f"{x:7.3f}" for x in vals) + f" | {vals[-1] / vals[0]:.3f}x")
    for v in views:
        print(f"views={v}: profiled/before us per row across ops and modes {min(ratios[v]):.3f}..{max(ratios[v]):.3f}x")
        latch = [us[("profiled", v, op, m)] for op in OPS if op != "delete" for m in MODES]
        delete = [us[("profiled", v, "delete", m)] for m in MODES]
        print(f"views={v}, profiled: insert/update/REPLACE {min(latch):.3f}..{max(latch):.3f} us per row; "
              f"delete {min(delete):.3f}..{max(delete):.3f}")


# ---------------------------------------------------------------------------
# The matrix and the confirmation
# ---------------------------------------------------------------------------


def exploration(cells, speedup):
    section("Exploration matrix (m1b-phase5.csv)")
    print(f"cells: {len(cells)}; speedup > 2: {sum(s > 2 for s in speedup.values())}; "
          f"speedup < 1: {sum(s < 1 for s in speedup.values())}")
    for base in sorted({k[1] for k in cells}):
        ks = [k for k in cells if k[1] == base]
        ss = [speedup[k] for k in ks]
        print(
            f"base_rows={base}: {len(ks)} cells, >2: {sum(s > 2 for s in ss)}, "
            f"<1: {sum(s < 1 for s in ss)}, min {min(ss):.3f}x, max {max(ss):.3f}x"
        )
    print("cells below 1x:")
    for k in sorted(k for k in cells if speedup[k] < 1):
        print(f"  {fmt_cell(k)}: {speedup[k]:.3f}x")
    beats = sorted(k for k, e in cells.items() if total(e["ivmlite"]) < total(e["hand_written_trigger"]))
    print(f"exploration cells where ivmlite beats hand_written_trigger: {len(beats)}")
    for k in beats:
        e = cells[k]
        print(f"  {fmt_cell(k)}: ivmlite/trigger {total(e['ivmlite']) / total(e['hand_written_trigger']):.3f}x")


def confirmation(confirm, cells, speedup):
    section("Confirmation (m1b-phase5-confirm.csv)")
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
    section("§10.2 bars against hand-written triggers")
    print("Expected: ivmlite / hand_written_trigger apply + maintain, confirmed cells, sorted by median:")
    rows = []
    for k, rs in reps.items():
        tr = [total(rs[i]["ivmlite"]) / total(rs[i]["hand_written_trigger"]) for i in sorted(rs)]
        rows.append((med_range(tr), k))
    for m, k in sorted(rows):
        print(f"  {fmt_cell(k)}: {fmt_mr(m, 'x', 2)}")
    meds = [m[0] for m, _ in rows]
    print(f"confirmed-cell medians: min {min(meds):.2f}x, median {statistics.median(meds):.2f}x, max {max(meds):.2f}x; "
          f"within 2x: {sum(m <= 2 for m in meds)}, within 1.5x: {sum(m <= 1.5 for m in meds)}, "
          f"below 1x: {sum(m < 1 for m in meds)} of {len(meds)}")

    print("Exploration (one run per cell), ivmlite / trigger apply + maintain by batch size:")
    for batch in sorted({k[2] for k in cells}):
        ks = [k for k in cells if k[2] == batch]
        rs = [total(cells[k]["ivmlite"]) / total(cells[k]["hand_written_trigger"]) for k in ks]
        print(f"  batch={batch}: {len(ks)} cells, median {statistics.median(rs):.2f}x, "
              f"min {min(rs):.2f}x, max {max(rs):.2f}x, within 2x: {sum(r <= 2 for r in rs)}, below 1x: {sum(r < 1 for r in rs)}")
    print("Exploration, ivmlite / trigger by views at batch=1000:")
    for v in sorted({k[0] for k in cells}):
        ks = [k for k in cells if k[0] == v and k[2] == 1000]
        rs = [total(cells[k]["ivmlite"]) / total(cells[k]["hand_written_trigger"]) for k in ks]
        print(f"  views={v}: {len(ks)} cells, median {statistics.median(rs):.2f}x, min {min(rs):.2f}x, max {max(rs):.2f}x")
    print("Exploration, maintain share of ivmlite's apply + maintain, by batch size (median):")
    for batch in sorted({k[2] for k in cells}):
        ks = [k for k in cells if k[2] == batch]
        sh = [float(cells[k]["ivmlite"]["maintain_ms"]) / total(cells[k]["ivmlite"]) for k in ks]
        print(f"  batch={batch}: {100 * statistics.median(sh):.1f}% [{100 * min(sh):.1f}, {100 * max(sh):.1f}]")
    lower = sum(1 for e in cells.values() if float(e["ivmlite"]["apply_ms"]) < float(e["hand_written_trigger"]["apply_ms"]))
    print(f"exploration cells where ivmlite's apply_ms alone is below the trigger's: {lower} of {len(cells)}")


# ---------------------------------------------------------------------------
# Phase 4 -> Phase 5
# ---------------------------------------------------------------------------


def phase_change(p4, p5, sp4, sp5):
    section("Phase 4 -> Phase 5 per cell (m1b-phase4.csv vs m1b-phase5.csv, one run each)")
    common = sorted(set(p4) & set(p5))
    print(f"cells in both matrices: {len(common)} (Phase 4 {len(p4)}, Phase 5 {len(p5)})")
    print("cell | ivmlite apply + maintain P4 -> P5 ms (P4/P5) | maintain P4/P5 | apply P4/P5 | speedup P4 -> P5 | ivmlite/trigger P4 -> P5")
    ratio = {}
    for k in common:
        a, b = p4[k]["ivmlite"], p5[k]["ivmlite"]
        ratio[k] = total(a) / total(b)
        mr = float(a["maintain_ms"]) / float(b["maintain_ms"])
        ar = float(a["apply_ms"]) / float(b["apply_ms"]) if float(b["apply_ms"]) > 0 else float("nan")
        t4 = total(a) / total(p4[k]["hand_written_trigger"])
        t5 = total(b) / total(p5[k]["hand_written_trigger"])
        print(
            f"  {fmt_cell(k)} | {total(a):,.3f} -> {total(b):,.3f} ({ratio[k]:.2f}x) | {mr:.2f}x | {ar:.2f}x | "
            f"{sp4[k]:.3f}x -> {sp5[k]:.3f}x | {t4:.2f}x -> {t5:.2f}x"
        )
    rs = list(ratio.values())
    print(f"ivmlite P4/P5 over {len(rs)} cells: median {statistics.median(rs):.2f}x, min {min(rs):.2f}x, max {max(rs):.2f}x")
    for lo, hi, name in ((0, 0.9, "slower by more than 10% (P4/P5 < 0.9)"), (0.9, 1.1, "within 10%"), (1.1, float("inf"), "faster by more than 10%")):
        ks = [k for k in common if lo <= ratio[k] < hi]
        print(f"  {name}: {len(ks)}")
    print("cells where Phase 5's ivmlite is slower than Phase 4's (P4/P5 < 1):")
    for k in sorted(k for k in common if ratio[k] < 1):
        print(f"  {fmt_cell(k)}: {ratio[k]:.3f}x (P4 {total(p4[k]['ivmlite']):,.3f} ms, P5 {total(p5[k]['ivmlite']):,.3f} ms)")
    print("by base_rows and batch: median ivmlite P4/P5")
    for base in sorted({k[1] for k in common}):
        parts = []
        for batch in sorted({k[2] for k in common}):
            xs = [ratio[k] for k in common if k[1] == base and k[2] == batch]
            if xs:
                parts.append(f"batch={batch} {statistics.median(xs):.2f}x (n={len(xs)})")
        print(f"  base_rows={base}: " + ", ".join(parts))
    print("by views: median ivmlite P4/P5")
    for v in sorted({k[0] for k in common}):
        xs = [ratio[k] for k in common if k[0] == v]
        print(f"  views={v}: {statistics.median(xs):.2f}x [{min(xs):.2f}, {max(xs):.2f}] (n={len(xs)})")
    print("drift control, the unchanged engines' apply + maintain P4/P5 (median [min, max]):")
    for eng in ("no_maintenance", "hand_written_trigger", "naive_recompute"):
        xs = [total(p4[k][eng]) / total(p5[k][eng]) for k in common if total(p5[k][eng]) > 0 and total(p4[k][eng]) > 0]
        print(f"  {eng}: {fmt_mr(med_range(xs), 'x')} over {len(xs)} cells")
    print("speedup surface counts, P4 -> P5:")
    for base in sorted({k[1] for k in common}):
        ks = [k for k in common if k[1] == base]
        print(
            f"  base_rows={base}: >2 {sum(sp4[k] > 2 for k in ks)} -> {sum(sp5[k] > 2 for k in ks)}, "
            f"<1 {sum(sp4[k] < 1 for k in ks)} -> {sum(sp5[k] < 1 for k in ks)}"
        )
    for name, cs in (("P4", p4), ("P5", p5)):
        lower = sum(1 for k in common if float(cs[k]["ivmlite"]["apply_ms"]) < float(cs[k]["hand_written_trigger"]["apply_ms"]))
        print(f"{name}: cells where ivmlite's apply_ms alone is below the trigger's: {lower} of {len(common)}")
    print("bootstrap ms, ivmlite P4/P5 (median [min, max]): "
          + fmt_mr(med_range([float(p4[k]['ivmlite']['bootstrap_ms']) / float(p5[k]['ivmlite']['bootstrap_ms']) for k in common]), "x"))
    print("bootstrapped used pages, ivmlite P4/P5 (median [min, max]): "
          + fmt_mr(med_range([used_pages(p4[k]["ivmlite"], "bootstrapped") / used_pages(p5[k]["ivmlite"], "bootstrapped") for k in common]), "x"))


def confirmed_change(c4, c5):
    section("Phase 4 -> Phase 5, cells confirmed in both phases (five repeats each)")
    r4, r5 = by_repeat(c4), by_repeat(c5)
    common = sorted(set(r4) & set(r5))
    print(f"cells confirmed in both: {len(common)} (Phase 4 {len(r4)}, Phase 5 {len(r5)})")
    print("cell | ivmlite apply + maintain ms P4 | P5 | P4/P5 medians | ranges overlap | ivmlite/trigger P4 -> P5")
    for k in common:
        a = med_range([total(r4[k][i]["ivmlite"]) for i in r4[k]])
        b = med_range([total(r5[k][i]["ivmlite"]) for i in r5[k]])
        t4 = med_range([total(r4[k][i]["ivmlite"]) / total(r4[k][i]["hand_written_trigger"]) for i in r4[k]])
        t5 = med_range([total(r5[k][i]["ivmlite"]) / total(r5[k][i]["hand_written_trigger"]) for i in r5[k]])
        print(
            f"  {fmt_cell(k)} | {fmt_mr(a)} | {fmt_mr(b)} | {a[0] / b[0]:.2f}x | {overlap(a, b)} | "
            f"{fmt_mr(t4, 'x', 2)} -> {fmt_mr(t5, 'x', 2)}"
        )


# ---------------------------------------------------------------------------
# Demos
# ---------------------------------------------------------------------------


def demos(rows, builds):
    section("Demo builds (m1b-phase5-demos-builds.csv)")
    for b in builds:
        print(f"  {b['label']}: {b['commit']}")
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
                f"apply + maintain {float(iv['apply_plus_maintain_ms']) / float(tr['apply_plus_maintain_ms']):.2f}x"
            )

    section("Demos, drift control: the modes no build changes, after/before median end to end")
    for case in cases:
        parts = []
        for mode in ("no_maintenance", "unindexed_recompute", "indexed_recompute", "handwritten_trigger"):
            if ("before", case, mode) in d:
                b, a = float(d[("before", case, mode)]["end_to_end_ms"]), float(d[("after", case, mode)]["end_to_end_ms"])
                parts.append(f"{mode} {a / b:.3f}x")
        print(f"  {case}: " + ", ".join(parts))


def main():
    ablation(load("m1b-phase5-ablation.csv"), load("m1b-phase5-ablation-builds.csv"))
    ablation_write_amp(load("m1b-phase5-ablation-write-amp.csv"))
    p5 = by_cell(load("m1b-phase5.csv"))
    sp5 = speedups(p5)
    exploration(p5, sp5)
    reps = confirmation(load("m1b-phase5-confirm.csv"), p5, sp5)
    bars(p5, reps)
    p4 = by_cell(load("m1b-phase4.csv"))
    phase_change(p4, p5, speedups(p4), sp5)
    confirmed_change(load("m1b-phase4-confirm.csv"), load("m1b-phase5-confirm.csv"))
    demos(load("m1b-phase5-demos.csv"), load("m1b-phase5-demos-builds.csv"))


if __name__ == "__main__":
    main()
