//! P28 — Schema validation sweep.
//!
//! Iterates every registered built-in node, calls `input_schema()` and
//! `output_schema()` on each, and asserts that the returned value is a
//! well-formed JSON Schema object.  This is a regression guard: it catches
//! nodes that accidentally return malformed JSON (e.g. null where an object
//! is expected, or a non-string element inside "required").
//!
//! Run with: `cargo test -p aerini-engine --test schema_validation`

use aerini_engine::node::NodeRegistry;
use aerini_engine::nodes::register_builtins;

fn make_registry() -> NodeRegistry {
    let mut registry = NodeRegistry::new();
    let data_dir = std::env::temp_dir();
    register_builtins(&mut registry, &data_dir, None);
    registry.seal_builtins();
    registry
}

/// Every node's `input_schema()` and `output_schema()` must either:
///  - be `null` or `{}` (declaring no constraints), or
///  - be a JSON object containing at least a `"type"` key.
///
/// Additionally, when present:
///  - `"required"` must be an array of strings.
///  - `"properties"` must be an object.
#[test]
fn all_node_schemas_are_valid_objects() {
    let registry = make_registry();
    let descriptors = registry.all_descriptors();

    assert!(
        !descriptors.is_empty(),
        "registry must contain at least one node"
    );

    for desc in &descriptors {
        for (schema_name, schema) in [
            ("input_schema",  &desc.input_schema),
            ("output_schema", &desc.output_schema),
        ] {
            // Null and empty object are valid "no constraint" declarations.
            if schema.is_null() {
                continue;
            }
            let obj = schema
                .as_object()
                .unwrap_or_else(|| panic!(
                    "node '{}' {schema_name} must be null or an object, got: {schema}",
                    desc.type_id
                ));

            if obj.is_empty() {
                // {} is a valid no-constraint schema.
                continue;
            }

            // Non-empty schemas must declare a "type".
            assert!(
                obj.contains_key("type"),
                "node '{}' {schema_name} is non-empty but missing \"type\" key.\nSchema: {schema}",
                desc.type_id
            );

            // "required", when present, must be an array of strings.
            if let Some(required) = obj.get("required") {
                let arr = required.as_array().unwrap_or_else(|| panic!(
                    "node '{}' {schema_name}.required must be an array, got: {required}",
                    desc.type_id
                ));
                for (i, item) in arr.iter().enumerate() {
                    assert!(
                        item.is_string(),
                        "node '{}' {schema_name}.required[{i}] must be a string, got: {item}",
                        desc.type_id
                    );
                }
            }

            // "properties", when present, must be an object.
            if let Some(props) = obj.get("properties") {
                assert!(
                    props.is_object(),
                    "node '{}' {schema_name}.properties must be an object, got: {props}",
                    desc.type_id
                );
            }
        }
    }
}

/// Every node's `input_schema()` must not list a field in `"required"` that
/// is absent from `"properties"`.  A required field with no property entry
/// means the frontend cannot render a config field for it — the user can
/// never satisfy the requirement.
#[test]
fn required_fields_exist_in_properties() {
    let registry = make_registry();

    for desc in registry.all_descriptors() {
        let schema = &desc.input_schema;

        let obj = match schema.as_object() {
            Some(o) if !o.is_empty() => o,
            _ => continue,
        };

        let required: Vec<&str> = match obj.get("required").and_then(|r| r.as_array()) {
            Some(arr) => arr.iter().filter_map(|v| v.as_str()).collect(),
            None => continue,
        };

        let properties = match obj.get("properties").and_then(|p| p.as_object()) {
            Some(p) => p,
            None => {
                // required exists but properties is absent — every required field
                // has no corresponding property entry.
                if !required.is_empty() {
                    panic!(
                        "node '{}' input_schema has required fields {:?} but no properties object",
                        desc.type_id, required
                    );
                }
                continue;
            }
        };

        for field in &required {
            assert!(
                properties.contains_key(*field),
                "node '{}' input_schema lists \"{}\" in required but it is absent from properties",
                desc.type_id,
                field
            );
        }
    }
}

/// The node registry must not contain duplicate type IDs.
/// A duplicate silently replaces an earlier registration, hiding the first node.
#[test]
fn no_duplicate_type_ids() {
    let registry = make_registry();
    let descriptors = registry.all_descriptors();

    let mut seen = std::collections::HashSet::new();
    for desc in &descriptors {
        assert!(
            seen.insert(desc.type_id.clone()),
            "duplicate type_id registered: '{}'",
            desc.type_id
        );
    }
}

/// Every registered node must have a non-empty display_name and version.
#[test]
fn all_nodes_have_display_name_and_version() {
    let registry = make_registry();
    for desc in registry.all_descriptors() {
        assert!(
            !desc.display_name.is_empty(),
            "node '{}' has empty display_name",
            desc.type_id
        );
        assert!(
            !desc.version.is_empty(),
            "node '{}' has empty version",
            desc.type_id
        );
    }
}
