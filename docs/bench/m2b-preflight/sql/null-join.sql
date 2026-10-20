CREATE TABLE l(k INTEGER,v INTEGER) STRICT;
 CREATE TABLE r(k INTEGER,label TEXT) STRICT;
 INSERT INTO l VALUES(1,3),(NULL,9);
 INSERT INTO r VALUES(1,'match'),(NULL,'null-side');
 CREATE MATERIALIZED VIEW m AS SELECT r.label,COUNT(*) AS n,SUM(l.v) AS s FROM l JOIN r ON l.k=r.k GROUP BY r.label;
 SELECT 'mv',* FROM m ORDER BY label;
 SELECT 'full',r.label,COUNT(*),SUM(l.v) FROM l JOIN r ON l.k=r.k GROUP BY r.label ORDER BY r.label;
