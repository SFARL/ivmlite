SELECT 'reopened',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM totals ORDER BY k;
SELECT 'reopened_full',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM (SELECT k,COUNT(*) AS n,SUM(v) AS s FROM orders GROUP BY k) ORDER BY k;
UPDATE orders SET v=11 WHERE id=1;
 INSERT INTO orders VALUES(3,'a',2);
SELECT 'post_reopen_dml',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM totals ORDER BY k;
SELECT 'post_reopen_dml_full',quote(k),n,quote(s),typeof(s),CAST(s AS INTEGER) FROM (SELECT k,COUNT(*) AS n,SUM(v) AS s FROM orders GROUP BY k) ORDER BY k;
