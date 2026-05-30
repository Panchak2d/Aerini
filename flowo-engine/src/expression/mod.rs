//! Expression resolver for `{{...}}` template syntax.
//!
//! Supported syntax:
//!   {{node_name.output.field}}          — field from a predecessor node's output
//!   {{node_name.output.field.nested}}   — dot-path traversal
//!   {{node_name.output.array_field[0]}} — array index
//!   {{$run.id}}                         — current execution ID
//!   {{$run.timestamp}}                  — ISO timestamp (current time at resolution)
//!   {{$run.workflow_name}}              — workflow name
//!   {{$env.VAR_NAME}}                   — environment variable (empty if unset)
//!   {{upper(node_name.output.field)}}   — inline function call
//!   {{format_date($run.timestamp, "YYYY-MM-DD")}} — function with literal args
//!
//! Fallback: any expression that cannot be resolved produces an empty string
//! and an INFO-level message in the returned warnings list. No panics, no errors.

mod resolver;
mod functions;
mod parser;

pub use resolver::{resolve_string, resolve_all_strings};
