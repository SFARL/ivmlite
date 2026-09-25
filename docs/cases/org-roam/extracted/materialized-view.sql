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
