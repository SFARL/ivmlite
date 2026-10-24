#!/usr/bin/env python3
"""Reprobe pinned Nexmark SELECTs and typed native row bags; no timing score."""

import argparse
from collections import Counter
from contextlib import closing
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import platform
import re
import sqlite3
import subprocess
import sys
import tempfile

REPO = Path(__file__).resolve().parent.parent
NEXMARK_SHA = "6b3646c3baec701f1fa74baf938d235f742e5d3c"
QUERY_DIR = Path("nexmark-flink/src/main/resources/queries")
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
BID = [(1, 11, 20, "web", "url", 100, "x")] * 2 + [
    (2, 12, 30, "app", None, 101, None)
]
AUCTION = [(1, "item1", "desc", 1, 10, 0, 1000, 7, 10, "a")] * 2 + [
    (2, "item2", "desc", 1, 10, 0, 1000, 8, 11, None)
]
MUTATIONS = [
    "UPDATE auction SET category=10 WHERE id=2; "
    "UPDATE bid SET price=21 WHERE bidder=11;",
    "DELETE FROM bid WHERE rowid=(SELECT min(rowid) FROM bid WHERE bidder=11);",
    "INSERT INTO bid VALUES(1,11,21,'web','url',100,'x'); "
    "DELETE FROM auction WHERE rowid=(SELECT min(rowid) FROM auction WHERE id=1);",
]


class ProbeError(Exception):
    """Invalid provenance, incomplete execution, or a correctness mismatch."""


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(*args, cwd=REPO):
    return subprocess.check_output(["git", *args], cwd=cwd, text=True).strip()


def inside(root, path):
    return path == root or root in path.parents


def source_inputs():
    paths = git("ls-files", "-co", "--exclude-standard").splitlines()
    return {name: digest(REPO / name) for name in sorted(set(paths))
            if name in {"Cargo.toml", "Cargo.lock", "scripts/probe-m2d.py"}
            or (name.startswith("crates/") and
                (name.endswith(".rs") or Path(name).name in {"Cargo.toml", "Cargo.lock"}))}


def typed(value):
    if value is None:
        return ("null", None)
    if isinstance(value, int):
        return ("integer", value)
    if isinstance(value, float):
        if math.isnan(value):
            raise ProbeError("SQLite returned NaN instead of NULL")
        return ("real", value if math.isfinite(value) else repr(value))
    if isinstance(value, str):
        return ("text", value)
    if isinstance(value, bytes):
        return ("blob", value.hex())
    raise ProbeError(f"Unknown SQLite storage value {type(value)}")


def bag(rows):
    return Counter(tuple(typed(value) for value in row) for row in rows)


def bag_json(rows):
    return [{"row": row, "multiplicity": count}
            for row, count in sorted(rows.items(), key=lambda pair: repr(pair[0]))]


def open_database(filename, extension=None):
    conn = sqlite3.connect(filename, isolation_level=None)
    try:
        if extension:
            conn.enable_load_extension(True)
            conn.execute("SELECT load_extension(?,?)", (str(extension), "sqlite3_ivmlite_init"))
            conn.enable_load_extension(False)
        return conn
    except Exception:
        conn.close()
        raise


def make_view(conn, name, sql):
    conn.execute(f'CREATE VIRTUAL TABLE "{name}" USING ivm(\''
                 + sql.replace("'", "''") + "')")


def corpus(source, extension):
    if git("rev-parse", "HEAD", cwd=source) != NEXMARK_SHA:
        raise ProbeError("Unexpected Nexmark revision")
    if git("status", "--porcelain", "--untracked-files=all", cwd=source):
        raise ProbeError("Nexmark checkout must be clean")
    expected = {source / QUERY_DIR / f"q{i}.sql" for i in range(24)}
    if set((source / QUERY_DIR).glob("q*.sql")) != expected:
        raise ProbeError("Nexmark corpus must be exactly Q0-Q23")
    inventory = []
    for index in range(24):
        path = source / QUERY_DIR / f"q{index}.sql"
        pieces = re.split(rf"INSERT\s+INTO\s+nexmark_q{index}\s+", path.read_text(), flags=re.I)
        if len(pieces) != 2:
            raise ProbeError(f"Cannot extract exactly one defining SELECT for Q{index}")
        sql = pieces[1].strip().rstrip(";")
        with closing(open_database(":memory:", extension)) as conn:
            conn.executescript(SCHEMA)
            try:
                make_view(conn, "probe", sql)
                status, error = "compiled", None
            except sqlite3.Error as failure:
                status, error = "rejected", str(failure)
        inventory.append({"query": f"Q{index}", "source": str(QUERY_DIR / path.name),
                          "sourceSHA256": digest(path), "selectSHA256": hashlib.sha256(sql.encode()).hexdigest(),
                          "select": sql, "status": status, "error": error})
    return inventory


def ports(inventory):
    original = {item["query"]: item["select"] for item in inventory}
    return [
        {"name": "q0", "sourceQuery": "Q0", "sql": original["Q0"],
         "adaptation": "SELECT unchanged; Flink sink/generator/event-time replaced by declared SQLite fixture schema"},
        {"name": "q1", "sourceQuery": "Q1", "sql": original["Q1"],
         "adaptation": "SELECT unchanged; SQLite REAL multiplication, not Flink DECIMAL(23,3) sink rounding; timestamp fixture uses INTEGER"},
        {"name": "q20", "sourceQuery": "Q20", "sql":
         "SELECT auction,bidder,price,channel,url,B.dateTime AS bid_dateTime,"
         "B.extra AS bid_extra,itemName,description,initialBid,reserve,"
         "A.dateTime AS auction_dateTime,expires,seller,category,A.extra AS auction_extra "
         "FROM bid B INNER JOIN auction A ON B.auction=A.id WHERE A.category=10",
         "adaptation": "Add unique output aliases matching upstream sink fields; preserve relational row-bag/filter/join; no COUNT multiplicity rewrite"},
    ]


def seed(conn):
    conn.executescript(SCHEMA)
    conn.executemany("INSERT INTO bid VALUES(?,?,?,?,?,?,?)", BID)
    conn.executemany("INSERT INTO auction VALUES(?,?,?,?,?,?,?,?,?,?)", AUCTION)


def check_bases(connections, bids, auctions):
    for label, conn in connections:
        for table, intended in [("bid", bids), ("auction", auctions)]:
            actual = bag(conn.execute(f"SELECT * FROM {table}"))
            if actual != bag(intended):
                raise ProbeError(f"{label} {table} differs from independently intended base rows")


def execute_ports(extension, scratch, selected):
    main_path, oracle_path = scratch / "maintained.sqlite", scratch / "oracle.sqlite"
    main, oracle = open_database(main_path, extension), open_database(oracle_path)
    checkpoints, controls = [], []
    bids, auctions = list(BID), list(AUCTION)
    schema_format = None
    try:
        seed(main)
        seed(oracle)
        for port in selected:
            make_view(main, port["name"], port["sql"])
        schema_format = main.execute("SELECT value FROM __ivm_meta WHERE key='format'").fetchone()[0]

        def checkpoint(label, intended_bids, intended_auctions):
            check_bases([("maintained", main), ("oracle", oracle)], intended_bids, intended_auctions)
            for port in selected:
                actual, expected = bag(main.execute(f'SELECT * FROM "{port["name"]}"')), bag(oracle.execute(port["sql"]))
                if actual != expected:
                    raise ProbeError(f'{label}: {port["name"]} typed row bag differs from independent SQLite oracle')
                checkpoints.append({"checkpoint": label, "query": port["name"], "equal": True,
                                    "actual": bag_json(actual), "oracle": bag_json(expected)})

        checkpoint("bootstrap", bids, auctions)
        # A real wrong write can make IVM and a same-DB oracle agree. The
        # independent intended-base check must still reject it, then roll back.
        main.execute("BEGIN")
        main.execute("UPDATE bid SET price=price+1 WHERE bidder=11")
        main.execute("SELECT ivm_refresh_all()").fetchone()
        for port in selected:
            if bag(main.execute(f'SELECT * FROM "{port["name"]}"')) != bag(main.execute(port["sql"])):
                raise ProbeError("Wrong-base control did not establish same-database oracle agreement")
        try:
            check_bases([("maintained", main)], bids, auctions)
        except ProbeError as failure:
            controls.append({"control": "wrong-base-write-while-same-db-oracle-agrees", "detected": True,
                             "failure": str(failure)})
        else:
            raise ProbeError("Independent base oracle failed to detect the deliberate wrong write")
        finally:
            main.execute("ROLLBACK")
        checkpoint("intact-after-control-rollback", bids, auctions)

        for index, sql in enumerate(MUTATIONS):
            main.executescript(sql)
            oracle.executescript(sql)
            if index == 0:
                bids = [row[:2] + (21,) + row[3:] if row[1] == 11 else row for row in bids]
                auctions = [row[:8] + (10,) + row[9:] if row[0] == 2 else row for row in auctions]
            elif index == 1:
                del bids[0]
            else:
                bids.append((1, 11, 21, "web", "url", 100, "x"))
                del auctions[0]
            main.execute("SELECT ivm_refresh_all()").fetchone()
            checkpoint(f"mutation-{index + 1}", bids, auctions)

        main.execute("BEGIN")
        oracle.execute("BEGIN")
        main.execute("SAVEPOINT nested")
        oracle.execute("SAVEPOINT nested")
        for conn in [main, oracle]:
            conn.execute("UPDATE bid SET price=price+4 WHERE auction=1")
        temporary = [row[:2] + (row[2] + 4,) + row[3:] if row[0] == 1 else row for row in bids]
        main.execute("SELECT ivm_refresh_all()").fetchone()
        checkpoint("inside-savepoint", temporary, auctions)
        for conn in [main, oracle]:
            conn.execute("ROLLBACK TO nested")
            conn.execute("RELEASE nested")
            conn.execute("COMMIT")
        checkpoint("after-savepoint-rollback", bids, auctions)

        stale = {port["name"]: bag(main.execute(f'SELECT * FROM "{port["name"]}"')) for port in selected}
        new_bid = (3, 13, 7, "web", "new-url", 102, "new")
        new_auction = (3, "item3", "new-desc", 1, 2, 0, 1000, 9, 10, "new")
        for conn in [main, oracle]:
            conn.execute("INSERT INTO bid VALUES(?,?,?,?,?,?,?)", new_bid)
            conn.execute("INSERT INTO auction VALUES(?,?,?,?,?,?,?,?,?,?)", new_auction)
        bids.append(new_bid)
        auctions.append(new_auction)
        main.close()
        oracle.close()
        main, oracle = open_database(main_path, extension), open_database(oracle_path)
        check_bases([("maintained", main), ("oracle", oracle)], bids, auctions)
        for port in selected:
            if bag(main.execute(f'SELECT * FROM "{port["name"]}"')) != stale[port["name"]]:
                raise ProbeError("Pending writes unexpectedly refreshed on reopen/read")
        main.execute("SELECT ivm_refresh_all()").fetchone()
        checkpoint("reopen-pending-then-refresh", bids, auctions)
        for port in selected:
            main.execute(f'DROP TABLE "{port["name"]}"')
        remaining = main.execute("SELECT name FROM sqlite_schema WHERE name LIKE '__ivm_%'").fetchall()
        if remaining:
            raise ProbeError(f"Owned objects remain after last view DROP: {remaining}")
        return {"status": "passed", "format": schema_format, "ports": selected,
                "checkpoints": checkpoints, "independentBaseControl": controls,
                "baseTablesCheckedAtEveryCheckpoint": ["bid", "auction"],
                "schema": SCHEMA, "initial": {"bid": BID, "auction": AUCTION},
                "mutations": MUTATIONS, "dropCleanup": "passed", "scratch": str(scratch)}
    finally:
        main.close()
        oracle.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--nexmark-repo", required=True, type=Path)
    parser.add_argument("--extension", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--scratch", type=Path, default=Path(tempfile.gettempdir()))
    args = parser.parse_args()
    source, extension, output, parent = [path.resolve() for path in
                                        [args.nexmark_repo, args.extension, args.output, args.scratch]]
    for target in [output, parent]:
        if inside(REPO, target) or inside(source, target):
            parser.error("Evidence and scratch storage must be external to both source checkouts")
    if tuple(map(int, sqlite3.sqlite_version.split("."))) < (3, 45, 0):
        parser.error("Use Python linked to SQLite >=3.45")
    if not extension.is_file():
        parser.error("--extension must name a built native library")
    parent.mkdir(parents=True, exist_ok=True)
    output.parent.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="m2d-nexmark-", dir=parent))
    evidence = {"status": "failed", "capturedAtUTC": datetime.now(timezone.utc).isoformat(),
                "nexmarkRevision": NEXMARK_SHA, "sourceBaseCommit": git("rev-parse", "HEAD"),
                "sourceInputsSHA256": source_inputs(),
                "runnerSHA256": digest(Path(__file__)), "extensionSHA256": digest(extension),
                "python": sys.version, "sqlite": sqlite3.sqlite_version,
                "host": {"platform": platform.system(), "architecture": platform.machine()},
                "canonicalNexmarkScore": False}
    try:
        inventory = corpus(source, extension)
        evidence["originalInventory"] = inventory
        evidence["originalSELECTsCompiledUnderSQLiteSchema"] = sum(item["status"] == "compiled" for item in inventory)
        evidence["nativeBagAcceptance"] = execute_ports(extension, scratch, ports(inventory))
        evidence["status"] = "passed"
    except Exception as failure:
        evidence["error"] = f"{type(failure).__name__}: {failure}"
        raise
    finally:
        output.write_text(json.dumps(evidence, indent=2, ensure_ascii=True, allow_nan=False) + "\n")
    print(json.dumps({"status": "passed", "originalSELECTsCompiledUnderSQLiteSchema":
                      evidence["originalSELECTsCompiledUnderSQLiteSchema"], "executedPorts": 3,
                      "typedCheckpoints": len(evidence["nativeBagAcceptance"]["checkpoints"]), "output": str(output)}))


if __name__ == "__main__":
    main()
