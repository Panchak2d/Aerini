/**
 * Static description fallback for nodes that do not yet carry a description
 * from the Rust engine (e.g. frontend-only canvas nodes, or plugin nodes
 * authored before the description() trait method existed).
 *
 * Keys are type_id values. The Rust-sourced description (NodeDescriptor.description)
 * takes priority; this map is only consulted when that field is absent or empty.
 */
export const NODE_DESCRIPTION_FALLBACK: Readonly<Record<string, string>> = {
  note:           "A text annotation on the canvas. No inputs or outputs — does not affect workflow execution.",
  transform_data: "Apply a series of transformation steps to reshape or filter an object or array.",
};
