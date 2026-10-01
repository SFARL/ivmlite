# M1b Phase 4 cost spike (2026-09-28)

A throwaway measurement taken before Phase 4 was designed, kept so its numbers can be reproduced. It motivates the two fixes in the Phase 4 spec (§2). It is **not** Phase 4's before/after data; the ablation run through `ivmlite-bench` is (Phase 4 spec §6).

**Setup:** macOS, Apple Silicon. Python 3.13 with its bundled SQLite 3.53.0. The extension built with `cargo build --release --manifest-path crates/ivmlite-sqlite/Cargo.toml` at `63f15f0`. An in-memory database. Every cell ran once.

**To reproduce:** build the release extension, then run each script with `python3 <script>`. `LIB` must point at `crates/ivmlite-sqlite/target/release/libivmlite_sqlite` (without the file extension).

## Script 1: refresh against full recompute

```python
# THROWAWAY spike: order-of-magnitude costs of the ivmlite extension on the m0 workload shape.
import sqlite3, time, random, sys
LIB = "crates/ivmlite-sqlite/target/release/libivmlite_sqlite"
def run(base_rows, card, views, batch, ext=True):
    c = sqlite3.connect(":memory:", isolation_level=None)
    c.enable_load_extension(True); c.load_extension(LIB, entrypoint="sqlite3_ivmlite_init")
    c.execute("CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT")
    r = random.Random(1)
    c.execute("BEGIN"); c.executemany("INSERT INTO orders VALUES (?,?,?)", ((i, f"r{r.randrange(card)}", r.randrange(200)) for i in range(base_rows))); c.execute("COMMIT")
    t = time.perf_counter()
    if ext:
        for v in range(views):
            c.execute(f"CREATE VIRTUAL TABLE mv_{v} USING ivm('SELECT region, SUM(amount), COUNT(*) FROM orders WHERE amount > {(v*7)%150} GROUP BY region')")
    boot = time.perf_counter() - t
    nd = batch // 3
    dels = r.sample(range(base_rows), nd)
    ins = [(base_rows + i, f"r{r.randrange(card)}", r.randrange(200)) for i in range(batch - nd)]
    t = time.perf_counter()
    c.execute("BEGIN")
    c.executemany("DELETE FROM orders WHERE id = ?", ((d,) for d in dels))
    c.executemany("INSERT INTO orders VALUES (?,?,?)", ins)
    c.execute("COMMIT")
    apply = time.perf_counter() - t
    t = time.perf_counter()
    if ext:
        for v in range(views): c.execute(f"INSERT INTO mv_{v}(mv_{v}) VALUES ('refresh')")
    refresh = time.perf_counter() - t
    t = time.perf_counter()
    for v in range(views): c.execute(f"SELECT region, SUM(amount), COUNT(*) FROM orders WHERE amount > {(v*7)%150} GROUP BY region").fetchall()
    recompute = time.perf_counter() - t
    return boot, apply, refresh, recompute
print("base card views batch | boot_s apply_ms refresh_ms recompute_ms | apply_noext_ms")
for (b, card, views, batch) in [(100000,1000,10,100),(100000,1000,10,1000),(100000,100000,10,1000),(1000000,1000,10,1000),(1000000,100000,10,1000),(100000,1000,200,100),(1000000,10,10,1)]:
    boot, apply, ref, rec = run(b, card, views, batch)
    _, apply0, _, _ = run(b, card, views, batch, ext=False)
    print(f"{b} {card} {views} {batch} | {boot:.2f} {apply*1e3:.1f} {ref*1e3:.1f} {rec*1e3:.1f} | {apply0*1e3:.1f}", flush=True)
```

Output:

```
base card views batch | boot_s apply_ms refresh_ms recompute_ms | apply_noext_ms
100000 1000 10 100 | 1.07 1.1 39.7 266.5 | 0.1
100000 1000 10 1000 | 1.07 10.1 164.3 263.4 | 0.7
100000 100000 10 1000 | 5.16 10.1 11056.1 689.9 | 0.8
1000000 1000 10 1000 | 10.58 10.4 171.5 2783.6 | 1.0
1000000 100000 10 1000 | 24.08 10.9 27364.6 3914.2 | 1.0
100000 1000 200 100 | 19.21 6.9 929.5 3996.8 | 0.1
1000000 10 10 1 | 7.84 0.2 3.0 2480.7 | 0.0

```

The columns are: bootstrap seconds; then apply, refresh-all and recompute-all in milliseconds; then apply without the extension, in milliseconds.

## Script 2: fixed refresh overhead, and write cost by number of views

```python
# THROWAWAY: split refresh cost into fixed overhead vs per-delta work
import sqlite3, time, random
LIB = "crates/ivmlite-sqlite/target/release/libivmlite_sqlite"
def setup(base_rows, card, views):
    c = sqlite3.connect(":memory:", isolation_level=None)
    c.enable_load_extension(True); c.load_extension(LIB, entrypoint="sqlite3_ivmlite_init")
    c.execute("CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT")
    r = random.Random(1)
    c.execute("BEGIN"); c.executemany("INSERT INTO orders VALUES (?,?,?)", ((i, f"r{r.randrange(card)}", r.randrange(200)) for i in range(base_rows))); c.execute("COMMIT")
    for v in range(views):
        c.execute(f"CREATE VIRTUAL TABLE mv_{v} USING ivm('SELECT region, SUM(amount), COUNT(*) FROM orders WHERE amount > {(v*7)%150} GROUP BY region')")
    return c, r
def refresh_all(c, views):
    t = time.perf_counter()
    for v in range(views): c.execute(f"INSERT INTO mv_{v}(mv_{v}) VALUES ('refresh')")
    return (time.perf_counter() - t) * 1e3 / views
for card in [10, 1000, 100000]:
    c, r = setup(100000, card, 10)
    empty = refresh_all(c, 10)
    c.execute("BEGIN"); c.executemany("INSERT INTO orders VALUES (?,?,?)", ((200000+i, f"r{r.randrange(card)}", r.randrange(200)) for i in range(1))); c.execute("COMMIT")
    one = refresh_all(c, 10)
    c.execute("BEGIN"); c.executemany("INSERT INTO orders VALUES (?,?,?)", ((300000+i, f"r{r.randrange(card)}", r.randrange(200)) for i in range(100))); c.execute("COMMIT")
    hundred = refresh_all(c, 10)
    print(f"card={card}: per-view refresh ms — empty {empty:.2f}, 1-row insert {one:.2f}, 100-row insert {hundred:.2f}", flush=True)
# write cost per row by view count
for views in [0, 1, 10, 50]:
    c, r = setup(10000, 1000, views)
    n_schema = c.execute("SELECT count(*) FROM sqlite_schema").fetchone()[0]
    t = time.perf_counter(); c.execute("BEGIN")
    c.executemany("INSERT INTO orders VALUES (?,?,?)", ((100000+i, f"r{r.randrange(1000)}", r.randrange(200)) for i in range(2000))); c.execute("COMMIT")
    print(f"views={views} schema_rows={n_schema}: insert us/row {(time.perf_counter()-t)*1e6/2000:.1f}", flush=True)
```

Output:

```
card=10: per-view refresh ms — empty 0.20, 1-row insert 0.26, 100-row insert 0.43
card=1000: per-view refresh ms — empty 0.21, 1-row insert 0.30, 100-row insert 3.84
card=100000: per-view refresh ms — empty 0.75, 1-row insert 1.54, 100-row insert 95.87
views=0 schema_rows=1: insert us/row 1.0
views=1 schema_rows=26: insert us/row 10.1
views=10 schema_rows=71: insert us/row 14.2
views=50 schema_rows=271: insert us/row 32.1
```
