Hey,
I have a database that I save in HTML from couple of web scrapers. There are around 200k+, 50+ rows in a couple of tables, with sqlite file weighing around 600MB.

The app runs on a VPS with 2 core CPU, 4GB RAM and refreshing database page regularly takes more than 10 seconds. I was suspecting that counting tables was the culprit, but manually running `select count(*) from table_name` for the largest table finishes under a second.

I've looked at the source code. There's a check for index page for mutable databases larger than 100MB
https://github.com/simonw/datasette/blob/799c5d53570d773203527f19530cf772dc2eeb24/datasette/views/index.py#L15

but this check is not performed for database page. 
I've manually crippled `Database::table_counts` method
```py
async def table_counts(self, limit=10):
    if not self.is_mutable and self.cached_table_counts is not None:
        return self.cached_table_counts
    # Try to get counts for each table, $limit timeout for each count
    counts = {}
    for table in await self.table_names():
        try:
            # table_count = (
            #     await self.execute(
            #         "select count(*) from [{}]".format(table),
            #         custom_time_limit=limit,
            #     )
            # ).rows[0][0]
            counts[table] = 10 # table_count
        # In some cases I saw "SQL Logic Error" here in addition to
        # QueryInterrupted - so we catch that too:
        except (QueryInterrupted, sqlite3.OperationalError, sqlite3.DatabaseError):
            counts[table] = None
    if not self.is_mutable:
        self.cached_table_counts = counts
    return counts
```

now the page loads in <100ms.

Is it possible to apply size check on database page too?

<details>
<summary>
/-/versions output
</summary>
<pre>
{
    "python": {
        "version": "3.8.0",
        "full": "3.8.0 (default, Oct 28 2019, 16:14:01) \n[GCC 8.3.0]"
    },
    "datasette": {
        "version": "0.44"
    },
    "asgi": "3.0",
    "uvicorn": "0.11.5",
    "sqlite": {
        "version": "3.22.0",
        "fts_versions": [
            "FTS5",
            "FTS4",
            "FTS3"
        ],
        "extensions": {
            "json1": null
        },
        "compile_options": [
            "COMPILER=gcc-7.4.0",
            "ENABLE_COLUMN_METADATA",
            "ENABLE_DBSTAT_VTAB",
            "ENABLE_FTS3",
            "ENABLE_FTS3_PARENTHESIS",
            "ENABLE_FTS3_TOKENIZER",
            "ENABLE_FTS4",
            "ENABLE_FTS5",
            "ENABLE_JSON1",
            "ENABLE_LOAD_EXTENSION",
            "ENABLE_PREUPDATE_HOOK",
            "ENABLE_RTREE",
            "ENABLE_SESSION",
            "ENABLE_STMTVTAB",
            "ENABLE_UNLOCK_NOTIFY",
            "ENABLE_UPDATE_DELETE_LIMIT",
            "HAVE_ISNAN",
            "LIKE_DOESNT_MATCH_BLOBS",
            "MAX_SCHEMA_RETRY=25",
            "MAX_VARIABLE_NUMBER=250000",
            "OMIT_LOOKASIDE",
            "SECURE_DELETE",
            "SOUNDEX",
            "TEMP_STORE=1",
            "THREADSAFE=1"
        ]
    }
}
</pre>
</details>