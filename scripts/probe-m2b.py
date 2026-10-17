#!/usr/bin/env python3
"""Record pinned Nexmark compatibility and Turso semantics; no timing claims."""

import argparse
from collections import Counter
from contextlib import closing
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import sqlite3
import subprocess
import sys
import tempfile

NEXMARK_SHA = "6b3646c3baec701f1fa74baf938d235f742e5d3c"
TURSO_SHA = "1f48d31b11effb623c2ebd5df6216bb6753d6fe5"
TURSO_VERSION = "Turso 0.9.0-pre.1"
QUERIES = Path("nexmark-flink/src/main/resources/queries")
REPO = Path(__file__).resolve().parent.parent
FIXTURES = REPO / "docs/bench/m2b-preflight/sql"
SCHEMA = """
CREATE TABLE bid(auction INTEGER,bidder INTEGER,price INTEGER,channel TEXT,
 url TEXT,dateTime INTEGER,extra TEXT) STRICT;
CREATE TABLE auction(id INTEGER,itemName TEXT,description TEXT,initialBid INTEGER,
 reserve INTEGER,dateTime INTEGER,expires INTEGER,seller INTEGER,category INTEGER,
 extra TEXT) STRICT;
CREATE TABLE person(id INTEGER,name TEXT,emailAddress TEXT,creditCard TEXT,
 city TEXT,state TEXT,dateTime INTEGER,extra TEXT) STRICT;
CREATE TABLE side_input(key INTEGER,value TEXT) STRICT;
"""
INITIAL = """
INSERT INTO bid VALUES (1,11,20,'web','url',100,'x'),
 (1,11,20,'web','url',100,'x'),(2,12,30,'app',NULL,101,NULL);
INSERT INTO auction VALUES (1,'item1','desc',1,10,0,1000,7,10,'a'),
 (1,'item1','desc',1,10,0,1000,7,10,'a'),
 (2,'item2','desc',1,10,0,1000,8,11,NULL);
"""
MUTATIONS = [
    "UPDATE auction SET category=10 WHERE id=2; "
    "UPDATE bid SET price=21 WHERE bidder=11;",
    "DELETE FROM bid WHERE rowid=(SELECT min(rowid) FROM bid WHERE bidder=11);",
    "INSERT INTO bid VALUES (1,11,21,'web','url',100,'x'); "
    "DELETE FROM auction WHERE rowid=(SELECT min(rowid) FROM auction WHERE id=1);",
]


class ProbeError(Exception):
    """Invalid input, failed oracle, or incomplete probe execution."""


def command(argv, **kwargs):
    return subprocess.run(argv, capture_output=True, text=True, timeout=60, **kwargs)


def digest(path):
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def json_file(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=True) + "\n")


def pinned_checkout(path, expected, label):
    result = command(["git", "-C", str(path), "rev-parse", "HEAD"])
    if result.returncode or result.stdout.strip() != expected:
        raise ProbeError(f"{label} checkout must have HEAD {expected}")
    dirty = command(["git", "-C", str(path), "status", "--porcelain", "--untracked-files=no"])
    if dirty.returncode or dirty.stdout.strip():
        raise ProbeError(f"{label} checkout has tracked changes; use the pinned clean source")
    return {"revision": expected, "tracked_clean": True}


def turso_checkout(binary, explicit):
    if explicit:
        return explicit.resolve()
    for parent in binary.parents:
        if (parent / ".git").exists() and (parent / "cli/Cargo.toml").is_file():
            return parent
    raise ProbeError("cannot locate Turso source for --turso-bin; supply --turso-repo")


def checked_file(path, label, executable=False):
    if not path.is_file() or (executable and not os.access(path, os.X_OK)):
        raise ProbeError(f"{label} must be an existing {'executable' if executable else 'file'}")
    return path.resolve()


def open_extension(library):
    conn = sqlite3.connect(":memory:")
    try:
        conn.enable_load_extension(True)
        conn.execute("SELECT load_extension(?,?)", (str(library), "sqlite3_ivmlite_init"))
        conn.enable_load_extension(False)
        conn.executescript(SCHEMA)
        return conn
    except Exception:
        conn.close()
        raise


def adapted_queries():
    q0 = "SELECT auction,bidder,price,dateTime,extra FROM bid"
    q20 = (
        "SELECT auction,bidder,price,channel,url,B.dateTime AS bid_dateTime,"
        "B.extra AS bid_extra,itemName,description,initialBid,reserve,"
        "A.dateTime AS auction_dateTime,expires,seller,category,A.extra AS auction_extra "
        "FROM bid B INNER JOIN auction A ON B.auction=A.id WHERE A.category=10"
    )
    keys = [
        "auction,bidder,price,dateTime,extra",
        "auction,bidder,price,channel,url,B.dateTime,B.extra,itemName,description,"
        "initialBid,reserve,A.dateTime,expires,seller,category,A.extra",
    ]
    return [
        (name, original, original.replace(" FROM", ",COUNT(*) AS multiplicity FROM")
         + " GROUP BY " + group)
        for name, original, group in zip(["q0_bag", "q20_bag"], [q0, q20], keys)
    ]


def bag_json(bag):
    return [{"row": list(row), "multiplicity": count}
            for row, count in sorted(bag.items(), key=lambda item: repr(item[0]))]


def probe_nexmark(source, library):
    paths = set((source / QUERIES).glob("q*.sql"))
    expected = {source / QUERIES / f"q{i}.sql" for i in range(24)}
    if paths != expected:
        raise ProbeError("pinned Nexmark query inventory must be exactly Q0 through Q23")
    inventory = []
    for i in range(24):
        path = source / QUERIES / f"q{i}.sql"
        parts = re.split(rf"INSERT\s+INTO\s+nexmark_q{i}\s+", path.read_text(), flags=re.I)
        if len(parts) != 2:
            raise ProbeError(f"cannot extract exactly one defining SELECT for Q{i}")
        sql = parts[1].strip().rstrip(";")
        with closing(open_extension(library)) as conn:
            try:
                conn.execute("CREATE VIRTUAL TABLE probe USING ivm('"
                             + sql.replace("'", "''") + "')")
                status, error = "compiled", None
            except sqlite3.Error as failure:
                status, error = "rejected", str(failure)
        inventory.append({"query": f"Q{i}", "source": str(QUERIES / path.name),
                          "source_sha256": digest(path), "select_sha256": hashlib.sha256(sql.encode()).hexdigest(),
                          "status": status, "error": error})
    queries = adapted_queries()
    checkpoints = []
    with closing(open_extension(library)) as conn:
        conn.executescript(INITIAL)
        for name, _, sql in queries:
            conn.execute(f"CREATE VIRTUAL TABLE {name} USING ivm('" + sql.replace("'", "''") + "')")
        for index, mutation in enumerate([None, *MUTATIONS]):
            if mutation:
                conn.executescript(mutation)
                conn.execute("SELECT ivm_refresh_all()").fetchone()
            for name, original, _ in queries:
                actual = Counter()
                for row in conn.execute(f"SELECT * FROM {name}"):
                    if not isinstance(row[-1], int) or row[-1] <= 0:
                        raise ProbeError(f"{name} emitted an invalid multiplicity")
                    actual[tuple(row[:-1])] += row[-1]
                expected_bag = Counter(conn.execute(original))
                checkpoints.append({"query": name, "checkpoint": index,
                                    "equal": actual == expected_bag,
                                    "actual": bag_json(actual), "oracle": bag_json(expected_bag)})
    return {"variant": "nexmark-flink SQL, queries directory, Q0-Q23", "schema_mapping": SCHEMA,
            "original_inventory": inventory, "original_compiled": sum(q["status"] == "compiled" for q in inventory),
            "canonical_nexmark_score": False, "adapted_queries": [
                {"name": name, "original_select": original, "adapted_select": sql}
                for name, original, sql in queries], "initial_data": INITIAL,
            "mutations": MUTATIONS, "adapted_checkpoints": checkpoints}


def compare_output(actual, expected):
    got, want = actual.splitlines(), expected.splitlines()
    differences, representation = [], []
    if len(got) != len(want):
        differences.append({"kind": "row_count", "actual": len(got), "oracle": len(want)})
    for index, (left, right) in enumerate(zip(got, want)):
        if left == right:
            continue
        lfields, rfields = left.split("|"), right.split("|")
        semantic_equal = len(lfields) == len(rfields)
        for lvalue, rvalue in zip(lfields, rfields):
            if lvalue == rvalue or {lvalue, rvalue} == {"integer", "real"}:
                continue
            try:
                equal = Decimal(lvalue) == Decimal(rvalue)
            except InvalidOperation:
                equal = False
            semantic_equal = semantic_equal and equal
        entry = {"row": index, "actual": left, "oracle": right}
        (representation if semantic_equal else differences).append(entry)
    return {"exact_equal": got == want, "numeric_equal": not differences,
            "semantic_differences": differences, "representation_differences": representation}


def sqlite_oracle(sql, database):
    output = []
    with closing(sqlite3.connect(database, isolation_level=None)) as conn:
        statement = ""
        for char in sql:
            statement += char
            if char != ";" or not sqlite3.complete_statement(statement):
                continue
            cursor = conn.execute(statement)
            if cursor.description:
                for row in cursor:
                    output.append("|".join("" if value is None else str(value) for value in row))
            statement = ""
        if statement.strip():
            raise ProbeError("SQL fixture ends with an incomplete statement")
    return "\n".join(output) + ("\n" if output else "")


def turso_run(binary, sql, database, db_label, views=True):
    flags = ["-q", "-m", "list"] + (["--experimental-views"] if views else [])
    result = command([str(binary), *flags, str(database), sql])
    return {"command": ["${TURSO_BIN}", *flags, db_label, sql],
            "exit_code": result.returncode, "stdout": result.stdout, "stderr": result.stderr}


def probe_turso(binary, temporary):
    failures = {"null-sum", "integer-precision", "null-join", "rollback-then-begin",
                "null-sum-transitions", "precision-increment"}
    lifecycle = {"reopen-create", "reopen-update", "drop-cleanup"}
    names = ["null-sum", "integer-precision", "null-join", "rollback-then-begin",
             "null-sum-transitions", "precision-increment", "auto-rollback-zero",
             "commit-twice", "delete-last-row", "nonnull-join-updates",
             "reopen-create", "reopen-update", "drop-cleanup"]
    cases = []
    for name in names:
        sql = (FIXTURES / f"{name}.sql").read_text()
        db = temporary / "lifecycle.db" if name in lifecycle else ":memory:"
        oracle_db = str(temporary / "lifecycle-oracle.db") if name in lifecycle else ":memory:"
        label = "${TEMP_DB}/lifecycle.db" if name in lifecycle else ":memory:"
        ordinary_sql = sql.replace("CREATE MATERIALIZED VIEW", "CREATE VIEW")
        expected = sqlite_oracle(ordinary_sql, oracle_db)
        result = turso_run(binary, sql, db, label)
        result.update(name=name, fixture=f"docs/bench/m2b-preflight/sql/{name}.sql",
                      sqlite_oracle={"engine": "Python sqlite3", "sql": ordinary_sql, "stdout": expected})
        if result["exit_code"]:
            result["status"] = "runtime_failure"
        else:
            result["comparison"] = compare_output(result["stdout"], expected)
            result["status"] = "numeric_match" if result["comparison"]["numeric_equal"] else "semantic_disagreement"
        if name in failures:
            control = turso_run(binary, ordinary_sql, ":memory:", ":memory:")
            control["comparison"] = compare_output(control["stdout"], expected)
            control["status"] = "ordinary_control_pass" if not control["exit_code"] and control["comparison"]["exact_equal"] else "ordinary_control_failed"
            result["ordinary_turso_control"] = control
        cases.append(result)
    for name, views, marker in [("flag-required", False, "experimental"),
                                 ("mvcc-rejected", True, "not supported in MVCC mode"),
                                 ("refresh-rejected", True, 'near "REFRESH": syntax error')]:
        sql = (FIXTURES / f"{name}.sql").read_text()
        result = turso_run(binary, sql, ":memory:", ":memory:", views)
        text = result["stdout"] + result["stderr"]
        result.update(name=name, fixture=f"docs/bench/m2b-preflight/sql/{name}.sql",
                      status="expected_rejection" if result["exit_code"] and marker.lower() in text.lower()
                      else "unsupported_boundary_changed")
        cases.append(result)
    settings_sql = "SELECT sqlite_version(); PRAGMA journal_mode; PRAGMA synchronous; PRAGMA page_size; PRAGMA cache_size;"
    return {"cases": cases, "settings": turso_run(binary, settings_sql, ":memory:", ":memory:")}


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--nexmark-repo", required=True, type=Path)
    parser.add_argument("--turso-bin", required=True, type=Path)
    parser.add_argument("--turso-repo", type=Path, help="source checkout if not an ancestor of --turso-bin")
    parser.add_argument("--extension", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path, help="new or empty evidence directory")
    parser.add_argument("--fail-on-incompatibility", action="store_true", help="exit 2 when verdict is blocked")
    return parser.parse_args()


def main():
    args = parse_args()
    binary = checked_file(args.turso_bin, "--turso-bin", executable=True)
    library = checked_file(args.extension, "--extension")
    nexmark = args.nexmark_repo.resolve()
    provenance = {"nexmark": pinned_checkout(nexmark, NEXMARK_SHA, "Nexmark"),
                  "turso": pinned_checkout(turso_checkout(binary, args.turso_repo), TURSO_SHA, "Turso")}
    if not hasattr(sqlite3.Connection, "enable_load_extension"):
        raise ProbeError("this Python SQLite build lacks loadable-extension support")
    with closing(open_extension(library)):
        pass
    version = command([str(binary), "--version"])
    if version.returncode or version.stdout.strip() != TURSO_VERSION:
        raise ProbeError(f"--turso-bin must report {TURSO_VERSION}")
    if args.out.exists() and (not args.out.is_dir() or any(args.out.iterdir())):
        raise ProbeError("--out must be a new or empty directory")
    args.out.mkdir(parents=True, exist_ok=True)
    provenance.update(schema_version=1, captured_at_utc=datetime.now(timezone.utc).isoformat(),
                      host={"os": platform.system(), "release": platform.release(), "architecture": platform.machine()},
                      python_version=platform.python_version(), sqlite_version=sqlite3.sqlite_version,
                      runner_sha256=digest(Path(__file__)), extension_sha256=digest(library),
                      turso_binary_sha256=digest(binary), turso_version=version.stdout.strip(),
                      turso_binary_association="supplied binary and verified clean source checkout; digest recorded, not a reproducible-build attestation",
                      fixture_sha256={p.name: digest(p) for p in sorted(FIXTURES.glob("*.sql"))},
                      replay_command=["python3", "scripts/probe-m2b.py", "--nexmark-repo", "${NEXMARK_REPO}",
                                      "--turso-bin", "${TURSO_BIN}", "--extension", "${EXTENSION}", "--out", "${OUT}"])
    if args.turso_repo:
        provenance["replay_command"].extend(["--turso-repo", "${TURSO_REPO}"])
    if args.fail_on_incompatibility:
        provenance["replay_command"].append("--fail-on-incompatibility")
    json_file(args.out / "provenance.json", provenance)
    nexmark_result = probe_nexmark(nexmark, library)
    json_file(args.out / "nexmark.json", nexmark_result)
    with tempfile.TemporaryDirectory(prefix="ivmlite-m2b-") as temporary:
        turso_result = probe_turso(binary, Path(temporary))
    json_file(args.out / "turso.json", turso_result)
    blockers = [case["name"] for case in turso_result["cases"]
                if case["status"] in {"runtime_failure", "semantic_disagreement", "unsupported_boundary_changed"}
                or case.get("ordinary_turso_control", {}).get("status") == "ordinary_control_failed"]
    if any(c.get("comparison", {}).get("representation_differences")
           for c in turso_result["cases"]):
        blockers.append("turso_typed_output_difference")
    adapted_ok = all(c["equal"] for c in nexmark_result["adapted_checkpoints"])
    if not adapted_ok:
        blockers.append("nexmark_adapted_oracle_disagreement")
    verdict = {"verdict": "blocked" if blockers else "bounded_numeric_preflight_pass",
               "blockers": blockers, "original_nexmark_compiled": nexmark_result["original_compiled"],
               "original_nexmark_total": 24, "original_rejections_are_inventory": True,
               "adapted_queries": 2, "adapted_checkpoints_per_query": 4, "adapted_oracles_passed": adapted_ok,
               "typed_sql_equivalence": all(c["status"] == "expected_rejection"
                                            or c.get("comparison", {}).get("exact_equal", False)
                                            for c in turso_result["cases"]),
               "canonical_nexmark_score": False, "performance_comparison_authorized_by_result": False}
    json_file(args.out / "verdict.json", verdict)
    print(json.dumps(verdict, indent=2))
    return 2 if blockers and args.fail_on_incompatibility else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ProbeError, OSError, sqlite3.Error, subprocess.SubprocessError) as failure:
        print(f"M2b probe error: {failure}", file=sys.stderr)
        sys.exit(1)
