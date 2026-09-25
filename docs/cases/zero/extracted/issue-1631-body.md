We are using the `begin concurrent` branch (https://www.sqlite.org/src/vdiff?branch=begin-concurrent -- see also https://www.sqlite.org/cgi/src/doc/begin-concurrent/doc/begin_concurrent.md) of sqlite for https://zerosync.dev.

We use it in kind of an interesting way: we don't ever commit the concurrent transactions. Instead, we open concurrent write transactions, read and write data to them, then rollback the transaction.

The reason we want to do this is because to accomplish its work, zero needs to maintain materialized views of queries from the client. While cheaper than running each client query from scratch, maintaining these views can still be expensive so we want to do it on multiple threads.

The algorithm we have developed to maintain these views requires a series of interactive write/read steps. For an incoming write transaction, we write a row to the datastore (sqlite), do some queries, write another row, do some queries, etc. At the end of these series of steps we have the output that we send to the client. We cannot do all the writes up front - the correctness of the algorithm requires the write/read interleaving.

If we run this algorithm on multiple thread in parallel, the writes will take eager locks and conflict with each other. But `begin concurrent` give us an awesome out: we can open transactions in parallel, do our write/read pairs, then abort the transactions. Then we just have a single 'writer' transaction that actually flushes the persistent writes to the db at the end.

Unfortunately we have to use this kind of semi-zombie branch of sqlite to accomplish this. It would be super useful to have it officially supported in libsql.