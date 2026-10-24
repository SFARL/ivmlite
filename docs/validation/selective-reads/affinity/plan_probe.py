#!/usr/bin/env python3
"""Run with: python3 plan_probe.py > expression-plans.json."""
import json
import sqlite3

connection = sqlite3.connect(':memory:')
queries = [
    ('column', "SELECT * FROM json_each WHERE json='[1]'"),
    ('unary_plus', "SELECT * FROM json_each WHERE +json='[1]'"),
    ('cast', "SELECT * FROM json_each WHERE CAST(json AS TEXT)='[1]'"),
    ('collate', "SELECT * FROM json_each WHERE json COLLATE BINARY='[1]'"),
    ('row_value', "SELECT * FROM json_each WHERE (json,root)=('[1]','$')"),
    ('transitive', "SELECT * FROM json_each CROSS JOIN "
                   "(SELECT '[1]' AS j) AS r WHERE json=r.j AND r.j='[1]'"),
]
print(json.dumps(dict(
    sqlite_version=sqlite3.sqlite_version,
    results={name: dict(sql=sql, plan=connection.execute(
        'EXPLAIN QUERY PLAN ' + sql).fetchall()) for name, sql in queries}),
    indent=2))
