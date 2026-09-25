# Introduction

While using Org-roam - the data is stored across tables in the database.
The tables are as follows

``` example
(files . file title hash atime mtime)

(nodes . id file level pos todo priority scheduled deadline title properties olp)

(aliases . node_id alias)

(citations . node_id cite_key pos properties)

(refs . node_id ref type)

(tags . node_id tag)

(links . pos source dest type properties)
```

The subset relevant to query operations is

``` example
(files . file title atime mtime)

(nodes . id file level pos todo priority scheduled deadline title properties olp)

(aliases . node_id alias)

(refs . node_id ref type)

(tags . node_id tag)
```

# Problem:

**Problem description**: Table joins are computationally intensive
operation. Implies querying data across tables when node size is large
for the respective table is a potential bottleneck.

-   Solution: Materialized View -\*- a strategy to cache expensive query
    operations

    -   Problem: No native support for Sqlite

        -   Solution: Hack Sqlite framework to get the same benefits

            -   Strategy 1: As implemented in Vulpea : Latch onto
                Org-roam\'s write-to-db protocol -- implies trigger
                computation is done by Emacs.

            -   Strategy 2: Latch onto \`Create Trigger\' of sqlite --
                implies computation is delegated to SQLITE

# Exploring Strategy 2

Note: see **Appendix/Sql Transaction** below

## Step 0 : Define the new table schema

``` example
(nodes_view . nview_id file level pos todo priority scheduled deadline title properties olp alias type+ref tag)

(files . file title hash atime mtime)

(nodes . id file level pos todo priority scheduled deadline title properties olp)

(aliases . node_id alias)

(citations . node_id cite_key pos properties)

(refs . node_id ref type)

(tags . node_id tag)

(links . pos source dest type properties)
```

-   Create the table

    ``` sql
    CREATE TABLE nodes_view (nview_id PRIMARY KEY, file NOT NULL, level, pos, todo, priority, scheduled, deadline, title, properties, olp, tag, alias, type_ref, FOREIGN KEY (file) REFERENCES files (file) ON DELETE CASCADE);
    ```

-   To delete the table

    ``` sql
    DROP TABLE IF EXISTS nodes_view;
    ```

## Step 1: Find a way to properly store refs,tags & aliases

Currently each tag, alias and ref+type is stored individually. In
nodes_view each row must correspond to only one id. That is every ID
row must be unique

This is the query used by Org-roam to generate org-roam-node-list for
reference:

``` sql
 "SELECT
  id,
  file,
  filetitle,
  \"level\",
  todo,
  pos,
  priority ,
  scheduled ,
  deadline ,
  title,
  properties ,
  olp,
  atime,
  mtime,
  '(' || group_concat(tags, ' ') || ')' as tags,
  aliases,
  refs
FROM
  (
  SELECT
    id,
    file,
    filetitle,
    \"level\",
    todo,
    pos,
    priority ,
    scheduled ,
    deadline ,
    title,
    properties ,
    olp,
    atime,
    mtime,
    tags,
    '(' || group_concat(aliases, ' ') || ')' as aliases,
    refs
  FROM
    (
    SELECT
      nodes.id as id,
      nodes.file as file,
      nodes.\"level\" as \"level\",
      nodes.todo as todo,
      nodes.pos as pos,
      nodes.priority as priority,
      nodes.scheduled as scheduled,
      nodes.deadline as deadline,
      nodes.title as title,
      nodes.properties as properties,
      nodes.olp as olp,
      files.atime as atime,
      files.mtime as mtime,
      files.title as filetitle,
      tags.tag as tags,
      aliases.alias as aliases,
      '(' || group_concat(RTRIM (refs.\"type\", '\"') || ':' || LTRIM(refs.ref, '\"'), ' ') || ')' as refs
    FROM nodes
    LEFT JOIN files ON files.file = nodes.file
    LEFT JOIN tags ON tags.node_id = nodes.id
    LEFT JOIN aliases ON aliases.node_id = nodes.id
    LEFT JOIN refs ON refs.node_id = nodes.id
    GROUP BY nodes.id, tags.tag, aliases.alias )
  GROUP BY id, tags )
GROUP BY id;
```

We use a modified version thereof to populate the table first:

``` sql
INSERT INTO nodes_view (nview_id, file, level, pos, todo, priority, scheduled, deadline, title, properties, olp, tag, alias, type_ref)
 SELECT
  id,
  file,
  "level",
  pos,
  todo,
  priority ,
  scheduled ,
  deadline ,
  title,
  properties ,
  olp,
  '(' || group_concat(tags, ' ') || ')' as tags,
  aliases,
  refs
FROM
  (
  SELECT
    id,
    file,
    "level",
    todo,
    pos,
    priority ,
    scheduled ,
    deadline ,
    title,
    properties ,
    olp,
    tags,
    '(' || group_concat(aliases, ' ') || ')' as aliases,
    refs
  FROM
    (
    SELECT
      nodes.id as id,
      nodes.file as file,
      nodes."level" as "level",
      nodes.todo as todo,
      nodes.pos as pos,
      nodes.priority as priority,
      nodes.scheduled as scheduled,
      nodes.deadline as deadline,
      nodes.title as title,
      nodes.properties as properties,
      nodes.olp as olp,
      tags.tag as tags,
      aliases.alias as aliases,
      '(' || group_concat(RTRIM (refs."type", '"') || ':' || LTRIM(refs.ref, '"'), ' ') || ')' as refs
    FROM nodes
    LEFT JOIN tags ON tags.node_id = nodes.id
    LEFT JOIN aliases ON aliases.node_id = nodes.id
    LEFT JOIN refs ON refs.node_id = nodes.id
    GROUP BY nodes.id, tags.tag, aliases.alias )
  GROUP BY id, tags )
GROUP BY id;
```

## Step 2: Explore Triggers

**Question**: What is a Trigger again?

*Answer*: A trigger is a database object that is automatically executed
or fired when certain events occur. A trigger is activated by one of the
following events on a table:

-   INSERT: When a new row is inserted.

-   UPDATE: When an existing row is modified.

-   DELETE: When a row is deleted.

To create the materialized view using SQLite triggers and ensure that
all related data for a \`nodes.id\` (including tags, aliases, and refs)
is tracked properly by \`nodes_view\`, we establish the following
protocols:

### Triggers to Maintain \`nodes_view\`

Create triggers that automatically update \`nodes_view\` when
\`nodes\`, \`tags\`, \`aliases\` or \`refs\` are updated.

1.  Triggers for the nodes table

    1.  Trigger for Inserting into \`nodes\`

        ``` sql
        CREATE TRIGGER insert_node_trigger
        AFTER INSERT ON nodes
        BEGIN
            INSERT INTO nodes_view (nview_id, file, level, pos, todo, priority, scheduled, deadline, title, properties, olp)
            VALUES (NEW.id, NEW.file, NEW.level, NEW.pos, NEW.todo, NEW.priority, NEW.scheduled, NEW.deadline, NEW.title, NEW.properties, NEW.olp);
        END;
        ```

    2.  Trigger for Updating \`nodes\`

        ``` sql
        CREATE TRIGGER update_node_trigger
        AFTER UPDATE ON nodes
        BEGIN
            UPDATE nodes_view
            SET file = NEW.file, 
                level = NEW.level,
                pos = NEW.pos,
                todo = NEW.todo,
                priority = NEW.priority,
                scheduled = NEW.scheduled,
                deadline = NEW.deadline,
                title = NEW.title,
                properties = NEW.properties,
                olp = NEW.olp
            WHERE nview_id = OLD.id;
        END;
        ```

    3.  Trigger for Deleting from \`nodes\`

        ``` sql
        CREATE TRIGGER delete_node_trigger
        AFTER DELETE ON nodes
        BEGIN
            DELETE FROM nodes_view
            WHERE nview_id = OLD.id;
        END;
        ```

2.  Triggers for the tags table

    1.  Trigger for Inserting into \`tags\`

        ``` sql
        CREATE TRIGGER insert_tag_trigger
        AFTER INSERT ON tags
        BEGIN
            UPDATE nodes_view
            SET tag = (SELECT '(' || group_concat(tags.tag, ' ') || ')' FROM tags WHERE node_id = NEW.node_id)
            WHERE nview_id = NEW.node_id;
        END;
        ```

    2.  Trigger for Updating \`tags\`

        ``` sql
        CREATE TRIGGER update_tag_trigger
        AFTER UPDATE ON tags
        BEGIN
            UPDATE nodes_view
            SET tag = (SELECT '(' || group_concat(tags.tag, ' ') || ')' FROM tags WHERE node_id = OLD.node_id)
            WHERE nview_id = OLD.node_id;
        END;
        ```

    3.  Trigger for Deleting from \`tags\`

        ``` sql
        CREATE TRIGGER delete_tag_trigger
        AFTER DELETE ON tags
        BEGIN
            UPDATE nodes_view
            SET tag = (SELECT '(' || group_concat(tags.tag, ' ') || ')' FROM tags WHERE node_id = OLD.node_id)
            WHERE nview_id = OLD.node_id;
        END;
        ```

3.  Triggers for the aliases table

    1.  Trigger for Inserting into \`aliases\`

        ``` sql
        CREATE TRIGGER insert_alias_trigger
        AFTER INSERT ON aliases
        BEGIN
            UPDATE nodes_view
            SET alias = (SELECT '(' || group_concat(aliases.alias, ' ') || ')' FROM aliases WHERE node_id = NEW.node_id)
            WHERE nview_id = NEW.node_id;
        END;
        ```

    2.  Trigger for Updating \`aliases\`

        ``` sql
        CREATE TRIGGER update_alias_trigger
        AFTER UPDATE ON aliases
        BEGIN
            UPDATE nodes_view
            SET alias = (SELECT '(' || group_concat(aliases.alias, ' ') || ')' FROM aliases WHERE node_id = OLD.node_id)
            WHERE nview_id = OLD.node_id;
        END;
        ```

    3.  Trigger for Deleting from \`aliases\`

        ``` sql
        CREATE TRIGGER delete_alias_trigger
        AFTER DELETE ON aliases
        BEGIN
            UPDATE nodes_view
            SET alias = (SELECT '(' || group_concat(aliases.alias, ' ') || ')' FROM aliases WHERE node_id = OLD.node_id)
            WHERE nview_id = OLD.node_id;
        END;
        ```

4.  Triggers for the refs table

    1.  Trigger for Inserting into \`refs\`

        ``` sql
        CREATE TRIGGER insert_ref_trigger
        AFTER INSERT ON refs
        BEGIN
            UPDATE nodes_view
            SET type_ref = (SELECT '(' || group_concat(RTRIM (refs."type", '"') || ':' || LTRIM(refs.ref, '"'), ' ') || ')' FROM refs WHERE node_id = NEW.node_id)
            WHERE nview_id = NEW.node_id;
        END;
        ```

    2.  Trigger for Updating \`refs\`

        ``` sql
        CREATE TRIGGER update_ref_trigger
        AFTER UPDATE ON refs
        BEGIN
            UPDATE nodes_view
            SET type_ref = (SELECT '(' || group_concat(RTRIM (refs."type", '"') || ':' || LTRIM(refs.ref, '"'), ' ') || ')' FROM refs WHERE node_id = OLD.node_id)
            WHERE nview_id = OLD.node_id;
        END;
        ```

    3.  Trigger for Deleting from \`refs\`

        ``` sql
        CREATE TRIGGER delete_ref_trigger
        AFTER DELETE ON refs
        BEGIN
            UPDATE nodes_view
            SET type_ref = (SELECT '(' || group_concat(RTRIM (refs."type", '"') || ':' || LTRIM(refs.ref, '"'), ' ') || ')' FROM refs WHERE node_id = OLD.node_id)
            WHERE nview_id = OLD.node_id;
        END;
        ```

## Step 3: Clean up

Run \"Vacuum\" on the database to defragment it so that we don\'t leave
a mess behind

``` sql
VACUUM;
```

## Step 4: Utilise the nodes_view table to do queries

Put the following in your Emacs INIT Protocol.

``` elisp
(defun +org-roam-node-list ()
  "Modified `org-roam-node-list' that utilises MATERIALIZED VIEW to cache
 expensive query operations;

 Return all nodes stored in the database as a list of `org-roam-node's."
  (let ((rows (org-roam-db-query
           "SELECT

nodes_view.nview_id, nodes_view.file, files.title,

nodes_view.\"level\", nodes_view.todo, nodes_view.pos,

nodes_view.priority, nodes_view.scheduled, nodes_view.deadline,

nodes_view.title, nodes_view.properties, nodes_view.olp,

files.atime, files.mtime, nodes_view.tag,

nodes_view.alias, nodes_view.type_ref

FROM nodes_view
INNER JOIN files ON files.file = nodes_view.file")))
      (cl-loop for row in rows
           append (pcase-let* ((`(,id ,file ,file-title ,level ,todo ,pos ,priority ,scheduled ,deadline
                      ,title ,properties ,olp ,atime ,mtime ,tags ,aliases ,refs)
                    row)
                   (all-titles (cons title aliases)))
            (mapcar (lambda (temp-title)
                  (org-roam-node-create :id id
                            :file file
                            :file-title file-title
                            :file-atime atime
                            :file-mtime mtime
                            :level level
                            :point pos
                            :todo todo
                            :priority priority
                            :scheduled scheduled
                            :deadline deadline
                            :title temp-title
                            :aliases aliases
                            :properties properties
                            :olp olp
                            :tags tags
                            :refs refs))
                all-titles)))))

(advice-add 'org-roam-node-list :override #'+org-roam-node-list)    
```

# Appendix:

## Sql Transaction

It is recommended to save the following in a file named your choice -
then pipe it into \`sqlite3\'

**NOTE**: These transactions will have to be reintroduced when db file
is rebuilt. Such as when running \`org-roam-db-sync\' with \`FORCE as
NON-NIL\'. These changes are made to the db file directly.

[Corollary]: These changes may be discarded easily by
discarding the db file. (Remember to remove the advice to
\`org-roam-node-list\' given just above when choosing to do so!)

``` bash
$ sqlite3 "/path/to/your/db" < org-roam-db-materialized-view.sql
```

``` sql
BEGIN TRANSACTION;

-- Create the materialized views table

CREATE TABLE nodes_view (
 nview_id PRIMARY KEY, file NOT NULL, level, pos, todo,
 priority, scheduled, deadline, title, properties, 
 olp, tag, alias, type_ref, 
 FOREIGN KEY (file) REFERENCES files (file) ON DELETE CASCADE);

-- Populate the table

INSERT INTO nodes_view (
 nview_id, file, level, pos, todo,
 priority, scheduled, deadline, title, properties,
 olp, tag, alias, type_ref)

 SELECT 
  id, file, "level", pos, todo,
  priority, scheduled, deadline, title, properties,
  olp, '(' || group_concat(tags, ' ') || ')' as tags, aliases, refs

  FROM
  (SELECT 
   id, file, "level", pos, todo, 
   priority , scheduled , deadline , title, properties,
   olp, tags, '(' || group_concat(aliases, ' ') || ')' as aliases, refs

   FROM
   (SELECT
    nodes.id as id, nodes.file as file, nodes."level" as "level", nodes.pos as pos, nodes.todo as todo,
    nodes.priority as priority, nodes.scheduled as scheduled, nodes.deadline as deadline, nodes.title as title, nodes.properties as properties,
    nodes.olp as olp, tags.tag as tags, aliases.alias as aliases, '(' || group_concat(RTRIM (refs."type", '"') || ':' || LTRIM(refs.ref, '"'), ' ') || ')' as refs

    FROM nodes
    LEFT JOIN tags ON tags.node_id = nodes.id
    LEFT JOIN aliases ON aliases.node_id = nodes.id
    LEFT JOIN refs ON refs.node_id = nodes.id
    GROUP BY nodes.id, tags.tag, aliases.alias)

   GROUP BY id, tags)
 GROUP BY id;

-- Index

CREATE INDEX nodes_view_file ON nodes_view (file);

-- Triggers

-- nodes table triggers

CREATE TRIGGER insert_node_trigger
AFTER INSERT ON nodes
BEGIN
    INSERT INTO nodes_view (nview_id, file, level, pos, todo, priority, scheduled, deadline, title, properties, olp)
    VALUES (NEW.id, NEW.file, NEW.level, NEW.pos, NEW.todo, NEW.priority, NEW.scheduled, NEW.deadline, NEW.title, NEW.properties, NEW.olp);
END;

CREATE TRIGGER update_node_trigger
AFTER UPDATE ON nodes
BEGIN
    UPDATE nodes_view
    SET file = NEW.file, 
        level = NEW.level,
        pos = NEW.pos,
        todo = NEW.todo,
        priority = NEW.priority,
        scheduled = NEW.scheduled,
        deadline = NEW.deadline,
        title = NEW.title,
        properties = NEW.properties,
        olp = NEW.olp
    WHERE nview_id = OLD.id;
END;

CREATE TRIGGER delete_node_trigger
AFTER DELETE ON nodes
BEGIN
    DELETE FROM nodes_view
    WHERE nview_id = OLD.id;
END;

-- tags table triggers

CREATE TRIGGER insert_tag_trigger
AFTER INSERT ON tags
BEGIN
    UPDATE nodes_view
    SET tag = (SELECT '(' || group_concat(tags.tag, ' ') || ')' FROM tags WHERE node_id = NEW.node_id)
    WHERE nview_id = NEW.node_id;
END;

CREATE TRIGGER update_tag_trigger
AFTER UPDATE ON tags
BEGIN
    UPDATE nodes_view
    SET tag = (SELECT '(' || group_concat(tags.tag, ' ') || ')' FROM tags WHERE node_id = OLD.node_id)
    WHERE nview_id = OLD.node_id;
END;

CREATE TRIGGER delete_tag_trigger
AFTER DELETE ON tags
BEGIN
    UPDATE nodes_view
    SET tag = (SELECT '(' || group_concat(tags.tag, ' ') || ')' FROM tags WHERE node_id = OLD.node_id)
    WHERE nview_id = OLD.node_id;
END;

-- aliases table triggers

CREATE TRIGGER insert_alias_trigger
AFTER INSERT ON aliases
BEGIN
    UPDATE nodes_view
    SET alias = (SELECT '(' || group_concat(aliases.alias, ' ') || ')' FROM aliases WHERE node_id = NEW.node_id)
    WHERE nview_id = NEW.node_id;
END;

CREATE TRIGGER update_alias_trigger
AFTER UPDATE ON aliases
BEGIN
    UPDATE nodes_view
    SET alias = (SELECT '(' || group_concat(aliases.alias, ' ') || ')' FROM aliases WHERE node_id = OLD.node_id)
    WHERE nview_id = OLD.node_id;
END;

CREATE TRIGGER delete_alias_trigger
AFTER DELETE ON aliases
BEGIN
    UPDATE nodes_view
    SET alias = (SELECT '(' || group_concat(aliases.alias, ' ') || ')' FROM aliases WHERE node_id = OLD.node_id)
    WHERE nview_id = OLD.node_id;
END;

-- refs table triggers

CREATE TRIGGER insert_ref_trigger
AFTER INSERT ON refs
BEGIN
    UPDATE nodes_view
    SET type_ref = (SELECT '(' || group_concat(RTRIM (refs."type", '"') || ':' || LTRIM(refs.ref, '"'), ' ') || ')' FROM refs WHERE node_id = NEW.node_id)
    WHERE nview_id = NEW.node_id;
END;

CREATE TRIGGER update_ref_trigger
AFTER UPDATE ON refs
BEGIN
    UPDATE nodes_view
    SET type_ref = (SELECT '(' || group_concat(RTRIM (refs."type", '"') || ':' || LTRIM(refs.ref, '"'), ' ') || ')' FROM refs WHERE node_id = OLD.node_id)
    WHERE nview_id = OLD.node_id;
END;

CREATE TRIGGER delete_ref_trigger
AFTER DELETE ON refs
BEGIN
    UPDATE nodes_view
    SET type_ref = (SELECT '(' || group_concat(RTRIM (refs."type", '"') || ':' || LTRIM(refs.ref, '"'), ' ') || ')' FROM refs WHERE node_id = OLD.node_id)
    WHERE nview_id = OLD.node_id;
END;

COMMIT;

VACUUM; -- defragment the database.
```

## The Original Table in SQL format

``` sql
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
```
