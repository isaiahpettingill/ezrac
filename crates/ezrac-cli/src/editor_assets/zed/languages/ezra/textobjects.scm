(function_item
  (block) @function.inside) @function.around

(struct_item) @class.around
(layout_declaration) @class.around

(line_comment)+ @comment.around
