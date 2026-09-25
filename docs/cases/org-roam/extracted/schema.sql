-- Original Tables

CREATE TABLE files (
file UNIQUE PRIMARY KEY,
title,
hash NOT NULL, 
atime NOT NULL,
mtime NOT NULL
);

CREATE TABLE nodes (
id NOT NULL PRIMARY KEY,
file NOT NULL,
level NOT NULL,
pos NOT NULL,
todo,
priority,
scheduled TEXT,
deadline TEXT,
title,
properties,
olp,
FOREIGN KEY (file) REFERENCES files (file) ON DELETE CASCADE
);

CREATE TABLE aliases (
node_id NOT NULL,
alias,
FOREIGN KEY (node_id) REFERENCES nodes (id) ON DELETE CASCADE
);

CREATE TABLE citations (
node_id NOT NULL,
cite_key NOT NULL,
pos NOT NULL,
properties ,
FOREIGN KEY (node_id) REFERENCES nodes (id) ON DELETE CASCADE
);

CREATE TABLE refs (
node_id NOT NULL,
ref NOT NULL,
type NOT NULL,
FOREIGN KEY (node_id) REFERENCES nodes (id) ON DELETE CASCADE
);

CREATE TABLE tags (
node_id NOT NULL, tag , FOREIGN KEY (node_id) REFERENCES nodes (id) ON DELETE CASCADE
);

CREATE TABLE links (
pos NOT NULL, source NOT NULL, dest NOT NULL, type NOT NULL, properties NOT NULL, FOREIGN KEY (source) REFERENCES nodes (id) ON DELETE CASCADE
);

CREATE INDEX alias_node_id ON aliases (node_id );
CREATE INDEX refs_node_id ON refs (node_id );
CREATE INDEX tags_node_id ON tags (node_id );
