CREATE TABLE orders(id INTEGER PRIMARY KEY,k TEXT,v INTEGER) STRICT;
INSERT INTO orders VALUES(1,'exact',9007199254740992),(2,'inexact',9007199254740993);
CREATE MATERIALIZED VIEW totals AS SELECT k,COUNT(*) AS n,SUM(v) AS s FROM orders GROUP BY k;
SELECT 'bootstrap',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM totals ORDER BY k;
SELECT 'bootstrap_full',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM (SELECT k,COUNT(*) AS n,SUM(v) AS s FROM orders GROUP BY k) ORDER BY k;
INSERT INTO orders VALUES(3,'increment',9007199254740992);
 INSERT INTO orders VALUES(4,'increment',1);
SELECT 'increment',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM totals ORDER BY k;
SELECT 'increment_full',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM (SELECT k,COUNT(*) AS n,SUM(v) AS s FROM orders GROUP BY k) ORDER BY k;
