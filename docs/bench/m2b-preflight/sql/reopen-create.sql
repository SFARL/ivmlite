CREATE TABLE orders(id INTEGER PRIMARY KEY,k TEXT,v INTEGER) STRICT;
INSERT INTO orders VALUES(1,'a',3),(2,'b',5);
CREATE MATERIALIZED VIEW totals AS SELECT k,COUNT(*) AS n,SUM(v) AS s FROM orders GROUP BY k;
SELECT 'created',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM totals ORDER BY k;
SELECT 'created_full',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM (SELECT k,COUNT(*) AS n,SUM(v) AS s FROM orders GROUP BY k) ORDER BY k;
