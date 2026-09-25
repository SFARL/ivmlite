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
