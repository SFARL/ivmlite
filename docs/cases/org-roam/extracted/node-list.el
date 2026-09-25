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
