DROP VIEW totals;
 SELECT type,name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name;
 INSERT INTO orders VALUES(4,'after_drop',1);
