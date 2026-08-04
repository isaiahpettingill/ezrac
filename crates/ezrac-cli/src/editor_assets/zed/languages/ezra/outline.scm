(function_item
  name: (identifier) @name) @item

(struct_item
  name: (identifier) @name) @item

(layout_declaration
  (identifier) @name) @item

[
  (const_declaration (identifier) @name)
  (alias_declaration (identifier) @name)
  (global_declaration (identifier) @name)
  (port_declaration (identifier) @name)
  (mmio_declaration (identifier) @name)
  (embed_declaration (identifier) @name)
] @item
