#[cfg(feature = "offline")]
mod imp {
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use aerini_engine::{
        model::Workflow, node::NodeRegistry, nodes::register_builtins,
        validation::validate_workflow_json,
    };
    use serde_json::{json, Value};

    const PLUGIN_NOTE: &str = "Checked against the built-in nodes only; a node from a plugin installed in the desktop app is reported as an unknown type.";

    pub struct Offline {
        registry: NodeRegistry,
        out_dir: PathBuf,
    }

    impl Offline {
        pub fn new(out_dir: Option<PathBuf>) -> Result<Self, String> {
            let mut registry = NodeRegistry::new();
            register_builtins(&mut registry, &std::env::temp_dir(), None);
            Ok(Self { registry, out_dir: out_dir.unwrap_or_else(default_out_dir) })
        }

        pub fn nodes(&self) -> Vec<Value> {
            let mut descriptors = self.registry.all_descriptors();
            descriptors.sort_by(|a, b| a.type_id.cmp(&b.type_id));
            descriptors
                .iter()
                .filter_map(|d| serde_json::to_value(d).ok())
                .map(|mut v| {
                    if let Some(fields) = v.as_object_mut() {
                        fields.remove("icon");
                    }
                    v
                })
                .collect()
        }

        pub fn validate(&self, workflow_json: &str) -> Value {
            let mut report = serde_json::to_value(validate_workflow_json(workflow_json, &self.registry))
                .unwrap_or(Value::Null);
            if let Some(fields) = report.as_object_mut() {
                fields.insert("note".to_string(), json!(PLUGIN_NOTE));
            }
            report
        }

        pub async fn save(&self, workflow_json: &str, allow_invalid: bool, overwrite: bool) -> Result<Value, String> {
            let report = self.validate(workflow_json);
            let valid = report["valid"].as_bool().unwrap_or(false);
            if !valid && !allow_invalid {
                return Err(format!(
                    "Not written: the workflow has validation errors. Fix them, or pass allow_invalid=true to write it anyway. Report: {report}"
                ));
            }
            let workflow = Workflow::from_json(workflow_json).map_err(|e| format!("Not written: {e}"))?;
            let pretty = workflow.to_json_pretty().map_err(|e| format!("Not written: {e}"))?;
            let dir = self.out_dir.clone();
            let path = dir.join(file_name(&workflow.id));
            let target = path.clone();
            tokio::task::spawn_blocking(move || write_file(&dir, &target, &pretty, overwrite))
                .await
                .map_err(|e| format!("Not written: {e}"))??;
            Ok(json!({
                "written": true,
                "path": path.display().to_string(),
                "workflow_id": workflow.id,
                "validation": report,
                "next": "Drag the file onto the Aerini desktop canvas (or use the import button on an empty canvas), review the preview, then import it. Nothing was run.",
            }))
        }
    }

    fn default_out_dir() -> PathBuf {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        home.map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("aerini-workflows")
    }

    pub(super) fn file_name(id: &str) -> String {
        let cleaned: String = id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
            .take(80)
            .collect();
        let stem = cleaned.trim_start_matches('.');
        let stem = if stem.is_empty() { "workflow" } else { stem };
        let base = stem.split('.').next().unwrap_or_default();
        let reserved = ["con", "prn", "aux", "nul"].iter().any(|r| base.eq_ignore_ascii_case(r))
            || (base.len() == 4
                && (base[..3].eq_ignore_ascii_case("com") || base[..3].eq_ignore_ascii_case("lpt"))
                && base.as_bytes()[3].is_ascii_digit());
        if reserved {
            format!("_{stem}.aerini")
        } else {
            format!("{stem}.aerini")
        }
    }

    fn write_file(dir: &Path, path: &Path, content: &str, overwrite: bool) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("Not written: cannot create {}: {e}", dir.display()))?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true);
        if overwrite {
            options.create(true).truncate(true);
        } else {
            options.create_new(true);
        }
        let mut file = options.open(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                format!("Not written: {} already exists; pass overwrite=true to replace it.", path.display())
            } else {
                format!("Not written: cannot open {}: {e}", path.display())
            }
        })?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("Not written: cannot write {}: {e}", path.display()))
    }

    #[cfg(test)]
    mod tests {
        use super::file_name;

        #[test]
        fn file_names_are_flat_safe_and_never_empty() {
            assert_eq!(file_name("my flow/../x"), "my_flow_.._x.aerini");
            assert_eq!(file_name("..."), "workflow.aerini");
            assert_eq!(file_name("").as_str(), "workflow.aerini");
            assert_eq!(file_name(&"a".repeat(200)).len(), 80 + ".aerini".len());
            assert_eq!(file_name("NUL"), "_NUL.aerini");
            assert_eq!(file_name("com1.v2"), "_com1.v2.aerini");
            assert_eq!(file_name("com10"), "com10.aerini");
        }
    }
}

#[cfg(not(feature = "offline"))]
mod imp {
    use std::path::PathBuf;

    use serde_json::Value;

    const MISSING: &str = "This build of aerini-mcp has no offline mode; set AERINI_URL or use the offline build.";

    pub struct Offline;

    impl Offline {
        pub fn new(_out_dir: Option<PathBuf>) -> Result<Self, String> {
            Err("AERINI_URL is not set and this build of aerini-mcp has no offline mode; set AERINI_URL, or use the offline build (cargo feature `offline`)".to_string())
        }

        pub fn nodes(&self) -> Vec<Value> {
            Vec::new()
        }

        pub fn validate(&self, _workflow_json: &str) -> Value {
            Value::Null
        }

        pub async fn save(&self, _workflow_json: &str, _allow_invalid: bool, _overwrite: bool) -> Result<Value, String> {
            Err(MISSING.to_string())
        }
    }
}

pub use imp::Offline;
