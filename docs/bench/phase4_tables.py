#!/usr/bin/env python3
"""Derive every number in docs/bench/README.md's "M1b Phase 4" section from
the committed CSVs in this directory.

The README's Phase 4 tables and the figures quoted in its prose are copied
from this script's output, so they can be re-derived after any change to the
data. It reads only the CSVs next to it and uses only the standard library.

Usage: python3 docs/bench/phase4_tables.py
"""
import collections
import csv
import statistics
from pathlib import Path

HERE = Path(__file__).resolve().parent

# Spec §3.5's confirmation rule, restated here only to check that the
# committed confirmation CSV holds exactly the cells the rule selects.
NEAR_1X = (0.7, 1.4)
NEAR_2X = (1.4, 2.8)

OPS = ["insert", "delete", "update", "replace_rowid", "replace_unique", "replace_two"]
REPLACE_OPS = {"replace_rowid", "replace_unique", "replace_two"}
MODES = ["false", "true"]
BUILDS = ["before", "index", "latch"]


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


def used_pages(r, point):
    return int(r[f"{point}_pages"]) - int(r[f"{point}_free"])


def mib(pages, page_size):
    return pages * page_size / 2**20


def section(title):
    print()
    print(f"== {title} ==")


def exploration(matrix):
    section("Exploration matrix (m1b-phase4.csv)")
    cells = collections.defaultdict(dict)
    for r in matrix:
        cells[cell_key(r)][r["engine"]] = r
    speedup = {k: total(e["naive_recompute"]) / total(e["ivmlite"]) for k, e in cells.items()}
    print(f"cells: {len(cells)}; speedup > 2: {sum(s > 2 for s in speedup.values())}")
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
    return cells, speedup


def confirmation(confirm, speedup):
    section("Confirmation (m1b-phase4-confirm.csv)")
    reps = collections.defaultdict(lambda: collections.defaultdict(dict))
    for r in confirm:
        reps[cell_key(r)][int(r["repeat"])][r["engine"]] = r
    selected = {
        k
        for k, s in speedup.items()
        if NEAR_1X[0] <= s <= NEAR_1X[1] or NEAR_2X[0] <= s <= NEAR_2X[1]
    }
    print(f"confirmed cells: {len(reps)}; repeats per cell: {sorted({len(v) for v in reps.values()})}")
    print(f"rule-selected cells (beats-trigger clause adds none): {len(selected)}; "
          f"matches the CSV: {selected == set(reps)}")
    print("cell | exploration speedup | confirmed speedup median [min, max] | ivmlite/trigger median [min, max]")
    for k in sorted(reps):
        rs = reps[k]
        sp = [total(rs[i]["naive_recompute"]) / total(rs[i]["ivmlite"]) for i in sorted(rs)]
        tr = [total(rs[i]["ivmlite"]) / total(rs[i]["hand_written_trigger"]) for i in sorted(rs)]
        m, lo, hi = med_range(sp)
        tm, tlo, thi = med_range(tr)
        straddles = " STRADDLES 1x" if lo < 1 < hi else ""
        print(
            f"  {fmt_cell(k)} | {speedup[k]:.3f}x | {m:.3f}x [{lo:.3f}, {hi:.3f}]{straddles} | "
            f"{tm:.2f}x [{tlo:.2f}, {thi:.2f}]"
        )
    beats = [
        (k, i)
        for k, rs in reps.items()
        for i, e in rs.items()
        if total(e["ivmlite"]) < total(e["hand_written_trigger"])
    ]
    print(f"confirmation repeats where ivmlite beats hand_written_trigger: {len(beats)}")


def ablation(abl, builds):
    section("Ablation builds (m1b-phase4-ablation-builds.csv)")
    for b in builds:
        print(f"  {b['label']}: {b['commit']} ({b['build']})")
    labels = {r["label"] for r in abl}
    print(f"labels in the ablation CSV match the build list: {labels == {b['label'] for b in builds}}")
    section("Ablation, apply + maintain (m1b-phase4-ablation.csv)")
    runs = collections.defaultdict(list)
    for r in abl:
        if r["engine"] == "ivmlite":
            runs[(r["label"], cell_key(r))].append(r)
    cells = sorted({k for _, k in runs})
    for k in cells:
        print(fmt_cell(k))
        stats = {}
        for b in BUILDS:
            rs = runs[(b, k)]
            tot = med_range([total(r) for r in rs])
            share = med_range([float(r["apply_ms"]) / total(r) for r in rs])
            apply_ = med_range([float(r["apply_ms"]) for r in rs])
            stats[b] = tot
            print(
                f"  {b:6} n={len(rs)} total {tot[0]:,.3f} [{tot[1]:,.3f}, {tot[2]:,.3f}] ms | "
                f"apply {apply_[0]:,.3f} [{apply_[1]:,.3f}, {apply_[2]:,.3f}] ms | "
                f"apply share {100 * share[0]:.1f}% [{100 * share[1]:.1f}, {100 * share[2]:.1f}]"
            )
        bi = stats["before"][0] / stats["index"][0]
        il = stats["index"][0] / stats["latch"][0]
        overlap = stats["index"][1] <= stats["latch"][2] and stats["latch"][1] <= stats["index"][2]
        print(
            f"  before/index median {bi:.1f}x; index/latch median {il:.3f}x "
            f"({100 * (1 - 1 / il):.1f}% lower); index and latch ranges overlap: {overlap}"
        )
    section("Ablation, bootstrapped pages (m1b-phase4-ablation.csv, ivmlite)")
    for k in cells:
        pages = {}
        for b in BUILDS:
            seen = {(int(r["bootstrapped_pages"]), int(r["maintained_pages"]), int(r["maintained_free"])) for r in runs[(b, k)]}
            pages[b] = seen
        before = next(iter(pages["before"]))[0]
        index = next(iter(pages["index"]))[0]
        print(
            f"  {fmt_cell(k)}: bootstrapped pages before {before}, index {index} "
            f"(+{100 * (index / before - 1):.1f}%), latch {sorted(p[0] for p in pages['latch'])}; "
            f"one value per build: {all(len(s) == 1 for s in pages.values())}"
        )


def ablation_write_amp(aw):
    section("Ablation write cost, us per written row (m1b-phase4-ablation-write-amp.csv)")
    us = {(r["label"], int(r["views"]), r["op"], r["recursive_triggers"]): float(r["us_per_row"]) for r in aw}
    counts = collections.Counter((r["label"], r["views"], r["op"], r["recursive_triggers"]) for r in aw)
    print(f"rows: {len(aw)}; runs per (build, views, op, mode): {sorted(set(counts.values()))}")
    print("views mode op | before | index | latch | index-before | latch-index | before/latch")
    for v in (10, 200):
        for m in MODES:
            for op in OPS:
                b, i, l = (us[(x, v, op, m)] for x in BUILDS)
                print(
                    f"  {v:3} {m:5} {op:14} | {b:7.3f} | {i:7.3f} | {l:7.3f} | "
                    f"{i - b:+7.3f} | {l - i:+7.3f} | {b / l:.3f}x"
                )
    for v in (10, 200):
        rise = [us[("index", v, op, m)] - us[("before", v, op, m)] for op in OPS if op != "delete" for m in MODES]
        save = [us[("index", v, op, m)] - us[("latch", v, op, m)] for op in OPS if op != "delete" for m in MODES]
        index_ratio = [us[("index", v, op, m)] / us[("latch", v, op, m)] for op in OPS if op != "delete" for m in MODES]
        net = [us[("before", v, op, m)] / us[("latch", v, op, m)] for op in OPS if op != "delete" for m in MODES]
        print(
            f"views={v}, non-delete ops: index-before {min(rise):+.2f}..{max(rise):+.2f} us; "
            f"index-latch {min(save):.2f}..{max(save):.2f} us; index/latch {min(index_ratio):.3f}..{max(index_ratio):.3f}x; "
            f"before/latch {min(net):.3f}..{max(net):.3f}x"
        )
        deletes = [us[(b, v, "delete", m)] for b in BUILDS for m in MODES]
        print(f"views={v}, delete: {min(deletes):.3f}..{max(deletes):.3f} us across builds and modes")


def write_amp(wa):
    section("Write amplification (m1b-phase4-write-amp.csv)")
    rows = {(r["engine"], int(r["views"]), r["op"], r["recursive_triggers"]): r for r in wa}
    views = sorted({k[1] for k in rows})

    def mult(e, v, op, m):
        return float(rows[(e, v, op, m)]["apply_ms"]) / float(rows[("no_maintenance", v, op, m)]["apply_ms"])

    def extra(e, v, op, m):
        return float(rows[(e, v, op, m)]["extra_us_per_row"])

    for e in ("ivmlite", "hand_written_trigger"):
        zx = [extra(e, 0, op, m) for op in OPS for m in MODES]
        zm = [mult(e, 0, op, m) for op in OPS for m in MODES]
        print(f"zero views, {e}: extra us/row {min(zx):+.2f}..{max(zx):+.2f}; multiple {min(zm):.2f}..{max(zm):.2f}x")
    print("views op | OFF ivmlite | OFF trigger | ON ivmlite | ON trigger   (* = trigger summary wrong, cost only)")
    for v in views:
        if v == 0:
            continue
        for op in OPS:
            parts = []
            for m in MODES:
                star = "*" if (op in REPLACE_OPS and m == "false") else ""
                parts.append(f"{mult('ivmlite', v, op, m):.2f}x (+{extra('ivmlite', v, op, m):.2f})")
                parts.append(f"{mult('hand_written_trigger', v, op, m):.2f}x (+{extra('hand_written_trigger', v, op, m):.2f}){star}")
            print(f"  {v:3} {op:14} | " + " | ".join(parts))
    print("ivmlite extra us/row below hand_written_trigger's, per (views, op), both modes:")
    for v in views:
        if v == 0:
            continue
        lower = [op for op in OPS if all(extra("ivmlite", v, op, m) < extra("hand_written_trigger", v, op, m) for m in MODES)]
        print(f"  views={v}: {lower}")
        for m in MODES:
            per_mode = [op for op in OPS if extra("ivmlite", v, op, m) < extra("hand_written_trigger", v, op, m)]
            print(f"    with recursive_triggers={m}: {per_mode}")
    for v in views:
        if v == 0:
            continue
        iv = [extra("ivmlite", v, op, m) for op in OPS if op != "delete" for m in MODES]
        ivd = [extra("ivmlite", v, "delete", m) for m in MODES]
        print(f"  views={v}: ivmlite non-delete extra {min(iv):.2f}..{max(iv):.2f} us/row; delete {min(ivd):.2f}..{max(ivd):.2f}")
    for e in ("ivmlite", "hand_written_trigger"):
        for v in views:
            if v == 0:
                continue
            diffs = {op: extra(e, v, op, "true") - extra(e, v, op, "false") for op in OPS}
            worst = max(diffs, key=lambda op: abs(diffs[op]))
            print(f"  {e} views={v}: ON - OFF extra us/row, largest |difference| {diffs[worst]:+.2f} ({worst})")
    for v in views:
        if v == 0:
            continue
        rt = [extra("ivmlite", v, op, m) for op in REPLACE_OPS for m in MODES]
        two = {m: extra("ivmlite", v, "replace_two", m) for m in MODES}
        best = {m: max(REPLACE_OPS, key=lambda op: extra("ivmlite", v, op, m)) for m in MODES}
        print(f"  views={v}: most expensive REPLACE form for ivmlite by extra us/row: OFF {best['false']}, ON {best['true']}; "
              f"REPLACE extra {min(rt):.2f}..{max(rt):.2f}; replace_two OFF {two['false']:.2f} ON {two['true']:.2f}")


def matrix_write_amp(cells):
    section("Matrix write amplification (m1b-phase4.csv)")
    by_views = collections.defaultdict(lambda: collections.defaultdict(list))
    for k, e in cells.items():
        v, _, batch, _ = k
        nm = float(e["no_maintenance"]["apply_ms"])
        for eng in ("ivmlite", "hand_written_trigger"):
            a = float(e[eng]["apply_ms"])
            by_views[(v, batch)][eng].append((a / nm, (a - nm) * 1000 / batch))
    print("views batch cells | ivmlite multiple median [min, max] / extra us/row median [min, max] | trigger the same")
    for key in sorted(by_views):
        parts = []
        for eng in ("ivmlite", "hand_written_trigger"):
            xs = by_views[key][eng]
            m = med_range([x[0] for x in xs])
            u = med_range([x[1] for x in xs])
            parts.append(f"{m[0]:.2f}x [{m[1]:.2f}, {m[2]:.2f}] / +{u[0]:.2f} [{u[1]:.2f}, {u[2]:.2f}]")
        print(f"  {key[0]:3} {key[1]:4} {len(by_views[key]['ivmlite']):2} | " + " | ".join(parts))
    all_iv = [x for d in by_views.values() for x in d["ivmlite"]]
    all_tr = [x for d in by_views.values() for x in d["hand_written_trigger"]]
    lower = sum(1 for k, e in cells.items() if float(e["ivmlite"]["apply_ms"]) < float(e["hand_written_trigger"]["apply_ms"]))
    for v in sorted({k[0] for k in cells}):
        ks = [k for k in cells if k[0] == v]
        lo = [k for k in ks if float(cells[k]["ivmlite"]["apply_ms"]) < float(cells[k]["hand_written_trigger"]["apply_ms"])]
        print(f"  views={v}: ivmlite apply_ms below the trigger's in {len(lo)} of {len(ks)} cells; "
              f"batches of those: {sorted(collections.Counter(k[2] for k in lo).items())}")
    print(f"all {len(all_iv)} cells: ivmlite extra median +{statistics.median(x[1] for x in all_iv):.2f} us/row, "
          f"trigger +{statistics.median(x[1] for x in all_tr):.2f}; cells where ivmlite apply_ms < trigger apply_ms: {lower}")


def space(cells, wa):
    section("Space (m1b-phase4.csv, ivmlite)")
    slice_ = [(10, 100000, 100, 10), (10, 100000, 100, 1000), (10, 100000, 100, 100000), (200, 100000, 1000, 1000)]
    for k in slice_:
        r = cells[k]["ivmlite"]
        ps = int(r["page_size"])
        base = used_pages(r, "base")
        parts = []
        for p in ("bootstrapped", "written", "maintained"):
            u = used_pages(r, p)
            parts.append(f"{mib(u, ps):.2f} MiB ({u / base:.2f}x)")
        print(f"  {fmt_cell(k)}: page_size {ps}, base {mib(base, ps):.2f} MiB | " + " | ".join(parts))
    print("bootstrapped vs maintained used pages, ivmlite, by groups (all matrix cells):")
    for k in sorted(cells):
        r = cells[k]["ivmlite"]
        b, m = used_pages(r, "bootstrapped"), used_pages(r, "maintained")
        if k[3] == 100000 or k in slice_:
            print(
                f"  {fmt_cell(k)}: bootstrapped {b} (free {r['bootstrapped_free']}), "
                f"maintained {m} (free {r['maintained_free']}), bootstrapped/maintained {b / m:.2f}x"
            )


def main():
    matrix = load("m1b-phase4.csv")
    cells, speedup = exploration(matrix)
    confirmation(load("m1b-phase4-confirm.csv"), speedup)
    ablation(load("m1b-phase4-ablation.csv"), load("m1b-phase4-ablation-builds.csv"))
    ablation_write_amp(load("m1b-phase4-ablation-write-amp.csv"))
    wa = load("m1b-phase4-write-amp.csv")
    write_amp(wa)
    matrix_write_amp(cells)
    space(cells, wa)


if __name__ == "__main__":
    main()
