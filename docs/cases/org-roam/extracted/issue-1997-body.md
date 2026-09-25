## Brief Abstract

Materialized view is a table where each row contains _all_ information about node, including information from the following tables: nodes, aliases, citations, refs, tags and links.

## Benefits

This would improve **performance** of many query operations, where we rely on [multiplication of multiple tables][1]. See  [vulpea#116][2] for benchmarks of possible implementation. Those benchmarks are using some `vulpea` functions, but in short it compares approach of multiplication and view table on db of 9554 notes:

| test          | result size |            regular |         view table |     ratio |
| ------------- | ----------- | ------------------ | ------------------ | --------- |
| `filter-on-tags-1`   | 30 notes    | 4.6693460650999995 |       1.0112478712 | 4.6174100 |
| `filter-on-tags-2`  | 3168 notes  | 4.7333844436999996 |       1.0059819176 | 4.7052381 |
| `filter-on-links-1`  | 1657 notes  |       4.8095771283 | 1.0462236128999999 | 4.5970833 |
| `filter-on-links-2` | 92 notes    | 4.5517473337999995 |       1.0204833089 | 4.4603839 |

As you can see, when all notes needs to be traversed, view table provides **x4.595** performance improvement.

### Who would benefit?

The following group of users would benefit from this feature:

- Regular users of Org Roam, since interactive functions of finding and inserting notes would become much faster.
- Users of various advanced packages based on Org Roam (like delve or maybe even org-roam-ui) as these applications do lots of querying behind the scenes and they needs most of that information.
- Developers of applications based on Org Roam, since with view table you can quickly get all you need without thinking too much on how to get all required information at once. Writing query like [this][1] is hard.

## Long Description

Right now when we need information from multiple tables, we use [table multiplication][1]. But the more tables we want to multiply the slower this query becomes.

So instead of doing this 'multiplication' on the read side, we could maintain a separate table that contains all this information in one place. The schema would look like:

```emacs-lisp
([(id :not-null :primary-key)
  (path :not-null)
  (level :not-null)
  (title :not-null)
  (properties :not-null)
  aliases
  tags
  meta
  links
  refs
  citations]
 (:foreign-key [path] :references files [file] :on-delete :cascade))
```

## Proposed Implementation (if any)

See [vulpea#116][2] as example of implementation.

Implementation would consist of 2 parts (can and should be released separately):

1. Implementing view table lifecycle (e.g. writing).
2. Using it across the `org-roam` code base where query happens (e.g. reading).

### Writing

Whenever the note is being synced, we also add all relevant information into this view table. I suspect that the sync routine needs to be modified a little bit, so we can avoid double parsing or non atomic inserts.

### Reading

Instead of doing horrific SQL multiplication, we will use `org-roam-query` with this new materialized view table.

## FAQ

### What about write performance?

You might noticed that in [vulpea#116][2] db sync performance degrades quite a bit. This is explained by the fact that I simply duplicated buffer parsing, so it takes almost twice the time. If view table is implemented in `org-roam`, the footprint of view table should be minimal, e.g. hardly noticeable.

### Any volunteers?

Me 😸 If you think that it worth including such table in `org-roam` I would gladly work on that. Especially since I already have a working implementation that I use on a daily basis.

## Please check the following:

- [X] No similar feature requests

-----

@jethrokuan Please let me know what you think. If something is not clear, just let me know 😄 I would gladly work on this feature for `org-roam`. In case you think that read performance doesn't cost data duplication, I have another proposal - to ease adding extra tables in `org-roam` db (and btw I believe it would be nice to have on its own regardless of this proposal - I will send it later) - with this materialized view can come as an extension.

cc @publicimageltd as you were interested in this happening

[1]: https://github.com/org-roam/org-roam/commit/a199886ef7ae208b0b10dc45e0df9b54d210cd4d
[2]: https://github.com/d12frosted/vulpea/pull/116