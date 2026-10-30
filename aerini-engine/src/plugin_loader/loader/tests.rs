use std::io::Write;
use std::path::{Path, PathBuf};

use crate::plugin_loader::signature;
use crate::plugin_loader::error::PluginLoadError;
use crate::plugin_loader::loader::load::is_wasm_file_name;
use crate::plugin_loader::trigger::execute::trigger_linker;
use super::PluginLoader;

fn loader() -> PluginLoader {
    PluginLoader::new().expect("Wasmtime engine init failed in test")
}

/// Writes a sidecar next to `wasm_path` declaring a content hash that
/// can't match the file, and returns the sidecar's path.
fn write_tampered_sidecar(wasm_path: &Path) -> PathBuf {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

    let sig_path = signature::sig_sidecar_path(wasm_path);
    let sig_json = serde_json::json!({
        "schema_version": 1,
        "algorithm": "ed25519",
        "public_key": B64.encode([0u8; 32]),
        "files": [{"name": "plugin.wasm", "blake3": "0".repeat(64)}],
        "signature": B64.encode([0u8; 64]),
    })
    .to_string();
    std::fs::write(&sig_path, sig_json).expect("sig sidecar write failed");
    sig_path
}

/// The trigger linker defines the full import surface without a duplicate
/// definition between the WASI 0.2 (async) and 0.3 sets.
#[test]
fn trigger_linker_builds_on_trigger_engine() {
    let l = loader();
    assert!(trigger_linker(&l.trigger_engine).is_ok());
}

/// Normal and edge case: `.wasm` is recognised in any letter case and
/// sidecars are not plugin files.
#[test]
fn wasm_extension_match_ignores_case() {
    assert!(is_wasm_file_name(Path::new("a.wasm")));
    assert!(is_wasm_file_name(Path::new("A.WASM")));
    assert!(!is_wasm_file_name(Path::new("a.wasm.sig")));
    assert!(!is_wasm_file_name(Path::new("wasm")));
}

/// Non-existent path must return `PluginLoadError::Io`.
#[test]
fn load_plugin_nonexistent_file_returns_io_error() {
    let l = loader();
    let result = l.load_plugin(Path::new("/nonexistent/path/to/plugin.wasm"));
    assert!(result.is_err(), "expected Io error, got Ok");
    let err = result.err().unwrap();
    assert!(
        matches!(err, PluginLoadError::Io(_)),
        "expected Io, got: {err:?}"
    );
}

/// A valid WASM component that does NOT export the `aerini-node` world must
/// return `PluginLoadError::MissingInterface`.
///
/// Uses `(component)` — the smallest valid Component Model component. It has no
/// exports and no imports, so `linker.instantiate_pre` succeeds (no unsatisfied
/// imports) but `AeriniNodePre::new` fails (no `aerini:plugin/node` export).
#[test]
fn load_plugin_empty_component_returns_missing_interface() {
    let l = loader();

    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(b"(component)").expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let result = l.load_plugin(tmp.path());
    assert!(result.is_err(), "expected MissingInterface error, got Ok");
    let err = result.err().unwrap();
    assert!(
        matches!(err, PluginLoadError::MissingInterface),
        "expected MissingInterface, got: {err:?}"
    );
}

/// Normal case: `load_plugin` must reject a `.wasm` file whose sidecar
/// signature declares a content hash that doesn't match the file —
/// before any compilation is attempted, not just logged and let through.
#[test]
fn load_plugin_rejects_tampered_signature() {
    let l = loader();
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(b"(component)").expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let sig_path = write_tampered_sidecar(tmp.path());

    let result = l.load_plugin(tmp.path());
    let err = result.err().expect("expected the tampered signature to reject the load");
    assert!(
        matches!(err, PluginLoadError::SignatureRejected(_)),
        "expected SignatureRejected, got: {err:?}"
    );

    let _ = std::fs::remove_file(&sig_path);
}

/// Edge case: repeated `verify_signature` calls against the same,
/// unchanged file must hash it exactly once, not on every call — same
/// fingerprint-cache contract `component_cache` already provides,
/// checked here via `signature_check_count` instead of timing.
#[test]
fn verify_signature_reuses_cached_result_for_unchanged_file() {
    let l = loader();
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(b"(component)").expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    for _ in 0..3 {
        let _ = l.verify_signature(tmp.path());
    }

    assert_eq!(
        l.signature_check_count(),
        1,
        "expected exactly one real signature check across three verify_signature calls on an unchanged file"
    );
}

/// `describe_plugin` must reject a tampered file with `SignatureRejected`,
/// like `load_plugin`, instead of compiling and describing it.
#[test]
fn describe_plugin_rejects_tampered_signature() {
    let l = loader();
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(b"(component)").expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let sig_path = write_tampered_sidecar(tmp.path());

    let err = l.describe_plugin(tmp.path()).err().expect("expected the tampered signature to reject the describe");
    assert!(
        matches!(err, PluginLoadError::SignatureRejected(_)),
        "expected SignatureRejected, got: {err:?}"
    );

    let _ = std::fs::remove_file(&sig_path);
}

/// `resolve_trigger_instance` must skip a tampered file before compiling
/// it. `(component)` compiles fine on its own, so an unsigned copy bumps
/// the trigger compile count (control) while the tampered one must not.
#[tokio::test]
async fn resolve_trigger_instance_skips_tampered_file_without_compiling() {
    let l = loader();

    let tampered_dir = tempfile::tempdir().expect("tempdir create failed");
    let tampered = tampered_dir.path().join("plugin.wasm");
    std::fs::write(&tampered, b"(component)").expect("write failed");
    write_tampered_sidecar(&tampered);

    let err = l.resolve_trigger_instance(tampered_dir.path(), "any", None).await.err().expect("expected a miss");
    assert!(matches!(err, PluginLoadError::NoSuchTriggerPlugin(_)), "expected NoSuchTriggerPlugin, got: {err:?}");
    assert_eq!(l.trigger_compile_count(), 0, "tampered file must not be compiled");

    let unsigned_dir = tempfile::tempdir().expect("tempdir create failed");
    std::fs::write(unsigned_dir.path().join("plugin.wasm"), b"(component)").expect("write failed");
    let _ = l.resolve_trigger_instance(unsigned_dir.path(), "any", None).await;
    assert_eq!(l.trigger_compile_count(), 1, "unsigned file should be compiled");
}

/// `describe_plugin` must classify errors identically to `load_plugin`
/// for the same non-existent-path input — the non-leaking path must not
/// silently swallow or misclassify an error `load_plugin` already
/// handles correctly.
#[test]
fn describe_plugin_nonexistent_file_returns_io_error() {
    let l = loader();
    let result = l.describe_plugin(Path::new("/nonexistent/path/to/plugin.wasm"));
    assert!(result.is_err(), "expected Io error, got Ok");
    let err = result.err().unwrap();
    assert!(
        matches!(err, PluginLoadError::Io(_)),
        "expected Io, got: {err:?}"
    );
}

/// `describe_plugin` must classify a world-less component identically to
/// `load_plugin` (same mechanism as `load_plugin_empty_component_returns_missing_interface`
/// above — see that test's doc comment for why `(component)` triggers this path).
#[test]
fn describe_plugin_empty_component_returns_missing_interface() {
    let l = loader();

    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(b"(component)").expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let result = l.describe_plugin(tmp.path());
    assert!(result.is_err(), "expected MissingInterface error, got Ok");
    let err = result.err().unwrap();
    assert!(
        matches!(err, PluginLoadError::MissingInterface),
        "expected MissingInterface, got: {err:?}"
    );
}

/// `PluginLoader::shared()` must return the same
/// process-wide instance on every call, not construct a fresh `Engine` +
/// epoch-ticker thread each time (each such construction is a leak if
/// nothing ever tears it down — the exact failure mode a call site using
/// its own `PluginLoader::new()` instead of `shared()` would hit).
/// Pointer equality on the returned `&'static PluginLoader` is a direct
/// proxy for "no second construction happened": `OnceLock::get_or_init`
/// only ever runs its initializer once, so two `Ok` results can only
/// share an address if they came from the same underlying
/// `PluginLoader`.
#[test]
fn shared_returns_same_instance_across_repeated_calls() {
    let a = PluginLoader::shared().expect("shared() should succeed on a normal host");
    let b = PluginLoader::shared().expect("shared() should succeed on a normal host");
    let c = PluginLoader::shared().expect("shared() should succeed on a normal host");
    assert!(
        std::ptr::eq(a, b) && std::ptr::eq(b, c),
        "shared() must return the same PluginLoader on every call, not construct a new one"
    );
}

/// `describe_plugin` must agree with `load_plugin` on the metadata it
/// extracts for the same input — this only has an error path to compare
/// in this test module (no valid `aerini-node`-exporting fixture is
/// available without a real compiled plugin), so this asserts the two
/// paths at least agree on failure classification for a second distinct
/// error shape (`WasmCompile`, via a byte string that is neither valid
/// WAT text nor a valid WASM binary).
#[test]
fn describe_plugin_and_load_plugin_agree_on_invalid_bytes() {
    let l = loader();

    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(b"not a wasm module or wat text").expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let load_err = l.load_plugin(tmp.path()).err().expect("load_plugin should fail");
    let describe_err = l.describe_plugin(tmp.path()).err().expect("describe_plugin should fail");

    assert!(matches!(load_err, PluginLoadError::WasmCompile(_)));
    assert!(matches!(describe_err, PluginLoadError::WasmCompile(_)));
}

/// repeated `describe_plugin` calls against the same, unchanged file must
/// compile the `.wasm` bytes exactly once, not on every call. Uses a
/// per-instance compile counter (see `compile_count`) rather than
/// timing, so this can't be flaky.
#[test]
fn describe_plugin_reuses_cached_component_for_unchanged_file() {
    let l = loader();

    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(b"(component)").expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    for _ in 0..3 {
        // Every call returns MissingInterface (no export) — only the
        // compile *count*, not the result, is under test here.
        let _ = l.describe_plugin(tmp.path());
    }

    assert_eq!(
        l.compile_count(),
        1,
        "expected exactly one real compile across three describe_plugin calls on an unchanged file"
    );
}

/// Companion to the above: if the file at the same path actually
/// changes (different length — covers a plugin removed and a
/// different one reinstalled under the same filename), the cache must
/// not serve stale bytes — a fresh compile is required.
#[test]
fn describe_plugin_recompiles_when_file_content_changes() {
    let l = loader();

    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(b"(component)").expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let _ = l.describe_plugin(tmp.path());
    assert_eq!(l.compile_count(), 1);

    // Truncate and rewrite with different (longer) content so the
    // (mtime, len) fingerprint changes regardless of filesystem mtime
    // resolution.
    use std::io::{Seek, SeekFrom};
    tmp.as_file_mut().set_len(0).expect("truncate failed");
    tmp.as_file_mut().seek(SeekFrom::Start(0)).expect("seek failed");
    tmp.write_all(b"(component (core module (memory 1) (data (i32.const 100) \"abcd\")))")
        .expect("tempfile rewrite failed");
    tmp.flush().expect("tempfile flush failed");

    let _ = l.describe_plugin(tmp.path());
    assert_eq!(
        l.compile_count(),
        2,
        "file content/length changed at the same path — must recompile, not reuse the stale cached Component"
    );
}

/// Async entry point must classify a missing file identically to the
/// sync action-plugin path — `Io`, not `WasmCompile` — even though the
/// read now happens inside a spawned blocking task.
#[tokio::test]
async fn compile_trigger_component_nonexistent_file_returns_io_error() {
    let l = loader();
    let result = l.compile_trigger_component(Path::new("/nonexistent/path/to/plugin.wasm")).await;
    assert!(result.is_err(), "expected Io error, got Ok");
    assert!(
        matches!(result.err().unwrap(), PluginLoadError::Io(_)),
        "expected Io error classification to survive the spawn_blocking offload"
    );
}

/// The async (`spawn_blocking`-backed) and sync compile entry points
/// share one cache: compiling via one and then the other for the same
/// unchanged file must not trigger a second real compile.
#[tokio::test]
async fn compile_trigger_component_reuses_cache_across_sync_and_async_paths() {
    let l = loader();
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(b"(component)").expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let _ = l.compile_trigger_component_sync(tmp.path());
    let _ = l.compile_trigger_component(tmp.path()).await;

    assert_eq!(
        l.trigger_compile_count(),
        1,
        "expected exactly one real compile across the sync and async entry points for an unchanged file"
    );
}

/// `probe_trigger_capable`'s throwaway linker also links `storage` and
/// `wasi:http` p3 alongside `wasi:p3` — this must not change its answer
/// for a component that imports none of them: still `false`, since it
/// doesn't export `trigger` either.
#[test]
fn probe_trigger_capable_returns_false_for_component_without_trigger_export() {
    let l = loader();
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(b"(component)").expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    assert!(!l.probe_trigger_capable(tmp.path()));
}

/// A non-trigger component with an unsatisfied import must fail at link
/// time, not load and trap later. The import is a function: wasmtime treats
/// an unresolved import of an instance with no exports as satisfied.
#[test]
fn load_plugin_unsatisfied_import_without_trigger_export_fails_link() {
    let l = loader();
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(b"(component (import \"missing-fn\" (func)))")
        .expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let err = l.load_plugin(tmp.path()).err().expect("expected WasmLink error, got Ok");
    assert!(matches!(err, PluginLoadError::WasmLink(_)), "expected WasmLink, got: {err:?}");
}
