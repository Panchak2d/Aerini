//! Coverage here targets the pure validation/extraction/signature logic
//! that doesn't require an actual `aerini-node`-exporting component —
//! same limitation as `plugin_loader`'s own tests (no
//! fixture component exporting `aerini-node` exists to load). `install_plugin_pack_from_path`'s
//! happy path — reaching `PluginLoader::describe_plugin` on a real
//! staged member and its downstream member-conflict checks — is
//! therefore `UNVERIFIED` by these tests; every early-exit failure path
//! that returns before that point (extension/existence checks, manifest
//! parsing, signature verification, the already-installed/downgrade
//! gates) is covered directly.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aerini_engine::db::WorkflowDb;
use aerini_engine::plugin_loader::signature::{SIG_ALGORITHM, SIG_SCHEMA_VERSION, SigCheckResult, signed_message, SignedFileEntry};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use super::{install_plugin_pack_from_path, PLUGIN_DIR_SETTING, remove_plugin_pack};
use super::install::install_pack_blocking;
use super::pack::{check_pack_signature, extract_pack_zip, is_valid_pack_id, PACK_MANIFEST_MAX_BYTES, PACK_MAX_ENTRIES, PACK_SCHEMA_VERSION, parse_pack_manifest, read_entry_bounded, scan_installed_packs};
use super::remove::remove_pack_files;
use super::staging::{read_bounded, write_atomic};
use super::trust::{read_trust_store, save_trust_store, trust_store_path};

use super::*;

fn write_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(buf);
    let options = zip::write::SimpleFileOptions::default();
    for (name, data) in entries {
        zip.start_file(*name, options).expect("start_file failed");
        zip.write_all(data).expect("write_all failed");
    }
    zip.finish().expect("zip finish failed").into_inner()
}

fn write_pack_file(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
    let zip_bytes = write_test_zip(entries);
    let mut tmp = tempfile::Builder::new()
        .suffix(".aerinipkg")
        .tempfile()
        .expect("tempfile create failed");
    tmp.write_all(&zip_bytes).expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");
    tmp
}

fn sample_manifest_json(pack_id: &str, files: &[&str]) -> Vec<u8> {
    serde_json::json!({
        "schema_version": PACK_SCHEMA_VERSION,
        "pack_id": pack_id,
        "display_name": "Sample Pack",
        "files": files,
    })
    .to_string()
    .into_bytes()
}

#[test]
fn parse_pack_manifest_rejects_device_and_stream_member_names() {
    for bad in ["CON.wasm", "nul.wasm", "com1.wasm", "a:b.wasm", "a<b.wasm", "a .wasm", ".wasm"] {
        let manifest = sample_manifest_json("pack", &[bad]);
        assert!(parse_pack_manifest(&manifest).is_err(), "{bad} should be rejected");
    }
    let manifest = sample_manifest_json("pack", &["my_plugin-1.wasm"]);
    assert!(parse_pack_manifest(&manifest).is_ok());
}

#[test]
fn atomic_write_replaces_the_file_and_leaves_no_staging_file() {
    let dir = tempfile::tempdir().expect("tempdir create failed");
    let dest = dir.path().join("p.wasm");
    std::fs::write(&dest, b"old").expect("write failed");
    write_atomic(&dest, b"new content").expect("atomic write failed");
    assert_eq!(std::fs::read(&dest).expect("read failed"), b"new content");
    let names: Vec<_> = std::fs::read_dir(dir.path()).expect("read_dir failed").flatten().collect();
    assert_eq!(names.len(), 1);
}

#[test]
fn bounded_read_refuses_a_file_over_the_limit() {
    let dir = tempfile::tempdir().expect("tempdir create failed");
    let path = dir.path().join("f.bin");
    std::fs::write(&path, [0u8; 11]).expect("write failed");
    assert!(read_bounded(&path, 10).is_err());
    assert_eq!(read_bounded(&path, 11).expect("at the limit is allowed").len(), 11);
}

#[test]
fn corrupt_trust_store_is_an_error_and_a_missing_one_is_empty() {
    let dir = tempfile::tempdir().expect("tempdir create failed");
    assert!(read_trust_store(dir.path()).expect("missing store should read as empty").is_empty());

    std::fs::write(trust_store_path(dir.path()), b"{ not json").expect("write failed");
    assert!(read_trust_store(dir.path()).is_err());
}

#[test]
fn saved_trust_store_round_trips_without_a_leftover_temp_file() {
    let dir = tempfile::tempdir().expect("tempdir create failed");
    let mut store = HashMap::new();
    store.insert("pack:sample".to_string(), "key".to_string());
    save_trust_store(dir.path(), &store).expect("save failed");

    assert_eq!(read_trust_store(dir.path()).expect("read failed"), store);
    let leftovers = std::fs::read_dir(dir.path()).expect("read_dir failed").flatten().count();
    assert_eq!(leftovers, 1);
}

fn signing_key_fixture() -> ed25519_dalek::SigningKey {
    // Fixed, deterministic bytes — this is a test fixture, never a real
    // publisher key, so reproducibility matters more than secrecy here.
    ed25519_dalek::SigningKey::from_bytes(&[7u8; 32])
}

fn sig_file_json(signing_key: &ed25519_dalek::SigningKey, entries: &[SignedFileEntry]) -> Vec<u8> {
    use ed25519_dalek::Signer;
    let signature = signing_key.sign(&signed_message(entries));
    let files: Vec<_> = entries.iter().map(|e| serde_json::json!({"name": e.name, "blake3": e.blake3})).collect();
    serde_json::json!({
        "schema_version": SIG_SCHEMA_VERSION,
        "algorithm": SIG_ALGORITHM,
        "public_key": BASE64.encode(signing_key.verifying_key().to_bytes()),
        "files": files,
        "signature": BASE64.encode(signature.to_bytes()),
    })
    .to_string()
    .into_bytes()
}

// --- is_valid_pack_id ---

#[test]
fn is_valid_pack_id_accepts_reverse_domain_style_ids() {
    for id in ["com.example.mypack", "a", "A1", "pack-name_v2.1"] {
        assert!(is_valid_pack_id(id), "expected '{id}' to be accepted as a valid pack_id");
    }
}

#[test]
fn is_valid_pack_id_rejects_bad_starts_and_disallowed_chars() {
    for id in ["", ".starts-with-dot", "-starts-with-dash", "has space", "has/slash", "has\\backslash", "has:colon"] {
        assert!(!is_valid_pack_id(id), "expected '{id}' to be rejected as an invalid pack_id");
    }
}

// --- parse_pack_manifest ---

#[test]
fn parse_pack_manifest_accepts_well_formed_manifest() {
    let raw = sample_manifest_json("com.example.pack", &["a.wasm", "b.wasm"]);
    let m = parse_pack_manifest(&raw).expect("well-formed manifest should parse");
    assert_eq!(m.pack_id, "com.example.pack");
    assert_eq!(m.files, vec!["a.wasm".to_string(), "b.wasm".to_string()]);
}

#[test]
fn parse_pack_manifest_rejects_unsupported_schema_version() {
    let raw = serde_json::json!({
        "schema_version": 2, "pack_id": "com.example.pack", "display_name": "P", "files": ["a.wasm"]
    }).to_string().into_bytes();
    let err = parse_pack_manifest(&raw).expect_err("schema_version 2 should be rejected");
    assert!(err.starts_with("pack_invalid:"), "expected pack_invalid prefix, got: {err}");
}

#[test]
fn parse_pack_manifest_rejects_invalid_pack_id() {
    let raw = sample_manifest_json(".bad-id", &["a.wasm"]);
    let err = parse_pack_manifest(&raw).expect_err("invalid pack_id should be rejected");
    assert!(err.starts_with("pack_invalid:"));
}

#[test]
fn parse_pack_manifest_rejects_empty_files_list() {
    let raw = sample_manifest_json("com.example.pack", &[]);
    let err = parse_pack_manifest(&raw).expect_err("empty files list should be rejected");
    assert!(err.starts_with("pack_invalid:"));
}

#[test]
fn parse_pack_manifest_rejects_bad_member_filenames() {
    // Two distinct guard clauses in one table: a duplicate name, and a
    // name that isn't a bare filename.
    for files in [vec!["a.wasm", "a.wasm"], vec!["../evil.wasm"]] {
        let raw = sample_manifest_json("com.example.pack", &files);
        let err = parse_pack_manifest(&raw).expect_err(&format!("{files:?} should be rejected"));
        assert!(err.starts_with("pack_invalid:"));
    }
}

#[test]
fn parse_pack_manifest_rejects_oversized_manifest() {
    let raw = vec![b' '; (PACK_MANIFEST_MAX_BYTES + 1) as usize];
    let err = parse_pack_manifest(&raw).expect_err("oversized manifest should be rejected");
    assert!(err.starts_with("pack_invalid:"));
}

// --- read_entry_bounded ---

#[test]
fn read_entry_bounded_reads_all_bytes_under_cap() {
    let data = b"hello world";
    let result = read_entry_bounded(&data[..], 100).expect("should read fully under cap");
    assert_eq!(result, data);
}

#[test]
fn read_entry_bounded_errors_when_exceeding_cap() {
    let data = [0u8; 20];
    let err = read_entry_bounded(&data[..], 10).expect_err("should reject data over cap");
    assert!(err.starts_with("pack_invalid:"));
}

// --- extract_pack_zip ---

#[test]
fn extract_pack_zip_reads_manifest_and_members() {
    let zip_bytes = write_test_zip(&[("pack.json", b"manifest-bytes"), ("a.wasm", b"wasm-a"), ("b.wasm", b"wasm-b")]);
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(&zip_bytes).expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let (manifest, sig, members) = extract_pack_zip(tmp.path()).expect("valid zip should extract");
    assert_eq!(manifest, b"manifest-bytes");
    assert!(sig.is_none());
    assert_eq!(members.len(), 2);
    assert_eq!(members.get("a.wasm").map(Vec::as_slice), Some(&b"wasm-a"[..]));
}

#[test]
fn extract_pack_zip_rejects_directory_entry() {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(buf);
    zip.add_directory("subdir/", zip::write::SimpleFileOptions::default()).expect("add_directory failed");
    let zip_bytes = zip.finish().expect("zip finish failed").into_inner();
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(&zip_bytes).expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let err = extract_pack_zip(tmp.path()).expect_err("a directory entry should be rejected");
    assert!(err.starts_with("pack_invalid:"));
}

#[test]
fn extract_pack_zip_rejects_nested_path_entry() {
    let zip_bytes = write_test_zip(&[("sub/evil.wasm", b"x")]);
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(&zip_bytes).expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let err = extract_pack_zip(tmp.path()).expect_err("a nested path entry should be rejected");
    assert!(err.starts_with("pack_invalid:"));
}

#[test]
fn extract_pack_zip_rejects_unexpected_entry_type() {
    let zip_bytes = write_test_zip(&[("pack.json", b"m"), ("readme.txt", b"hi")]);
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(&zip_bytes).expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let err = extract_pack_zip(tmp.path()).expect_err("a non-manifest, non-.wasm entry should be rejected");
    assert!(err.starts_with("pack_invalid:"));
}

#[test]
fn extract_pack_zip_rejects_missing_manifest() {
    let zip_bytes = write_test_zip(&[("a.wasm", b"x")]);
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(&zip_bytes).expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let err = extract_pack_zip(tmp.path()).expect_err("an archive with no pack.json should be rejected");
    assert!(err.starts_with("pack_invalid:"));
}

#[test]
fn extract_pack_zip_rejects_entry_count_over_cap() {
    let mut entries: Vec<(String, Vec<u8>)> =
        (0..(PACK_MAX_ENTRIES + 1)).map(|i| (format!("m{i}.wasm"), Vec::new())).collect();
    entries.push(("pack.json".to_string(), Vec::new()));
    let entry_refs: Vec<(&str, &[u8])> = entries.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let zip_bytes = write_test_zip(&entry_refs);
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
    tmp.write_all(&zip_bytes).expect("tempfile write failed");
    tmp.flush().expect("tempfile flush failed");

    let err = extract_pack_zip(tmp.path()).expect_err("an archive over the entry cap should be rejected");
    assert!(err.starts_with("pack_invalid:"));
}

// --- check_pack_signature ---

#[test]
fn check_pack_signature_returns_unsigned_when_no_sidecar() {
    let actual = vec![SignedFileEntry { name: "pack.json".to_string(), blake3: "abc".to_string() }];
    assert!(matches!(check_pack_signature(None, &actual), SigCheckResult::Unsigned));
}

#[test]
fn check_pack_signature_returns_valid_for_a_correctly_signed_pack() {
    let signing_key = signing_key_fixture();
    let actual = vec![
        SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"manifest").to_hex().to_string() },
        SignedFileEntry { name: "a.wasm".to_string(), blake3: blake3::hash(b"wasm-a").to_hex().to_string() },
    ];
    let sig_raw = sig_file_json(&signing_key, &actual);
    let result = check_pack_signature(Some(&sig_raw), &actual);
    assert!(matches!(result, SigCheckResult::Valid(_)), "expected a valid signature check result");
}

#[test]
fn check_pack_signature_detects_integrity_mismatch_on_tampered_content() {
    let signing_key = signing_key_fixture();
    let signed = vec![SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"manifest").to_hex().to_string() }];
    let sig_raw = sig_file_json(&signing_key, &signed);
    let actual = vec![SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"tampered-manifest").to_hex().to_string() }];
    let result = check_pack_signature(Some(&sig_raw), &actual);
    assert!(matches!(result, SigCheckResult::IntegrityMismatch));
}

#[test]
fn check_pack_signature_detects_a_file_count_mismatch() {
    let signing_key = signing_key_fixture();
    let signed = vec![SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"manifest").to_hex().to_string() }];
    let sig_raw = sig_file_json(&signing_key, &signed);
    let actual = vec![
        SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"manifest").to_hex().to_string() },
        SignedFileEntry { name: "a.wasm".to_string(), blake3: blake3::hash(b"wasm-a").to_hex().to_string() },
    ];
    let result = check_pack_signature(Some(&sig_raw), &actual);
    assert!(matches!(result, SigCheckResult::IntegrityMismatch));
}

#[test]
fn check_pack_signature_rejects_tampered_signature_bytes() {
    let signing_key = signing_key_fixture();
    let actual = vec![SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"manifest").to_hex().to_string() }];
    let sig_raw = sig_file_json(&signing_key, &actual);
    let mut sig_val: serde_json::Value = serde_json::from_slice(&sig_raw).expect("sig json should parse");
    sig_val["signature"] = serde_json::Value::String(BASE64.encode([0u8; 64]));
    let tampered = serde_json::to_vec(&sig_val).expect("sig json should serialize");

    let result = check_pack_signature(Some(&tampered), &actual);
    assert!(matches!(result, SigCheckResult::SignatureInvalid));
}

#[test]
fn check_pack_signature_rejects_broken_json_and_unknown_schema_version() {
    assert!(matches!(check_pack_signature(Some(b"not json"), &[]), SigCheckResult::Malformed(_)));

    let raw = serde_json::json!({"schema_version": 99, "algorithm": "ed25519", "public_key": "", "files": [], "signature": ""})
        .to_string()
        .into_bytes();
    assert!(matches!(check_pack_signature(Some(&raw), &[]), SigCheckResult::Unrecognized));
}

// --- scan_installed_packs ---

#[test]
fn scan_installed_packs_finds_and_parses_a_valid_manifest() {
    let dir = tempfile::tempdir().expect("tempdir create failed");
    let manifest = sample_manifest_json("com.example.pack", &["a.wasm"]);
    std::fs::write(dir.path().join("com.example.pack.aerini-pack.json"), &manifest).expect("manifest write failed");

    let packs = scan_installed_packs(dir.path());
    assert_eq!(packs.len(), 1, "expected exactly one installed pack to be found");
    assert!(packs.contains_key("com.example.pack"));
}

#[test]
fn scan_installed_packs_skips_a_corrupt_manifest_without_erroring() {
    let dir = tempfile::tempdir().expect("tempdir create failed");
    std::fs::write(dir.path().join("broken.aerini-pack.json"), b"not json").expect("manifest write failed");

    let packs = scan_installed_packs(dir.path());
    assert!(packs.is_empty(), "a corrupt manifest should be skipped, not surfaced as a pack");
}

// --- remove_plugin_pack ---

#[test]
fn remove_plugin_pack_removes_manifest_members_and_sig() {
    let dir = tempfile::tempdir().expect("tempdir create failed");
    let pack_id = "com.example.pack";
    let manifest = sample_manifest_json(pack_id, &["a.wasm", "b.wasm"]);
    std::fs::write(dir.path().join(format!("{pack_id}.aerini-pack.json")), &manifest).expect("manifest write failed");
    std::fs::write(dir.path().join(format!("{pack_id}.aerini-pack.json.sig")), b"sig").expect("sig write failed");
    std::fs::write(dir.path().join("a.wasm"), b"wasm-a").expect("member write failed");
    std::fs::write(dir.path().join("b.wasm"), b"wasm-b").expect("member write failed");

    let result = remove_pack_files(pack_id, &dir.path().display().to_string());
    assert!(result.is_ok(), "expected removal to succeed, got: {result:?}");
    assert!(!dir.path().join(format!("{pack_id}.aerini-pack.json")).exists());
    assert!(!dir.path().join(format!("{pack_id}.aerini-pack.json.sig")).exists());
    assert!(!dir.path().join("a.wasm").exists());
    assert!(!dir.path().join("b.wasm").exists());
}

#[test]
fn remove_plugin_pack_errors_when_pack_not_found() {
    let dir = tempfile::tempdir().expect("tempdir create failed");
    let result = remove_pack_files("com.example.missing", &dir.path().display().to_string());
    assert!(result.is_err(), "expected an error for a pack that was never installed");
}

#[test]
fn remove_plugin_pack_rejects_invalid_pack_id() {
    let dir = tempfile::tempdir().expect("tempdir create failed");
    let result = remove_pack_files("../escape", &dir.path().display().to_string());
    assert!(result.is_err(), "expected an invalid pack_id to be rejected");
}

// --- install_plugin_pack_from_path (early-exit paths only — see module doc) ---

#[test]
fn install_plugin_pack_from_path_rejects_bad_source_file() {
    let dir = tempfile::tempdir().expect("tempdir create failed");

    let mut wrong_ext = tempfile::NamedTempFile::new().expect("tempfile create failed");
    wrong_ext.write_all(b"not a real package").expect("tempfile write failed");
    wrong_ext.flush().expect("tempfile flush failed");
    let result = install_pack_blocking(wrong_ext.path().display().to_string(), dir.path().display().to_string(), false);
    assert!(result.is_err(), "expected a non-.aerinipkg file to be rejected");

    let result = install_pack_blocking("/nonexistent/path/pack.aerinipkg".to_string(), dir.path().display().to_string(), false);
    assert!(result.is_err(), "expected a missing source file to be rejected");
}

#[test]
fn install_plugin_pack_from_path_rejects_malformed_manifest() {
    let manifest = serde_json::json!({
        "schema_version": 2, "pack_id": "com.example.pack", "display_name": "P", "files": ["a.wasm"]
    }).to_string();
    let tmp = write_pack_file(&[("pack.json", manifest.as_bytes()), ("a.wasm", b"x")]);
    let dir = tempfile::tempdir().expect("tempdir create failed");

    let result = install_pack_blocking(tmp.path().display().to_string(), dir.path().display().to_string(), false);
    let err = result.expect_err("unsupported schema_version should be rejected");
    assert!(err.starts_with("pack_invalid:"));
}

#[test]
fn install_plugin_pack_from_path_rejects_content_not_matching_manifest() {
    let manifest = sample_manifest_json("com.example.pack", &["a.wasm", "b.wasm"]);
    let tmp = write_pack_file(&[("pack.json", &manifest), ("a.wasm", b"x")]);
    let dir = tempfile::tempdir().expect("tempdir create failed");

    let result = install_pack_blocking(tmp.path().display().to_string(), dir.path().display().to_string(), false);
    let err = result.expect_err("a member set that doesn't match the manifest should be rejected");
    assert!(err.starts_with("pack_invalid:"));
}

#[test]
fn install_plugin_pack_from_path_rejects_when_already_installed_without_overwrite() {
    let pack_id = "com.example.pack";
    let manifest = sample_manifest_json(pack_id, &["a.wasm"]);
    let dir = tempfile::tempdir().expect("tempdir create failed");
    std::fs::write(dir.path().join(format!("{pack_id}.aerini-pack.json")), &manifest).expect("manifest write failed");
    std::fs::write(dir.path().join("a.wasm"), b"existing-wasm").expect("member write failed");

    let tmp = write_pack_file(&[("pack.json", &manifest), ("a.wasm", b"new-wasm")]);
    let result = install_pack_blocking(tmp.path().display().to_string(), dir.path().display().to_string(), false);
    let err = result.expect_err("reinstalling without overwrite should be rejected");
    assert!(err.starts_with("pack_already_installed:"));
}

#[test]
fn install_plugin_pack_from_path_rejects_integrity_mismatch() {
    let signing_key = signing_key_fixture();
    let manifest = sample_manifest_json("com.example.pack", &["a.wasm"]);
    let signed = vec![
        SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(&manifest).to_hex().to_string() },
        SignedFileEntry { name: "a.wasm".to_string(), blake3: blake3::hash(b"wasm-a").to_hex().to_string() },
    ];
    let sig_raw = sig_file_json(&signing_key, &signed);
    // Ships different member content than what the signature covers.
    let tmp = write_pack_file(&[("pack.json", &manifest), ("a.wasm", b"tampered"), ("pack.json.sig", &sig_raw)]);
    let dir = tempfile::tempdir().expect("tempdir create failed");

    let result = install_pack_blocking(tmp.path().display().to_string(), dir.path().display().to_string(), false);
    let err = result.expect_err("tampered member content should fail the integrity check");
    assert!(err.starts_with("pack_integrity_failed:"));
}

#[test]
fn install_plugin_pack_from_path_rejects_invalid_signature() {
    let manifest = sample_manifest_json("com.example.pack", &["a.wasm"]);
    let bad_sig = serde_json::json!({
        "schema_version": SIG_SCHEMA_VERSION,
        "algorithm": SIG_ALGORITHM,
        "public_key": BASE64.encode([1u8; 32]),
        "files": [
            {"name": "pack.json", "blake3": blake3::hash(&manifest).to_hex().to_string()},
            {"name": "a.wasm", "blake3": blake3::hash(b"wasm-a").to_hex().to_string()},
        ],
        "signature": BASE64.encode([0u8; 64]),
    }).to_string();
    let tmp = write_pack_file(&[("pack.json", &manifest), ("a.wasm", b"wasm-a"), ("pack.json.sig", bad_sig.as_bytes())]);
    let dir = tempfile::tempdir().expect("tempdir create failed");

    let result = install_pack_blocking(tmp.path().display().to_string(), dir.path().display().to_string(), false);
    let err = result.expect_err("a signature that doesn't verify should be rejected");
    assert!(err.starts_with("pack_signature_invalid:"));
}

#[test]
fn install_plugin_pack_from_path_rejects_signature_downgrade_even_with_overwrite() {
    let pack_id = "com.example.pack";
    let dir = tempfile::tempdir().expect("tempdir create failed");

    // Pre-seed the trust store as if a prior install had a verified signature.
    let signing_key = signing_key_fixture();
    let mut store = HashMap::new();
    store.insert(format!("pack:{pack_id}"), BASE64.encode(signing_key.verifying_key().to_bytes()));
    save_trust_store(dir.path(), &store).expect("trust store write failed");

    let manifest = sample_manifest_json(pack_id, &["a.wasm"]);
    let tmp = write_pack_file(&[("pack.json", &manifest), ("a.wasm", b"wasm-a")]); // no pack.json.sig

    // overwrite: true — proves the downgrade guard isn't bypassable by
    // the same flag that authorizes a legitimate update.
    let result = install_pack_blocking(tmp.path().display().to_string(), dir.path().display().to_string(), true);
    let err = result.expect_err("an unsigned update to a previously-trusted pack should be rejected");
    assert!(err.starts_with("pack_signature_downgrade:"));
}

// --- configured plugin folder guard, through managed State ---

fn mock_app_with_db(configured_dir: Option<&Path>) -> (tauri::App<tauri::test::MockRuntime>, tempfile::TempDir) {
    let db_dir = tempfile::tempdir().expect("tempdir create failed");
    let db = WorkflowDb::open(&db_dir.path().join("test.db"), 2).expect("db open failed");
    if let Some(dir) = configured_dir {
        db.set_setting(PLUGIN_DIR_SETTING, &dir.display().to_string()).expect("setting write failed");
    }
    let app = tauri::test::mock_app();
    app.manage(Arc::new(db));
    (app, db_dir)
}

fn install_sample_pack(dir: &Path, pack_id: &str) -> PathBuf {
    let manifest_path = dir.join(format!("{pack_id}.aerini-pack.json"));
    std::fs::write(&manifest_path, sample_manifest_json(pack_id, &["a.wasm"])).expect("manifest write failed");
    std::fs::write(dir.join("a.wasm"), b"wasm-a").expect("member write failed");
    manifest_path
}

#[tokio::test]
async fn remove_plugin_pack_works_in_the_configured_dir() {
    let dir = tempfile::tempdir().expect("tempdir create failed");
    let manifest_path = install_sample_pack(dir.path(), "com.example.pack");
    let (app, _db_dir) = mock_app_with_db(Some(dir.path()));

    let result = remove_plugin_pack("com.example.pack".to_string(), dir.path().display().to_string(), app.state()).await;
    assert!(result.is_ok(), "expected removal to succeed, got: {result:?}");
    assert!(!manifest_path.exists());
}

#[tokio::test]
async fn pack_commands_refuse_a_dir_other_than_the_configured_one() {
    let configured = tempfile::tempdir().expect("tempdir create failed");
    let other = tempfile::tempdir().expect("tempdir create failed");
    let manifest_path = install_sample_pack(other.path(), "com.example.pack");
    let (app, _db_dir) = mock_app_with_db(Some(configured.path()));

    let removed = remove_plugin_pack("com.example.pack".to_string(), other.path().display().to_string(), app.state()).await;
    assert!(matches!(&removed, Err(e) if e.contains("does not match")), "got: {removed:?}");
    assert!(manifest_path.exists(), "a refused removal must leave the pack in place");

    let mut wrong_ext = tempfile::NamedTempFile::new().expect("tempfile create failed");
    wrong_ext.write_all(b"not a real package").expect("tempfile write failed");
    wrong_ext.flush().expect("tempfile flush failed");
    let installed = install_plugin_pack_from_path(
        wrong_ext.path().display().to_string(),
        other.path().display().to_string(),
        false,
        app.state(),
    )
    .await;
    assert!(matches!(&installed, Err(e) if e.contains("does not match")), "the folder check must run before the source file is inspected, got: {installed:?}");
}

#[tokio::test]
async fn pack_commands_refuse_when_no_plugin_dir_is_configured() {
    let dir = tempfile::tempdir().expect("tempdir create failed");
    let manifest_path = install_sample_pack(dir.path(), "com.example.pack");
    let (app, _db_dir) = mock_app_with_db(None);

    let result = remove_plugin_pack("com.example.pack".to_string(), dir.path().display().to_string(), app.state()).await;
    assert!(matches!(&result, Err(e) if e.contains("No plugin folder is set")), "got: {result:?}");
    assert!(manifest_path.exists());
}
