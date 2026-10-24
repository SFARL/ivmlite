# SQLite compatibility records

A native SQLite **3.45.0** CLI on macOS arm64 loaded the selective-read extension
and passed **74 smoke checks plus 4 alpha.6 database-reopen checks**, with zero
failures. `smoke.json` records the CLI source ID and tested library SHA256.
The smoke covers seven RHS forms and five operators in both EXCEPT directions,
a combined key/range query, refresh, rollback, and invalid UTF-8 TEXT. The reopen
checks cover full count, bound read, refresh, and rollback of an older database.
These are bounded fixtures, not exhaustive 3.45 affinity coverage, production
validation, Linux validation, or support for every SQLite version.

The scalar candidate pool in `../affinity/` ran on **Python SQLite 3.53.0**;
those millions of ordered pairs did not run through an ivmlite virtual table.
The Rust integration suite uses **SQLite 3.53.2** and is separate evidence.
Do not combine the three runtimes into a claim that the full affinity pool ran
on the minimum supported runtime.

The original older database was created with Python SQLite 3.53.0 using the
old extension built at `4888480`; that commit differs from alpha.6 commit
`0ad47a9259ee7acee411b46b7f64d50d268b2096` only in documentation. The original
old-library, database and new-library hashes are in `provenance.json`, alongside
the original and publication SHA256 of every retained captured file. Binaries,
databases, archives and full upstream C sources are omitted.

## Reproduce in a scratch directory

Requirements: Python 3, a C compiler, curl, unzip, and two independently built
ivmlite libraries for your platform: alpha.6 at the pinned commit above, and the
selective-read source under review. Build each separately with
`cargo build --locked --manifest-path crates/ivmlite-sqlite/Cargo.toml` in its
own checkout, and retain the output before another build. The crate uses its
own target directory. macOS outputs `libivmlite_sqlite.dylib`; Linux outputs
`libivmlite_sqlite.so`. The recorded native execution was macOS arm64 only.

Set `PUBLISHED_COMPATIBILITY` to this published directory and both library
variables to absolute paths. Work outside the checkout. The runner expects a
CLI named `sqlite3` beside itself and a sibling `baseline/alpha6.sqlite3`.

```sh
export PUBLISHED_COMPATIBILITY=/absolute/path/to/compatibility
export IVMLITE_OLD_EXTENSION=/absolute/path/to/alpha6/libivmlite_sqlite.dylib
export IVMLITE_EXTENSION=/absolute/path/to/new/libivmlite_sqlite.dylib
scratch=$(mktemp -d)
cp -R "$PUBLISHED_COMPATIBILITY" "$scratch/compatibility"
mkdir "$scratch/baseline"
cd "$scratch"
curl -fL https://www.sqlite.org/2024/sqlite-amalgamation-3450000.zip -o sqlite.zip
python3 - <<'PYVERIFY'
import hashlib
from pathlib import Path
assert hashlib.sha256(Path('sqlite.zip').read_bytes()).hexdigest() == 'bde30d13ebdf84926ddd5e8b6df145be03a577a48fd075a087a5dd815bcdf740'
PYVERIFY
unzip -q sqlite.zip
python3 - <<'PYVERIFY'
import hashlib
from pathlib import Path
assert hashlib.sha3_256(Path('sqlite-amalgamation-3450000/sqlite3.c').read_bytes()).hexdigest() == 'f56d8e5e8c61d87b957f1cc60b3042c134d7bc0ca3aba002e6999e8f0af310a3'
PYVERIFY
cc -O2 sqlite-amalgamation-3450000/shell.c sqlite-amalgamation-3450000/sqlite3.c -ldl -lpthread -lm -o compatibility/sqlite3
{ printf ".load '%s' sqlite3_ivmlite_init\n" "$IVMLITE_OLD_EXTENSION"; cat compatibility/create-alpha6.sql; } | compatibility/sqlite3 baseline/alpha6.sqlite3
python3 compatibility/smoke.py
cat compatibility/smoke.json
```

The [official 3.45.0 release record](https://sqlite.org/releaselog/3_45_0.html)
publishes source ID `1066602b2b1976fe58b5150777cced894af17c803e068f5918390d6915b46e1d`
and SHA3-256 **of sqlite3.c** `f56d8e5e8c61d87b957f1cc60b3042c134d7bc0ca3aba002e6999e8f0af310a3`.
That SHA3 is not the ZIP digest. The C compilation command above is a portable
reproduction recipe; historical compiler flags were not retained. A regenerated
fixture is not claimed to match the original database bytes. Its exact schema
and seed rows came from read-only inspection of the original database.

`smoke.py` retains executed logic except that `IVMLITE_EXTENSION` is mandatory.
The runner writes SQL, stdout, stderr, its copied compatibility database, and
`smoke.json` into its own directory, so use the scratch copy. Captured SQL has
only its machine-specific `.load` path normalized to `./ivmlite-extension`;
the runner regenerates that line from the environment. Captured empty stderr
files are deliberate. `SHA256SUMS` binds publication files; `provenance.json`
separately binds their exact raw originals and states each normalization.
The new publication wrappers and reproduction commands were statically checked;
they have not been rerun as another native campaign.
