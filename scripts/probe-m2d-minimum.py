#!/usr/bin/env python3
"""Bounded native M2d acceptance on an explicitly supplied SQLite 3.45.0 CLI."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def typed_group(sql, columns):
    # typeof retains INTEGER/REAL identity; hex retains embedded NULs and blobs.
    terms = []
    for column in columns:
        terms.extend([f'typeof("{column}")', f'"{column}"', f'quote("{column}")',
                      f'hex(CAST("{column}" AS BLOB))'])
    values = ",".join(terms)
    return f"SELECT {values},count(*) FROM ({sql}) GROUP BY {values}"


def parity(label, name, sql, columns):
    actual, oracle = typed_group(f'SELECT * FROM "{name}"', columns), typed_group(sql, columns)
    return (f"INSERT INTO checks VALUES('{label}-{name}',"
            f"NOT EXISTS(SELECT * FROM ({actual}) EXCEPT SELECT * FROM ({oracle})) "
            f"AND NOT EXISTS(SELECT * FROM ({oracle}) EXCEPT SELECT * FROM ({actual})));\n")


VIEWS = [
    ("rows_mv", "SELECT a AS raw,+a AS pos,a+b AS total,a/b AS ratio,a%b AS remv,"
     "-9223372036854775808 AS minv FROM t WHERE enabled=1 AND b IS NOT NULL",
     ["raw", "pos", "total", "ratio", "remv", "minv"]),
    ("group_mv", "SELECT SUM(a+b) AS total,g,COUNT(a/b) AS n FROM t "
     "WHERE enabled=1 AND b IS NOT NULL GROUP BY g", ["total", "g", "n"]),
    ("join_mv", "SELECT t.a+r.x AS total FROM t JOIN r ON t.g=r.g "
     "WHERE t.enabled=1 AND r.active=1", ["total"]),
    ("hidden_mv", "SELECT COUNT(*)/100 AS bucket FROM t GROUP BY g", ["bucket"]),
    ("literal_mv", "SELECT " + ",".join(f"{number} AS f{i}" for i, number in enumerate([
        "1.00000000000000011102230246251565404236316680908203125",
        "1.00000000000000011102230246251565404236316680908203126",
        "0.1000000000000000055511151231257827021181583404541015625",
        "2.2250738585072012e-308", "2.2250738585072014e-308",
        "4.9406564584124654e-324", "1e-320", "9007199254740993.0",
        "1.2345678901234567890123456789012345",
    ])) + " FROM t", [f"f{i}" for i in range(9)]),
]


def checks(label):
    return "".join(parity(label, *view) for view in VIEWS)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sqlite-cli", type=Path, required=True)
    parser.add_argument("--extension", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    cli, extension, output = [p.resolve() for p in [args.sqlite_cli, args.extension, args.output]]
    repo = Path(__file__).resolve().parent.parent
    if output == repo or repo in output.parents:
        parser.error("--output must be outside the source checkout")
    version = subprocess.check_output([str(cli), "-version"], text=True).strip()
    if not version.startswith("3.45.0 "):
        parser.error("This acceptance requires the minimum supported SQLite 3.45.0 CLI")
    output.parent.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="m2d-minimum-", dir=output.parent))
    results = []
    preamble = f'.bail on\n.load "{extension}" sqlite3_ivmlite_init\n'

    def run(tag, database, sql, error=None):
        script = preamble + sql
        (scratch / f"{tag}.sql").write_text(script)
        result = subprocess.run([str(cli), str(database)], input=script, text=True,
                                capture_output=True, timeout=60)
        (scratch / f"{tag}.stdout").write_text(result.stdout)
        (scratch / f"{tag}.stderr").write_text(result.stderr)
        if error is None:
            if result.returncode or result.stderr:
                raise RuntimeError(f"{tag}: {result.stderr}")
        elif result.returncode == 0 or error not in result.stderr:
            raise RuntimeError(f"{tag}: expected failed statement containing {error!r}")
        results.append({"stage": tag, "exitCode": result.returncode,
                        "expectedFailure": error, "passed": True,
                        "sqlChecksExecuted": sql.count("INSERT INTO checks VALUES(")
                        if error is None else 0})
        return result.stdout

    evidence = {"status": "failed", "sqlite": version, "cliSHA256": sha(cli),
                "extensionSHA256": sha(extension), "runnerSHA256": sha(Path(__file__)),
                "scratch": str(scratch), "stages": results}
    try:
        for recursive in [0, 1]:
            db = scratch / f"recursive-{recursive}.sqlite"
            setup = f"PRAGMA recursive_triggers={recursive};\n" + """
CREATE TABLE checks(label TEXT PRIMARY KEY, passed INTEGER NOT NULL CHECK(passed=1));
CREATE TABLE t(id INTEGER PRIMARY KEY,g INTEGER,a BLOB,b BLOB,enabled INTEGER);
CREATE TABLE r(g INTEGER,x INTEGER,active INTEGER);
INSERT INTO t VALUES (1,1,1,2,1),(2,1,1,2,1),(3,1,1.0,2,1),
 (4,1,'2',2,1),(5,1,X'33',2,1),(6,1,NULL,2,1),
 (7,2,9223372036854775807,1,1),(8,3,-9223372036854775808,-1,1),
 (9,4,'2'||char(0)||'tail',0,1),(10,5,X'00ff',1,0);
INSERT INTO r VALUES (1,4,1),(1,4,1),(2,2,1),(3,2,1),(4,2,1),(5,2,0);
"""
            for name, sql, _ in VIEWS:
                setup += f"CREATE VIRTUAL TABLE {name} USING ivm('" + sql.replace("'", "''") + "');\n"
            setup += """
CREATE TABLE ck(id INTEGER PRIMARY KEY,a BLOB,b BLOB);
INSERT INTO ck VALUES(1,1.0,1),(2,1,1.0),(3,1,1.0);
CREATE VIRTUAL TABLE coherent_mv USING ivm('SELECT a/2+b/2 AS x,COUNT(*) AS n FROM ck GROUP BY a,b');
INSERT INTO checks VALUES('coherent-bootstrap',(SELECT count(*) FROM coherent_mv)=1 AND (SELECT x=0.5 AND n=3 FROM coherent_mv));
BEGIN;
DELETE FROM ck WHERE id IN (2,3);
INSERT INTO coherent_mv(coherent_mv) VALUES('refresh');
INSERT INTO checks VALUES('coherent-delete',(SELECT count(*) FROM coherent_mv)=1 AND (SELECT x=0.5 AND n=1 FROM coherent_mv));
ROLLBACK;
INSERT INTO checks VALUES('coherent-rollback',(SELECT count(*) FROM coherent_mv)=1 AND (SELECT x=0.5 AND n=3 FROM coherent_mv));
"""
            setup += checks("bootstrap")
            setup += """
DELETE FROM t WHERE id IN (1,2); UPDATE t SET a=3,enabled=1 WHERE id=10;
DELETE FROM r WHERE rowid=(SELECT min(rowid) FROM r WHERE g=1);
INSERT INTO t VALUES(11,1,1.0,2,1),(12,1,1.0,2,1);
"""
            setup += "".join(f"INSERT INTO {name}({name}) VALUES('refresh');\n" for name, _, _ in VIEWS)
            setup += checks("mixed") + "BEGIN; SAVEPOINT nested; UPDATE t SET a=7 WHERE g=1;\n"
            setup += "".join(f"INSERT INTO {name}({name}) VALUES('refresh');\n" for name, _, _ in VIEWS)
            setup += checks("inside-savepoint") + "ROLLBACK TO nested; RELEASE nested; COMMIT;\n"
            setup += checks("rollback")
            setup += "INSERT INTO t VALUES(13,1,1.0,2,1);\n"
            run(f"r{recursive}-setup", db, setup)
            reopen = f"PRAGMA recursive_triggers={recursive};\n"
            # Pending writes survive reopen and are applied only on refresh.
            reopen += "INSERT INTO checks VALUES('stale-reopen',(SELECT count(*) FROM rows_mv)=10);\n"
            reopen += "INSERT INTO checks VALUES('coherent-reopen',(SELECT count(*) FROM coherent_mv)=1 AND (SELECT x=0.5 AND n=3 FROM coherent_mv));\n"
            reopen += "".join(f"INSERT INTO {name}({name}) VALUES('refresh');\n" for name, _, _ in VIEWS)
            reopen += checks("reopen")
            reopen += """
CREATE TABLE u(k INTEGER,a INTEGER,b INTEGER);
INSERT INTO u VALUES(1,2,3),(2,2,3);
CREATE VIRTUAL TABLE collision_mv USING ivm('SELECT SUM(a+b) AS total FROM u GROUP BY k');
CREATE TABLE saved AS SELECT * FROM __ivm_out_collision_mv;
DELETE FROM __ivm_out_collision_mv WHERE rowid=(SELECT min(rowid) FROM __ivm_out_collision_mv);
DELETE FROM u;
CREATE TABLE before_progress AS SELECT * FROM __ivm_progress;
CREATE TABLE before_deltas AS SELECT * FROM __ivm_delta_u;
CREATE TABLE before_state AS SELECT * FROM __ivm_state_collision_mv_1_agg_groups;
CREATE TABLE before_representatives AS SELECT * FROM __ivm_state_collision_mv_1_agg_representatives;
"""
            run(f"r{recursive}-reopen", db, reopen)
            run(f"r{recursive}-underflow", db,
                f"PRAGMA recursive_triggers={recursive}; INSERT INTO collision_mv(collision_mv) VALUES('refresh');\n",
                "retraction exceeds stored copies")
            retry = f"PRAGMA recursive_triggers={recursive};\n" + """
INSERT INTO checks VALUES('underflow-output',(SELECT count(*) FROM collision_mv)=1);
INSERT INTO checks VALUES('underflow-progress',
 NOT EXISTS(SELECT * FROM __ivm_progress EXCEPT SELECT * FROM before_progress)
 AND NOT EXISTS(SELECT * FROM before_progress EXCEPT SELECT * FROM __ivm_progress));
INSERT INTO checks VALUES('underflow-deltas',
 NOT EXISTS(SELECT * FROM __ivm_delta_u EXCEPT SELECT * FROM before_deltas)
 AND NOT EXISTS(SELECT * FROM before_deltas EXCEPT SELECT * FROM __ivm_delta_u));
INSERT INTO checks VALUES('underflow-state',
 NOT EXISTS(SELECT * FROM __ivm_state_collision_mv_1_agg_groups EXCEPT SELECT * FROM before_state)
 AND NOT EXISTS(SELECT * FROM before_state EXCEPT SELECT * FROM __ivm_state_collision_mv_1_agg_groups));
INSERT INTO checks VALUES('underflow-representatives',
 NOT EXISTS(SELECT * FROM __ivm_state_collision_mv_1_agg_representatives EXCEPT SELECT * FROM before_representatives)
 AND NOT EXISTS(SELECT * FROM before_representatives EXCEPT SELECT * FROM __ivm_state_collision_mv_1_agg_representatives));
DELETE FROM __ivm_out_collision_mv; INSERT INTO __ivm_out_collision_mv SELECT * FROM saved;
INSERT INTO collision_mv(collision_mv) VALUES('refresh');
INSERT INTO checks VALUES('underflow-retry',(SELECT count(*) FROM collision_mv)=0);
DROP TABLE collision_mv;
DROP TABLE coherent_mv;
"""
            retry += checks("after-retry")
            retry += "".join(f"DROP TABLE {name};\n" for name, _, _ in VIEWS)
            retry += "INSERT INTO checks VALUES('drop-cleanup',NOT EXISTS(SELECT 1 FROM sqlite_schema WHERE name LIKE '__ivm_%'));\n"
            retry += "SELECT count(*) FROM checks;\n"
            count = int(run(f"r{recursive}-retry", db, retry).strip())
            # Assertions executed inside the savepoint/transaction still fail
            # immediately under .bail on, although ROLLBACK removes their rows.
            if count != len(VIEWS) * 5 + 11:
                raise RuntimeError(f"Incomplete acceptance: {count} committed SQL checks")
            executed = sum(stage["sqlChecksExecuted"] for stage in results
                           if stage["stage"].startswith(f"r{recursive}-"))
            if executed != len(VIEWS) * 6 + 12:
                raise RuntimeError(f"Incomplete acceptance: {executed} executed SQL checks")
            evidence[f"recursive{recursive}Checks"] = executed
            evidence[f"recursive{recursive}CommittedCheckRows"] = count
        evidence["status"] = "passed"
    except Exception as failure:
        evidence["error"] = f"{type(failure).__name__}: {failure}"
        raise
    finally:
        output.write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(evidence))


if __name__ == "__main__":
    main()
