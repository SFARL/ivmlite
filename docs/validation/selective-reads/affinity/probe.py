#!/usr/bin/env python3
"""Read-only-to-repository SQLite affinity candidate-superset probe.

Run: python3 probe.py > results.json

Each RHS table is populated without affinity before its in-memory declaration
is changed. This makes declared affinity independent of storage class, as may
occur with foreign virtual tables or mixed-affinity compound SELECT outputs.
Automatic indexes are disabled: rewriting an existing REAL declaration does
not establish valid ordinary-table REAL storage invariants for index seeks.
This probe checks scalar SQLite comparisons, not virtual-table pushdown itself.
No persistent database, extension, repository edit, or library build is used.
"""

import json
import math
import random
import sqlite3
import sys


def pool_values():
    rng = random.Random(103)
    pool = [None, 0, 1, -1, 42, -(2**63), 2**63 - 1, 0.0, -0.0,
            0.5, -0.5, 1.0, math.nextafter(1.0, 2.0),
            math.nextafter(1.0, 0.0), math.inf, -math.inf, 1e-323,
            math.nextafter(float(2**63), 0.0)]
    texts = ['', '0', '-0', '+0', '00', '01', '1.0', '1e0', '42',
             '42.0', '042', ' 42 ', '42x', 'x42', '1_0', '0x10',
             'NaN', 'Inf', '1e9999', '-1e9999', '1e-9999', '-1e-9999',
             '1\x00', '\x001', '1e2\x00', '\t\r\n1\v\f', '1.', ' .1 ',
             '1e+', '--1', '1e309', '1e-324', '5e-324']
    for n in (2**51, 2**53, 2**63, -(2**51), -(2**53), -(2**63)):
        for delta in range(-3, 4):
            value = n + delta
            if -(2**63) <= value < 2**63:
                pool.append(value)
            pool.append(float(value))
            texts.extend(str(value) + suffix
                         for suffix in ('', '.0', '.00000000000001', 'e0'))
    for _ in range(60):
        texts.append(rng.choice(['', '-', '+'])
                     + ''.join(rng.choice('0123456789')
                               for _ in range(rng.randrange(1, 24)))
                     + rng.choice(['', '.0', 'e0', 'e-2', 'e2', 'e309',
                                   'e-324', 'x']))
    for length in (64, 512, 4096):
        texts.extend(['0' * length + '1', '9' * length,
                      '0.' + '0' * length + '1', '1e' + '9' * length,
                      '1' + ' ' * length])
    return pool + texts + [b'', b'0', b'1', b'42', b'\x00', b'\xff',
                           b'01', b'1\x00']


def cast(expression, affinity):
    return f'+CAST({expression} AS {affinity})'


def generic_seven(operator, rhs):
    lhs = 'l.g'
    combinations = [
        (lhs, rhs),
        (cast(lhs, 'NUMERIC'), rhs),
        (lhs, cast(rhs, 'NUMERIC')),
        (cast(lhs, 'NUMERIC'), cast(rhs, 'NUMERIC')),
        (cast(lhs, 'TEXT'), rhs),
        (lhs, cast(rhs, 'TEXT')),
        (cast(lhs, 'TEXT'), cast(rhs, 'TEXT')),
    ]
    return '(' + ' OR '.join(f'{a} {operator} {b}'
                             for a, b in combinations) + ')'


def numeric_three(operator, rhs):
    return (f'(l.g {operator} {rhs}'
            f' OR {cast("l.g", "NUMERIC")} {operator} {rhs}'
            f' OR {cast("l.g", "TEXT")} {operator} {cast(rhs, "TEXT")})')


def main():
    connection = sqlite3.connect(':memory:')
    connection.execute('PRAGMA automatic_index=OFF')
    pool = pool_values()
    connection.execute('CREATE TABLE l(g)')
    connection.executemany('INSERT INTO l VALUES (?)', ((v,) for v in pool))
    classes = dict(connection.execute('SELECT typeof(g),count(*) FROM l '
                                      'GROUP BY typeof(g)'))
    results = []
    for affinity in ('BLOB', 'NUMERIC', 'INTEGER', 'REAL', 'TEXT'):
        table = 'r_' + affinity.lower()
        connection.execute(f'CREATE TABLE {table}(y)')
        connection.executemany(f'INSERT INTO {table} VALUES (?)',
                               ((v,) for v in pool))
        connection.execute('PRAGMA writable_schema=ON')
        connection.execute('UPDATE sqlite_schema SET sql=? WHERE name=?',
                           (f'CREATE TABLE {table}(y {affinity})', table))
        connection.execute('PRAGMA writable_schema=RESET')
        for operator in ('=', '>', '>=', '<', '<='):
            # Unary plus is essential: the candidate models a bound value
            # with no expression affinity, while the original keeps affinity.
            rhs = f'+{table}.y'
            candidates = [
                ('generic7_all_rhs', generic_seven(operator, rhs), '1'),
                ('numeric3', numeric_three(operator, rhs),
                 f"typeof({rhs}) IN ('integer','real')"),
                ('blob_direct', f'l.g {operator} {rhs}',
                 f"typeof({rhs})='blob'"),
                ('null_false', f'({rhs} IS NOT NULL AND 0)',
                 f"typeof({rhs})='null'"),
            ]
            if operator == '=':
                candidates.append((
                    'integer_eq2',
                    f"(l.g={rhs} OR (l.g>='' AND l.g<x''"
                    f" AND {cast('l.g', 'NUMERIC')}={rhs}))",
                    f"typeof({rhs})='integer'"))
            candidates.extend([
                ('blob_lhs_numeric2',
                 f"(l.g {operator} {rhs} OR (l.g>='' AND l.g<x''"
                 f" AND {cast('l.g', 'NUMERIC')} {operator} {rhs}))",
                 f"typeof({rhs}) IN ('integer','real')"),
                ('blob_lhs_text4',
                 '(' + ' OR '.join(
                     f'{a} {operator} {b}' for a, b in [
                         ('l.g', rhs), (cast('l.g', 'NUMERIC'), rhs),
                         ('l.g', cast(rhs, 'NUMERIC')),
                         (cast('l.g', 'NUMERIC'), cast(rhs, 'NUMERIC'))]) + ')',
                 f"typeof({rhs})='text'"),
            ])
            if operator in ('>', '>='):
                candidates.append(('blob_lhs_numeric_gt_ge_direct',
                                   f'l.g {operator} {rhs}',
                                   f"typeof({rhs}) IN ('integer','real')"))
            # Untyped table columns have BLOB affinity; unary-plus columns
            # have genuinely no affinity. The latter also exercises TEXT
            # comparison coercion, making the check conservatively stronger.
            expanded = [(mode, lhs, name, candidate, guard)
                        for mode, lhs in [('typeless_column', 'l.g'),
                                          ('no_affinity_expression', '+l.g')]
                        for name, candidate, guard in candidates]
            for mode, original_lhs, name, candidate, guard in expanded:
                if name.startswith('blob_lhs_') and mode != 'typeless_column':
                    continue
                original = f'{original_lhs} {operator} {table}.y'
                query = (
                    f'SELECT count(*), '
                    f'coalesce(sum(coalesce(({original}),0)!=0),0), '
                    f'coalesce(sum(coalesce(({original}),0)!=0 AND '
                    f'coalesce(({candidate}),0)=0),0) '
                    f'FROM l CROSS JOIN {table} WHERE {guard}')
                pairs, matches, misses = connection.execute(query).fetchone()
                item = dict(lhs_mode=mode, affinity=affinity, operator=operator,
                            candidate=name, eligible_pairs=pairs,
                            original_matches=matches, false_negatives=misses)
                if misses:
                    item['examples'] = connection.execute(
                        f'SELECT quote(l.g),typeof(l.g),quote({rhs}),'
                        f'typeof({rhs}) FROM l CROSS JOIN {table} '
                        f'WHERE ({guard}) AND ({original}) AND NOT '
                        f'coalesce(({candidate}),0) LIMIT 3').fetchall()
                results.append(item)
    totals = {}
    for result in results:
        name = result['candidate']
        aggregate = totals.setdefault(name, dict(queries=0, eligible_pairs=0,
                                                original_matches=0,
                                                false_negatives=0))
        aggregate['queries'] += 1
        for key in ('eligible_pairs', 'original_matches', 'false_negatives'):
            aggregate[key] += result[key]
    output = dict(sqlite_version=sqlite3.sqlite_version,
                  sqlite_source_id=connection.execute(
                      'SELECT sqlite_source_id()').fetchone()[0],
                  python_version=sys.version,
                  automatic_index=False, random_seed=103,
                  pool_count=len(pool), pool_storage_classes=classes,
                  pool_shape='i64/2^51/2^53 boundaries and adjacent values; '
                             'REAL infinities, subnormals, signed zero; '
                             'valid/malformed numeric text, embedded NUL, '
                             'whitespace, 64/512/4096-length parser cases; '
                             'empty/nonempty BLOB; NULL',
                  totals=totals, results=results)
    print(json.dumps(output, indent=2))
    return int(any(item['false_negatives'] for item in results))


if __name__ == '__main__':
    raise SystemExit(main())
