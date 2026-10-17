CREATE TABLE l(id INTEGER PRIMARY KEY,k INTEGER,v INTEGER) STRICT;

CREATE TABLE r(id INTEGER PRIMARY KEY,k INTEGER,label TEXT) STRICT;

INSERT INTO l VALUES(1,1,3),(2,1,3);

INSERT INTO r VALUES(1,1,'a'),(2,1,'a');

CREATE MATERIALIZED VIEW totals AS SELECT r.label AS k,COUNT(*) AS n,SUM(l.v) AS s FROM l JOIN r ON l.k=r.k GROUP BY r.label;
SELECT 'bootstrap',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM totals ORDER BY k;
SELECT 'bootstrap_full',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM (SELECT r.label AS k,COUNT(*) AS n,SUM(l.v) AS s FROM l JOIN r ON l.k=r.k GROUP BY r.label) ORDER BY k;
UPDATE l SET v=5 WHERE id=1;
SELECT 'left_update',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM totals ORDER BY k;
SELECT 'left_update_full',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM (SELECT r.label AS k,COUNT(*) AS n,SUM(l.v) AS s FROM l JOIN r ON l.k=r.k GROUP BY r.label) ORDER BY k;
UPDATE r SET label='b' WHERE id=1;
SELECT 'right_update',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM totals ORDER BY k;
SELECT 'right_update_full',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM (SELECT r.label AS k,COUNT(*) AS n,SUM(l.v) AS s FROM l JOIN r ON l.k=r.k GROUP BY r.label) ORDER BY k;
UPDATE l SET k=2 WHERE id=1;
SELECT 'left_key_update',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM totals ORDER BY k;
SELECT 'left_key_update_full',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM (SELECT r.label AS k,COUNT(*) AS n,SUM(l.v) AS s FROM l JOIN r ON l.k=r.k GROUP BY r.label) ORDER BY k;
UPDATE r SET k=2 WHERE id=1;
SELECT 'right_key_update',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM totals ORDER BY k;
SELECT 'right_key_update_full',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM (SELECT r.label AS k,COUNT(*) AS n,SUM(l.v) AS s FROM l JOIN r ON l.k=r.k GROUP BY r.label) ORDER BY k;
DELETE FROM r WHERE id=2;
SELECT 'delete_fanout',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM totals ORDER BY k;
SELECT 'delete_fanout_full',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM (SELECT r.label AS k,COUNT(*) AS n,SUM(l.v) AS s FROM l JOIN r ON l.k=r.k GROUP BY r.label) ORDER BY k;
