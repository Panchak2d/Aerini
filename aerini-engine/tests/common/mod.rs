//! Shared by the generated-reference tests: compare generated text with the
//! committed copy under `docs/reference/`, or rewrite it when asked to.

use std::path::PathBuf;

const REGEN_VAR: &str = "AERINI_REGEN_DOCS";

fn reference_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("docs")
        .join("reference")
        .join(name)
}

/// Fails when `docs/reference/<name>` differs from `generated`. With
/// `AERINI_REGEN_DOCS` set it writes the file instead and passes.
pub fn assert_committed(name: &str, generated: &str) {
    let path = reference_path(name);

    if std::env::var_os(REGEN_VAR).is_some() {
        std::fs::write(&path, generated)
            .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
        return;
    }

    let regen = format!(
        "{REGEN_VAR}=1 cargo test -p aerini-engine --features schema --test reference_docs --test workflow_schema"
    );
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}\nGenerate it with: {regen}", path.display()));
    // A Windows checkout may have converted line endings; the content is what must match.
    let committed = committed.replace("\r\n", "\n");

    if committed != generated {
        let line = committed
            .lines()
            .zip(generated.lines())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| committed.lines().count().min(generated.lines().count()))
            + 1;
        panic!(
            "docs/reference/{name} is out of date (first difference at line {line}).\n\
             Regenerate it with: {regen}\n\
             If you just changed or extracted source files and the regeneration still writes the old text, \
             cargo did not rebuild the engine. Run `touch aerini-engine/src/lib.rs` (or `cargo clean -p aerini-engine`) \
             and run the command again."
        );
    }
}
