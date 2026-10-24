.bail on
.load ./ivmlite-extension sqlite3_ivmlite_init
.mode list
.separator |
SELECT 'reopen_count',(SELECT COUNT(*)-4 FROM roll);
SELECT 'reopen_bound',(SELECT SUM("SUM(v)")-8 FROM roll WHERE g=42);
BEGIN;
INSERT INTO facts VALUES(5,42,1,11);
INSERT INTO roll(roll) VALUES('refresh');
SELECT 'reopen_refresh',(SELECT "SUM(v)"-14 FROM roll WHERE g=42 AND t=1);
ROLLBACK;
SELECT 'reopen_rollback',(SELECT "SUM(v)"-3 FROM roll WHERE g=42 AND t=1);
