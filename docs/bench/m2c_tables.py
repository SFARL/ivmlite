#!/usr/bin/env python3
"""Validate M2c evidence and print before/after tables (Python standard library).

Run with --self-check to exercise the real CSV parsers using temporary synthetic
fixtures. That mode never reads or writes benchmark evidence. Normal invocation
requires all seven m2c-*.csv inputs; --input-dir and individual path overrides
support private/scratch captures without changing the tracked producer output.
An optional m2c-confirm-rerun/ bundle beside those inputs must be complete and
hash-consistent when present; its tables retain the original flags and controls.

Both builds files use scope,label,commit,harness_commit,harness_dirty,sqlite_version.
The ablation builds scopes are ablation, confirm, write_amp. Demo scopes are
full case names, explicitly preserving the old harness for before FluxFlow/Kener.
"""

import argparse
import csv
import io
import hashlib
import json
import itertools
import math
import re
import statistics
import sys
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path


LABELS = ("before", "after")
COMMITS = {
    "before": "b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa",
    "after": "84e4c07c2ebfc9db8b6bafb57378627414df5524",
}
ENGINES = ("no_maintenance", "hand_written_trigger", "naive_recompute", "ivmlite")
ABLATION_ENGINES = ("ivmlite", "naive_recompute")
DIMENSIONS = ("views", "base_rows", "batch_size", "group_cardinality")
MATRIX_HEADER = tuple((
    "engine,views,base_rows,batch_size,group_cardinality,bootstrap_ms,apply_ms,"
    "maintain_ms,page_size,base_pages,base_free,bootstrapped_pages,bootstrapped_free,"
    "written_pages,written_free,maintained_pages,maintained_free,first_refresh_ms,"
    "apply_plus_maintain_ms"
).split(","))
PAGE_FIELDS = tuple(field for field in MATRIX_HEADER if field.endswith(("_pages", "_free")))
MATRIX_INTS = (*DIMENSIONS, "page_size", *PAGE_FIELDS)
WRITE_HEADER = tuple((
    "label,engine,views,op,recursive_triggers,rows,apply_ms,us_per_row,"
    "extra_us_per_row,page_size,base_pages,base_free,bootstrapped_pages,"
    "bootstrapped_free,written_pages,written_free,maintained_pages,maintained_free"
).split(","))
DEMO_METRICS = ("bootstrap_ms", "first_refresh_ms", "apply_ms", "maintain_ms", "read_ms",
                "apply_plus_maintain_ms", "end_to_end_ms", "database_kib")


def triplet(metric):
    stem = metric.removesuffix("_ms")
    unit = "_ms" if metric.endswith("_ms") else ""
    return metric, f"{stem}_min{unit}", f"{stem}_max{unit}"


DEMO_HEADER = ("label", "case", "mode", "base_rows", "batch_size", "repeats",
               *(field for metric in DEMO_METRICS for field in triplet(metric)))
BUILD_HEADER = ("scope", "label", "commit", "harness_commit", "harness_dirty", "sqlite_version")
CHANGED_CASES = {"fluxflow_grouped_flow_rollup", "kener_quarter_hour_rollup"}
HANDWRITTEN_CASES = {"fluxflow_grouped_flow_rollup", "zenod_usage_totals", "nicti_facet_counts",
                     "postio_mailbox_footprint", "phoenix_token_rollup", "pocketbase_author_counts"}
DEFAULT_FILES = {
    "ablation": "m2c-ablation.csv", "builds": "m2c-ablation-builds.csv",
    "write_amp": "m2c-ablation-write-amp.csv", "confirm_before": "m2c-confirm-before.csv",
    "confirm_after": "m2c-confirm-after.csv", "demos": "m2c-demos.csv",
    "demo_builds": "m2c-demos-builds.csv",
}
BUDGET_METRICS = {"maintain_ms", "apply_plus_maintain_ms"}


@dataclass
class Expected:
    ablation_cells: set
    confirm_cells: set
    ablation_repeats: int
    cases: dict
    write_keys: set
    write_rows: int
    repeats: int = 5  # Confirm/demo protocol: five fresh runs, not five batches.


def read_csv(path, header, text=(), integers=(), signed=()):
    """One strict producer schema, with finite typed values and no silent rows."""
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.reader(handle, strict=True)
        actual_header = next(reader, None)
        if actual_header != list(header):
            raise ValueError(f"{path}: expected exact header {','.join(header)}")
        rows = []
        for line, fields in enumerate(reader, 2):
            if len(fields) != len(header) or any(not field or field != field.strip() for field in fields):
                raise ValueError(f"{path}:{line}: empty field, whitespace, or wrong field count")
            row = dict(zip(header, fields))
            for field in header:
                if field in text:
                    continue
                value = row[field]
                if field in integers:
                    if not re.fullmatch(r"[0-9]+", value):
                        raise ValueError(f"{path}:{line}: {field} must be a nonnegative integer")
                    row[field] = int(value)
                else:
                    try:
                        number = float(value)
                    except ValueError as error:
                        raise ValueError(f"{path}:{line}: {field} is not numeric") from error
                    if not math.isfinite(number) or (number < 0 and field not in signed):
                        raise ValueError(f"{path}:{line}: {field} must be finite and nonnegative")
                    row[field] = number
            rows.append(row)
    return rows


def require_grid(rows, columns, expected, context):
    found = set()
    for row in rows:
        key = tuple(row[column] for column in columns)
        if key in found:
            raise ValueError(f"{context}: duplicate key {key}")
        found.add(key)
    if found != expected:
        missing = sorted(expected - found)[:3]
        extra = sorted(found - expected)[:3]
        raise ValueError(f"{context}: wrong cells/labels; missing={missing}, unexpected={extra}")


def require_labels(rows, context):
    found = {row["label"] for row in rows}
    if found != set(LABELS):
        raise ValueError(f"{context}: labels must be exactly before,after; found {sorted(found)}")


def matrix_rows(path, labelled):
    prefix = ("label", "repeat") if labelled else ("repeat",)
    return read_csv(path, (*prefix, *MATRIX_HEADER), text=("label", "engine"),
                    integers=("repeat", *MATRIX_INTS))


def expected_from_repo(root, phase6):
    with (root / "workloads/m0-baseline.toml").open("rb") as handle:
        ablation = tomllib.load(handle)["ablation"]
    cells = {tuple(cell[field] for field in DIMENSIONS) for cell in ablation["cells"]}
    if len(cells) != len(ablation["cells"]):
        raise ValueError("workloads/m0-baseline.toml: duplicate ablation cells")
    exploration = read_csv(phase6, MATRIX_HEADER, text=("engine",), integers=MATRIX_INTS)
    by_cell = {}
    for row in exploration:
        cell = tuple(row[field] for field in DIMENSIONS)
        engines = by_cell.setdefault(cell, {})
        if row["engine"] in engines:
            raise ValueError(f"{phase6}: duplicate exploration cell/engine {cell}/{row['engine']}")
        engines[row["engine"]] = row["apply_plus_maintain_ms"]
    confirm = set()
    for cell, values in by_cell.items():
        if set(values) != set(ENGINES):
            raise ValueError(f"{phase6}: incomplete exploration engine set at {cell}")
        iv, naive, trigger = (values[name] for name in ("ivmlite", "naive_recompute", "hand_written_trigger"))
        if (iv > 0 and 0.7 <= naive / iv <= 2.8) or iv < trigger:
            confirm.add(cell)
    if not confirm:
        raise ValueError(f"{phase6}: confirmation selection is empty")

    # The shell producer is the source of truth for documented demo scales.
    producer = (root / "scripts/bench-demos.sh").read_text(encoding="utf-8")
    match = re.search(r'^default_demos="([^"\n]*(?:\n[^"\n]*)*)"$', producer, re.MULTILINE)
    if not match:
        raise ValueError("scripts/bench-demos.sh: cannot locate literal default_demos")
    cases = {}
    for line in match[1].splitlines():
        words = line.split()
        if words[0] == "fluxflow_demo" and len(words) == 3:
            case, rows, batch = "fluxflow_grouped_flow_rollup", *map(int, words[1:])
        elif words[0] == "demand_case_bench" and len(words) == 4:
            case, rows, batch = words[1], *map(int, words[2:])
        else:
            raise ValueError(f"unsupported default demo invocation: {line}")
        if case in cases:
            raise ValueError(f"duplicate default demo: {case}")
        modes = {"no_maintenance", "unindexed_recompute", "indexed_recompute", "ivmlite"}
        if case in HANDWRITTEN_CASES:
            modes.add("handwritten_trigger")
        cases[case] = (rows, batch, modes)
    if not CHANGED_CASES <= cases.keys():
        raise ValueError("default demos must include changed FluxFlow and Kener cases")
    with (root / "workloads/write-amp.toml").open("rb") as handle:
        write = tomllib.load(handle)
    write_keys = set(itertools.product(write["ablation_view_counts"], write["ops"],
                                       (str(value).lower() for value in write["recursive_triggers"])))
    return Expected(cells, confirm, ablation["repeats"], cases, write_keys, write["rows_per_op"])


def read_builds(path, scopes):
    rows = read_csv(path, BUILD_HEADER, text=BUILD_HEADER)
    require_labels(rows, path)
    require_grid(rows, ("scope", "label"), set(itertools.product(scopes, LABELS)), path)
    for row in rows:
        for field in ("commit", "harness_commit"):
            if not re.fullmatch(r"[0-9a-f]{40}", row[field]):
                raise ValueError(f"{path}: {field} must be a full 40-hex SHA")
        if row["commit"] != COMMITS[row["label"]]:
            raise ValueError(f"{path}: wrong extension commit for {row['scope']}/{row['label']}")
        if row["harness_dirty"] != "no":
            raise ValueError(f"{path}: publishable evidence requires harness_dirty=no")
        if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", row["sqlite_version"]):
            raise ValueError(f"{path}: invalid SQLite version")
    return {(row["scope"], row["label"]): row for row in rows}


def load_all(paths, expected):
    ablation = matrix_rows(paths["ablation"], True)
    require_labels(ablation, paths["ablation"])
    grid = {(label, repeat, engine, *cell) for label, repeat, engine, cell in itertools.product(
        LABELS, range(expected.ablation_repeats), ABLATION_ENGINES, expected.ablation_cells)}
    require_grid(ablation, ("label", "repeat", "engine", *DIMENSIONS), grid, paths["ablation"])
    confirm = []
    for label in LABELS:
        rows = matrix_rows(paths[f"confirm_{label}"], False)
        grid = {(repeat, engine, *cell) for repeat, engine, cell in itertools.product(
            range(expected.repeats), ENGINES, expected.confirm_cells)}
        require_grid(rows, ("repeat", "engine", *DIMENSIONS), grid, paths[f"confirm_{label}"])
        confirm.extend(dict(row, label=label) for row in rows)

    demos = read_csv(paths["demos"], DEMO_HEADER, text=("label", "case", "mode"),
                     integers=("base_rows", "batch_size", "repeats"))
    require_labels(demos, paths["demos"])
    grid = {(label, case, mode, rows, batch, expected.repeats)
            for case, (rows, batch, modes) in expected.cases.items()
            for label, mode in itertools.product(LABELS, modes)}
    require_grid(demos, ("label", "case", "mode", "base_rows", "batch_size", "repeats"), grid, paths["demos"])
    for row in demos:
        for metric in DEMO_METRICS:
            median, low, high = (row[field] for field in triplet(metric))
            if not low <= median <= high:
                raise ValueError(f"{paths['demos']}: invalid min/median/max for {row['case']}/{metric}")

    write_amp = read_csv(paths["write_amp"], WRITE_HEADER,
                         text=("label", "engine", "op", "recursive_triggers"),
                         integers=("views", "rows", "page_size", *PAGE_FIELDS),
                         signed=("extra_us_per_row",))
    require_labels(write_amp, paths["write_amp"])
    grid = {(label, "ivmlite", *key, expected.write_rows)
            for label, key in itertools.product(LABELS, expected.write_keys)}
    require_grid(write_amp, ("label", "engine", "views", "op", "recursive_triggers", "rows"), grid, paths["write_amp"])

    builds = read_builds(paths["builds"], ("ablation", "confirm", "write_amp"))
    demo_builds = read_builds(paths["demo_builds"], expected.cases)
    all_builds = [*builds.values(), *demo_builds.values()]
    if len({row["sqlite_version"] for row in all_builds}) != 1:
        raise ValueError("SQLite versions differ across scoped build provenance")
    # Each wrapper uses one harness for both extensions. Separate captures may
    # have different clean harness commits; preserve those recorded identities.
    for scope in ("ablation", "write_amp"):
        if builds[scope, "before"]["harness_commit"] != builds[scope, "after"]["harness_commit"]:
            raise ValueError(f"{scope}: both extensions must use the same recorded harness")
    for (case, label), row in demo_builds.items():
        if case in CHANGED_CASES and label == "before" and row["harness_commit"] != COMMITS["before"]:
            raise ValueError(f"unexpected harness for demo {case}/{label}; preserve changed-shape provenance")
        if case not in CHANGED_CASES and row["harness_commit"] != demo_builds[case, "before"]["harness_commit"]:
            raise ValueError(f"{case}: unchanged demo must use the same recorded harness for both extensions")
    return ablation, confirm, demos, write_amp, builds, demo_builds


def ratio(before, after):
    if before == 0:
        return None if after == 0 else math.copysign(math.inf, after)
    return after / before


def matrix_groups(rows):
    grouped = {}
    for row in rows:
        key = (*[row[field] for field in DIMENSIONS], row["engine"])
        grouped.setdefault(key, {}).setdefault(row["label"], []).append(row)
    return grouped


def matrix_summary(labels, metric):
    summaries = []
    for label in LABELS:
        values = [row[metric] for row in labels[label]]
        summaries.append((statistics.median(values), min(values), max(values)))
    return summaries


def matrix_flags(rows):
    return {(key, metric) for key, labels in matrix_groups(rows).items()
            for metric in BUDGET_METRICS
            if (multiple := ratio(*(item[0] for item in matrix_summary(labels, metric)))) is not None
            and multiple > 1.05}


def load_confirm_rerun(directory, confirm, expected, selection_source):
    """Optional complete rerun of precisely the originally implicated cells."""
    if not directory.exists():
        return None
    original_flags = matrix_flags(confirm)
    cells = {key[:-1] for key, _ in original_flags}
    if not cells or not cells <= expected.confirm_cells:
        raise ValueError(f"{directory}: rerun requires originally flagged confirmation cells")
    selection = read_csv(directory / "selection.csv", MATRIX_HEADER, text=("engine",), integers=MATRIX_INTS)
    require_grid(selection, ("engine", *DIMENSIONS),
                 {(engine, *cell) for engine, cell in itertools.product(ENGINES, cells)}, directory)
    source = read_csv(selection_source, MATRIX_HEADER, text=("engine",), integers=MATRIX_INTS)
    selected_source = [row for row in source if tuple(row[field] for field in DIMENSIONS) in cells]
    if sorted(selection, key=lambda row: (row["engine"], *[row[field] for field in DIMENSIONS])) != sorted(
            selected_source, key=lambda row: (row["engine"], *[row[field] for field in DIMENSIONS])):
        raise ValueError(f"{directory}: selection must preserve the fixed exploration rows")
    rerun = []
    for label in LABELS:
        path = directory / f"{label}.csv"
        rows = matrix_rows(path, False)
        grid = {(repeat, engine, *cell) for repeat, engine, cell in itertools.product(
            range(expected.repeats), ENGINES, cells)}
        require_grid(rows, ("repeat", "engine", *DIMENSIONS), grid, path)
        rerun.extend(dict(row, label=label) for row in rows)
    try:
        metadata = json.loads((directory / "runs.json").read_text(encoding="utf-8"))
        if metadata["format_version"] != 1 or not metadata["normalization"]:
            raise ValueError("invalid metadata format/normalization")
        for field in ("original_runs_sha256", "original_driver_sha256", "published_driver_sha256"):
            if not re.fullmatch(r"[0-9a-f]{64}", metadata[field]):
                raise ValueError(f"invalid {field}")
        digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
        if metadata["published_driver_sha256"] != digest(directory / "capture.py"):
            raise ValueError("published driver hash mismatch")
        runs = metadata["runs"]
        require_grid(runs, ("label",), {(label,) for label in LABELS}, directory)
        for run in runs:
            label = run["label"]
            if run["source_commit"] != COMMITS[label] or run["harness_commit"] != COMMITS[label]:
                raise ValueError(f"{label}: rerun must use its own frozen source and harness")
            for field in ("binary_sha256", "library_sha256"):
                if not re.fullmatch(r"[0-9a-f]{64}", run[field]):
                    raise ValueError(f"invalid {field}")
            for field, path in (("output_sha256", directory / f"{label}.csv"),
                                ("selection_sha256", directory / "selection.csv"),
                                ("selection_source_sha256", selection_source)):
                if run[field] != digest(path):
                    raise ValueError(f"{label}: {field} mismatch")
            command = ["${FROZEN_ROOT}/target/release/ivmlite-bench", "confirm", "--from",
                       "${EVIDENCE_DIR}/selection.csv", "--workload", "${FROZEN_ROOT}/workloads/m0-baseline.toml",
                       "--extension", "${FROZEN_ROOT}/crates/ivmlite-sqlite/target/release/libivmlite_sqlite.dylib"]
            if run["command"] != command or type(run["returncode"]) is not int or run["returncode"] != 0:
                raise ValueError(f"{label}: wrong command or unsuccessful run")
            if not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z", run["started_at"]) or not run["platform"]:
                raise ValueError(f"{label}: missing timestamp/platform")
            numbers = [run["wall_seconds"], *run["load_start"], *run["load_end"]]
            if len(run["load_start"]) != 3 or len(run["load_end"]) != 3 or any(
                    type(value) not in (int, float) or not math.isfinite(value) or value < 0 for value in numbers):
                raise ValueError(f"{label}: invalid duration/load provenance")
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        raise ValueError(f"{directory}: malformed rerun provenance: {error}") from error
    return rerun


def emit_comparison(key, metric, before, after, gate=True):
    multiple = ratio(before[0], after[0])
    flag = gate and metric in BUDGET_METRICS and multiple is not None and multiple > 1.05
    display = "n/a (0/0)" if multiple is None else f"{multiple:.3f}"
    summaries = [f"{mid:.3f} [{low:.3f}, {high:.3f}]" for mid, low, high in (before, after)]
    print(f"| {key} | {metric} | {summaries[0]} | {summaries[1]} | {display} | {'>1.05' if flag else ''} |")
    return int(flag)


def report(data, rerun=None):
    ablation, confirm, demos, write_amp, builds, demo_builds = data
    print("Validated rows: " + ", ".join(f"{name}={len(rows)}" for name, rows in (
        ("ablation", ablation), ("confirm", confirm), ("demos", demos), ("write_amp", write_amp))))
    print("Before/after extension SHAs: " + ", ".join(f"{label}={COMMITS[label]}" for label in LABELS))
    print(f"SQLite: {next(iter(builds.values()))['sqlite_version']}; all scoped harnesses clean.")
    print("Ratios are after/before (>1 is slower). Values are median [min, max]. "
          "Only maintain_ms and apply_plus_maintain_ms enter the integer 1.05 check. "
          "0/0 is n/a; a positive result over zero is inf.")
    flags = 0
    for name, rows in (("Integer ablation", ablation), ("Integer confirm", confirm)):
        print(f"\n{name}\n\n| cell (views/base/batch/groups), engine | metric | before | after | ratio | flag |")
        print("|---|---|---:|---:|---:|---|")
        for key, labels in sorted(matrix_groups(rows).items()):
            for metric in ("bootstrap_ms", "first_refresh_ms", "apply_ms", "maintain_ms", "apply_plus_maintain_ms"):
                flags += emit_comparison("/".join(map(str, key)), metric, *matrix_summary(labels, metric))

    lookup = {(row["label"], row["case"], row["mode"]): row for row in demos}
    for changed in (False, True):
        if changed:
            print("\nCHANGED SHAPE: FluxFlow and Kener\n\n"
                  "Before uses each original integer query/harness; after uses non-STRICT REAL "
                  "SUM (FluxFlow) or AVG/MIN/MAX/COUNT (Kener). These shapes are outside the "
                  "integer budget. Their ratio is not an engine improvement or a budget verdict.")
        else:
            print("\nUnchanged integer demos")
        print("\n| case, mode | metric | before | after | ratio | flag |\n|---|---|---:|---:|---:|---|")
        for case, mode in sorted({(row["case"], row["mode"]) for row in demos if (row["case"] in CHANGED_CASES) == changed}):
            for metric in DEMO_METRICS:
                summaries = [tuple(lookup[label, case, mode][field] for field in triplet(metric)) for label in LABELS]
                flags += emit_comparison(f"{case}, {mode}", metric, *summaries, gate=not changed)

    print("\nWrite amplification (one capture per cell; no repeat ranges or integer maintenance-budget claim)"
          "\n\n| views/op/recursive | metric | before | after | after/before |\n|---|---|---:|---:|---:|")
    lookup = {(row["label"], row["views"], row["op"], row["recursive_triggers"]): row for row in write_amp}
    for key in sorted({(row["views"], row["op"], row["recursive_triggers"]) for row in write_amp}):
        for metric in ("apply_ms", "us_per_row", "extra_us_per_row"):
            before, after = (lookup[label, *key][metric] for label in LABELS)
            multiple = ratio(before, after)
            display = "n/a (0/0)" if multiple is None else f"{multiple:.3f}"
            print(f"| {'/'.join(map(str, key))} | {metric} | {before:.3f} | {after:.3f} | {display} |")
    print(f"\nOriginal integer checks flagged: {flags} metric/cell comparisons above 1.05. "
          + ("Original flags remain visible above."
             if rerun is not None else "Re-run each implicated cell before any performance verdict; report persistent regressions to the controller."
             if flags else "All captured integer maintenance/combined-cost medians are within 1.05."))
    if rerun is not None:
        print(f"\nConfirmation rerun: {len(rerun)} rows; five fresh runs per label/engine, "
              "same selected cells, each frozen build's own clean harness/library. "
              "This follow-up preserves the original captures and the 1.05 threshold.")
        print("\n| cell (views/base/batch/groups), engine | metric | before | after | ratio | flag |")
        print("|---|---|---:|---:|---:|---|")
        for key, labels in sorted(matrix_groups(rerun).items()):
            for metric in ("bootstrap_ms", "first_refresh_ms", "apply_ms", "maintain_ms", "apply_plus_maintain_ms"):
                emit_comparison("/".join(map(str, key)), metric, *matrix_summary(labels, metric))
        original_flags, rerun_flags = matrix_flags(confirm), matrix_flags(rerun)
        persistent = original_flags & rerun_flags
        print(f"\nOriginal confirmation flags reproduced: {len(persistent)}/{len(original_flags)}. "
              f"New rerun flags: {len(rerun_flags - original_flags)}; these remain reported, "
              "and are not evidence of a persistent regression. No further rerun was selected to seek a pass.")
        if not persistent:
            print("The originally flagged confirmation comparisons did not reproduce above 1.05.")
        else:
            print("Persistent confirmation flags require investigation before a performance verdict.")
    print("\nScoped build provenance\n\n| scope | label | extension | harness | SQLite |\n|---|---|---|---|---|")
    for row in [*builds.values(), *demo_builds.values()]:
        print(f"| {row['scope']} | {row['label']} | {row['commit']} | {row['harness_commit']} | {row['sqlite_version']} |")


def self_check_rerun(directory, expected, tables, data):
    """Exercise rerun inventory, provenance and verdicts through the real path."""
    bundle = directory / "m2c-confirm-rerun"
    if load_confirm_rerun(bundle, data[1], expected, directory / "absent.csv") is not None:
        raise ValueError("self-check: absent rerun bundle must remain optional")
    bundle.mkdir()
    before = tables["confirm_before"][1]
    confirm = [dict(row, label=label,
                    maintain_ms=row["maintain_ms"] * (1.1 if label == "after" and row["engine"] == "naive_recompute" else 1),
                    apply_plus_maintain_ms=row["apply_plus_maintain_ms"] * (
                        1.1 if label == "after" and row["engine"] == "naive_recompute" else 1))
               for label, row in itertools.product(LABELS, before)]
    def write_csv(path, header, rows):
        with path.open("w", newline="", encoding="utf-8") as handle:
            writer = csv.DictWriter(handle, fieldnames=header, extrasaction="ignore")
            writer.writeheader()
            writer.writerows(rows)
    selection = [row for row in before if row["repeat"] == 0]
    source = directory / "phase6.csv"
    write_csv(source, MATRIX_HEADER, selection)
    write_csv(bundle / "selection.csv", MATRIX_HEADER, selection)
    for label in LABELS:
        rows = [dict(row,
                     maintain_ms=row["maintain_ms"] * (1.02 if label == "after" else 1),
                     apply_plus_maintain_ms=row["apply_plus_maintain_ms"] * (
                         (1.06 if row["engine"] == "hand_written_trigger" else 1.02) if label == "after" else 1))
                for row in before]
        write_csv(bundle / f"{label}.csv", ("repeat", *MATRIX_HEADER), rows)
    (bundle / "capture.py").write_text("# Temporary parser fixture, not benchmark evidence.\n", encoding="utf-8")
    digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
    metadata = dict(format_version=1, normalization="temporary fixture", original_runs_sha256="0" * 64,
                    original_driver_sha256="0" * 64, published_driver_sha256=digest(bundle / "capture.py"),
                    runs=[dict(label=label, source_commit=COMMITS[label], harness_commit=COMMITS[label],
                               command=["${FROZEN_ROOT}/target/release/ivmlite-bench", "confirm", "--from",
                                        "${EVIDENCE_DIR}/selection.csv", "--workload", "${FROZEN_ROOT}/workloads/m0-baseline.toml",
                                        "--extension", "${FROZEN_ROOT}/crates/ivmlite-sqlite/target/release/libivmlite_sqlite.dylib"],
                               binary_sha256="0" * 64, library_sha256="0" * 64,
                               output_sha256=digest(bundle / f"{label}.csv"),
                               selection_sha256=digest(bundle / "selection.csv"), selection_source_sha256=digest(source),
                               started_at="2026-10-20T22:09:43Z", platform="temporary-fixture", returncode=0,
                               wall_seconds=1, load_start=[1, 1, 1], load_end=[1, 1, 1]) for label in LABELS])
    (bundle / "runs.json").write_text(json.dumps(metadata), encoding="utf-8")
    rerun = load_confirm_rerun(bundle, confirm, expected, source)
    import contextlib
    with contextlib.redirect_stdout(io.StringIO()) as output:
        report((data[0], confirm, *data[2:]), rerun)
    text = output.getvalue()
    if "Original confirmation flags reproduced: 0/2. New rerun flags: 1" not in text or (
            "| 1/100/10/2/hand_written_trigger | apply_plus_maintain_ms | 7.000 [7.000, 7.000] "
            "| 7.420 [7.420, 7.420] | 1.060 | >1.05 |") not in text:
        raise ValueError("self-check: rerun verdict lost resolved original flags or new control flag")
    originals = {path: path.read_bytes() for path in bundle.iterdir()}
    checks = 0
    for corruption in ("wrong_cell", "missing_pair", "missing_repeat", "nonfinite", "bad_hash", "wrong_harness", "missing_run", "bad_command"):
        if corruption in ("wrong_cell", "missing_repeat", "nonfinite"):
            rows = matrix_rows(bundle / "before.csv", False)
            if corruption == "wrong_cell":
                rows[0]["views"] = 2
            elif corruption == "missing_repeat":
                rows = [row for row in rows if row["repeat"] != 4]
            else:
                rows[0]["maintain_ms"] = "NaN"
            write_csv(bundle / "before.csv", ("repeat", *MATRIX_HEADER), rows)
        elif corruption == "missing_pair":
            (bundle / "after.csv").unlink()
        else:
            invalid = json.loads(json.dumps(metadata))
            if corruption == "bad_hash":
                invalid["runs"][0]["output_sha256"] = "0" * 64
            elif corruption == "wrong_harness":
                invalid["runs"][0]["harness_commit"] = COMMITS["after"]
            elif corruption == "missing_run":
                invalid["runs"].pop()
            else:
                invalid["runs"][0]["command"][1] = "matrix"
            (bundle / "runs.json").write_text(json.dumps(invalid), encoding="utf-8")
        try:
            load_confirm_rerun(bundle, confirm, expected, source)
        except (ValueError, OSError):
            checks += 1
        else:
            raise ValueError(f"self-check failed to reject rerun {corruption}")
        finally:
            for path, content in originals.items():
                path.write_bytes(content)
    return checks


def self_check():
    """Small complete fixture and corruptions, all through load_all/read_csv."""
    cell = (1, 100, 10, 2)
    modes = {"no_maintenance", "unindexed_recompute", "indexed_recompute", "ivmlite"}
    cases = {case: (100, 10, modes | ({"handwritten_trigger"} if case in HANDWRITTEN_CASES else set()))
             for case in ("noop_gravity_witness", *sorted(CHANGED_CASES))}
    expected = Expected({cell}, {cell}, 5, cases, {(1, "insert", "false")}, 10)
    tables = {}
    def add(name, header, rows):
        tables[name] = (header, rows)
    def matrix(engine, label):
        factor = 1 if label == "before" else 2
        return (dict.fromkeys(MATRIX_HEADER, 1) | dict(zip(DIMENSIONS, cell))
                | {"engine": engine, "apply_ms": 2 * factor, "maintain_ms": 3 * factor,
                   "apply_plus_maintain_ms": 7 * factor})
    add("ablation", ("label", "repeat", *MATRIX_HEADER), [dict(matrix(engine, label), label=label, repeat=repeat)
        for label, repeat, engine in itertools.product(LABELS, range(5), ABLATION_ENGINES)])
    for label in LABELS:
        add(f"confirm_{label}", ("repeat", *MATRIX_HEADER), [dict(matrix(engine, label), repeat=repeat)
            for repeat, engine in itertools.product(range(5), ENGINES)])
    add("demos", DEMO_HEADER, [dict.fromkeys(DEMO_HEADER, 1) | {"label": label, "case": case, "mode": mode,
        "base_rows": 100, "batch_size": 10, "repeats": 5}
        for case, (_, _, case_modes) in cases.items() for label, mode in itertools.product(LABELS, sorted(case_modes))])
    add("write_amp", WRITE_HEADER, [dict.fromkeys(WRITE_HEADER, 1) | {"label": label, "engine": "ivmlite",
        "op": "insert", "recursive_triggers": "false", "rows": 10} for label in LABELS])
    for name, scopes in (("builds", ("ablation", "confirm", "write_amp")), ("demo_builds", cases)):
        rows = []
        for scope, label in itertools.product(scopes, LABELS):
            old = label == "before" and (scope == "confirm" or scope in CHANGED_CASES)
            rows.append(dict(scope=scope, label=label, commit=COMMITS[label],
                             harness_commit=COMMITS["before"] if old else "c" * 40,
                             harness_dirty="no", sqlite_version="3.53.2"))
        add(name, BUILD_HEADER, rows)
    with tempfile.TemporaryDirectory(prefix="ivmlite-m2c-tables-self-check-") as directory:
        paths = {name: Path(directory) / filename for name, filename in DEFAULT_FILES.items()}
        def write(name, rows=None, header=None):
            default_header, default_rows = tables[name]
            with paths[name].open("w", newline="", encoding="utf-8") as handle:
                writer = csv.DictWriter(handle, fieldnames=header or default_header)
                writer.writeheader()
                writer.writerows(default_rows if rows is None else rows)
        for name in tables:
            write(name)
        data = load_all(paths, expected)
        # This producer column is a signed difference, unlike actual costs.
        write("write_amp", [dict(tables["write_amp"][1][0], extra_us_per_row=-1),
                            *tables["write_amp"][1][1:]])
        load_all(paths, expected)
        write("write_amp")
        # Exercise reporting too, without exposing synthetic numbers as evidence.
        import contextlib
        with contextlib.redirect_stdout(io.StringIO()) as output:
            report(data)
        write("write_amp", [dict(row, extra_us_per_row=0 if row["label"] == "before" else -1)
                            for row in tables["write_amp"][1]])
        with contextlib.redirect_stdout(io.StringIO()) as signed_output:
            report(load_all(paths, expected))
        if "| extra_us_per_row | 0.000 | -1.000 | -inf |" not in signed_output.getvalue():
            raise ValueError("self-check: zero-before signed overhead lost its negative direction")
        write("write_amp")
        if "CHANGED SHAPE" not in output.getvalue() or (
            "| 1/100/10/2/ivmlite | apply_plus_maintain_ms | 7.000 [7.000, 7.000] "
            "| 14.000 [14.000, 14.000] | 2.000 | >1.05 |"
        ) not in output.getvalue():
            raise ValueError("self-check: report lost changed-shape boundary or measured combined-cost column")
        rerun_checks = self_check_rerun(Path(directory), expected, tables, data)
        checks = 0
        mutations = [
            ("ablation", lambda rows: [row for row in rows if row["label"] != "after"]),
            ("ablation", lambda rows: [*rows, rows[0]]),
            ("ablation", lambda rows: [dict(rows[0], label="unknown"), *rows[1:]]),
            ("ablation", lambda rows: [dict(rows[0], views=2), *rows[1:]]),
            ("ablation", lambda rows: [dict(rows[0], maintain_ms="NaN"), *rows[1:]]),
            ("confirm_after", lambda rows: rows[:-1]),
            ("confirm_before", lambda rows: [*rows, rows[0]]),
            ("demos", lambda rows: [dict(rows[0], maintain_min_ms=2), *rows[1:]]),
            ("demos", lambda rows: [dict(rows[0], base_rows=99), *rows[1:]]),
            ("write_amp", lambda rows: [dict(rows[0], apply_ms=-1), *rows[1:]]),
            ("write_amp", lambda rows: [dict(rows[0], extra_us_per_row="inf"), *rows[1:]]),
            ("builds", lambda rows: [dict(rows[0], commit=COMMITS["after"]), *rows[1:]]),
            ("builds", lambda rows: [dict(rows[0], label="unknown"), *rows[1:]]),
            ("builds", lambda rows: rows[:-1]),
            ("demo_builds", lambda rows: [dict(rows[0], harness_dirty="yes"), *rows[1:]]),
            ("demo_builds", lambda rows: [dict(rows[0], harness_commit="short-sha"), *rows[1:]]),
            ("demo_builds", lambda rows: [*rows, rows[0]]),
        ]
        for name, mutate in mutations:
            write(name, mutate(tables[name][1]))
            try:
                load_all(paths, expected)
            except ValueError:
                checks += 1
            else:
                raise ValueError(f"self-check failed to reject invalid {name} fixture")
            finally:
                write(name)
        paths["ablation"].write_text("wrong,header\n", encoding="utf-8")
        try:
            load_all(paths, expected)
        except ValueError:
            checks += 1
        else:
            raise ValueError("self-check failed to reject wrong header")
    print(f"Self-check passed: valid temporary fixtures parsed; {checks} original and {rerun_checks} rerun corruptions rejected. No benchmark report generated.")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--input-dir", type=Path)
    parser.add_argument("--phase6", type=Path, help="Fixed Phase 6 exploration CSV used by confirm selection")
    parser.add_argument("--self-check", action="store_true")
    for name, filename in DEFAULT_FILES.items():
        option = {"builds": "ablation-builds", "demo_builds": "demos-builds"}.get(name, name.replace("_", "-"))
        parser.add_argument("--" + option, dest=name, type=Path, help=f"Override {filename}")
    args = parser.parse_args(argv)
    try:
        if args.self_check:
            self_check()
        else:
            directory = args.input_dir or args.repo_root / "docs/bench"
            paths = {name: getattr(args, name) or directory / filename for name, filename in DEFAULT_FILES.items()}
            expected = expected_from_repo(args.repo_root, args.phase6 or args.repo_root / "docs/bench/m1b-phase6.csv")
            data = load_all(paths, expected)
            rerun = load_confirm_rerun(directory / "m2c-confirm-rerun", data[1], expected,
                                       args.phase6 or args.repo_root / "docs/bench/m1b-phase6.csv")
            report(data, rerun)
    except (OSError, ValueError, csv.Error, tomllib.TOMLDecodeError) as error:
        print(f"m2c_tables: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
