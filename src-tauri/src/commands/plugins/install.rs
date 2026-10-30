use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use aerini_engine::plugin_loader::PluginLoader;
use aerini_engine::plugin_loader::signature::{check_signature, sig_sidecar_path, SigCheckResult, SignedFileEntry};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use super::pack::{check_pack_signature, extract_pack_zip, PACK_MANIFEST_ENTRY_NAME, pack_manifest_path, PACK_MAX_MEMBER_BYTES, parse_pack_manifest, scan_installed_packs};
use super::staging::{read_bounded, scan_type_ids, SIDECAR_MAX_BYTES, STAGED_PREFIX, StagedSidecar, sweep_stale_installs, write_atomic};
use super::trust::{read_trust_store, save_trust_store, TRUST_STORE_LOCK};

pub(super) fn install_plugin_blocking(src_path: String, plugin_dir: String, overwrite: bool) -> Result<String, String> {
    let src = Path::new(&src_path);

    if src.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("wasm")) != Some(true) {
        return Err("Selected file is not a .wasm file".to_string());
    }
    if !src.is_file() {
        return Err(format!("Source file not found: {src_path}"));
    }

    // Only the bare filename is used for the destination, so a crafted
    // src_path cannot place the file elsewhere.
    let filename = src
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| "Source path has no valid filename".to_string())?
        .to_string();

    let dir = PathBuf::from(&plugin_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create plugin directory: {e}"))?;
    sweep_stale_installs(&dir);

    // The source and its signature file are read once. Every check below runs
    // on the staged copy, and that same copy is what gets installed.
    let wasm_bytes = read_bounded(src, PACK_MAX_MEMBER_BYTES)?;
    let sig_src = sig_sidecar_path(src);
    let sig_bytes = if sig_src.is_file() { Some(read_bounded(&sig_src, SIDECAR_MAX_BYTES)?) } else { None };

    let mut staged = tempfile::Builder::new()
        .prefix(STAGED_PREFIX)
        .suffix(".part")
        .tempfile_in(&dir)
        .map_err(|e| format!("Failed to stage plugin: {e}"))?;
    std::io::Write::write_all(&mut staged, &wasm_bytes).map_err(|e| format!("Failed to stage plugin: {e}"))?;
    staged.as_file().sync_all().map_err(|e| format!("Failed to stage plugin: {e}"))?;
    let staged_path = staged.path().to_path_buf();
    let staged_sig = StagedSidecar(sig_sidecar_path(&staged_path));
    if let Some(bytes) = &sig_bytes {
        std::fs::write(&staged_sig.0, bytes).map_err(|e| format!("Failed to stage plugin signature file: {e}"))?;
    }

    let loader = PluginLoader::shared()?;
    let incoming_type_id = loader.describe_plugin(&staged_path).ok().map(|d| d.type_id);
    loader.forget_path(&staged_path);

    // Signature and integrity verification, before anything in `dir` changes.
    // A file with no signature still installs; only corrupted content, an
    // invalid signature, or a publisher key that differs from the one trusted
    // for this type_id blocks.
    let sig_check = check_signature(&staged_path, &wasm_bytes);
    {
        // Holds TRUST_STORE_LOCK for the whole read-decide-write section.
        // Everything inside is synchronous, so the guard is never held across
        // an `.await`.
        let _trust_guard = TRUST_STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        match sig_check {
            SigCheckResult::IntegrityMismatch => {
                return Err(
                    "plugin_integrity_failed: the plugin file does not match its signed checksum (corrupted or tampered download)"
                        .to_string(),
                );
            }
            SigCheckResult::SignatureInvalid => {
                return Err("plugin_signature_invalid: the plugin's signature file is present but does not verify".to_string());
            }
            SigCheckResult::Malformed(reason) => {
                return Err(format!("plugin_signature_invalid: the plugin's signature file is malformed ({reason})"));
            }
            SigCheckResult::Valid(verifying_key) => {
                if let Some(type_id) = &incoming_type_id {
                    let mut store = read_trust_store(&dir)?;
                    let incoming_key_b64 = BASE64.encode(verifying_key.as_bytes());
                    match store.get(type_id) {
                        Some(pinned) if *pinned != incoming_key_b64 => {
                            return Err(format!(
                                "plugin_key_mismatch: '{type_id}' is signed by a different publisher key than the one \
                                 previously trusted for this plugin. Remove the existing install first if you intend to \
                                 trust the new key."
                            ));
                        }
                        Some(_) => {}
                        None => {
                            // First valid signature seen for this type_id — trust on first use.
                            store.insert(type_id.clone(), incoming_key_b64);
                            save_trust_store(&dir, &store)?;
                        }
                    }
                }
                // No type_id (plugin failed to describe): signature verified
                // but there's no identity to pin it to — proceed without
                // touching the trust store.
            }
            SigCheckResult::Unsigned | SigCheckResult::Unrecognized => {
                // Downgrade guard: a type_id previously trusted with a
                // verified signature can't silently update to an
                // unsigned/unrecognized file — that would let an attacker
                // who can't forge a new signature just ship a plain
                // unsigned "update" instead.
                if let Some(type_id) = &incoming_type_id {
                    if read_trust_store(&dir)?.contains_key(type_id) {
                        return Err(format!(
                            "plugin_signature_downgrade: '{type_id}' was previously installed with a verified publisher \
                             signature; this file has none. Remove the existing install first if this is intentional."
                        ));
                    }
                }
            }
        }
    }

    let existing_filename = match &incoming_type_id {
        Some(id) => scan_type_ids(&dir, loader).into_iter().find(|(_, tid)| tid == id).map(|(f, _)| f),
        None => dir.join(&filename).is_file().then(|| filename.clone()),
    };

    let dest = dir.join(&filename);
    let conflict_err = || Err(format!(
        "A different plugin file already exists at '{filename}'. Rename the source file and try again."
    ));

    match existing_filename {
        Some(existing_filename) if !overwrite => {
            let label = incoming_type_id.as_deref().unwrap_or(&filename);
            if label == existing_filename {
                return Err(format!("plugin_already_installed: '{existing_filename}' is already installed"));
            }
            return Err(format!(
                "plugin_already_installed: '{label}' is already installed as '{existing_filename}'"
            ));
        }
        Some(existing_filename) => {
            let existing_path = dir.join(&existing_filename);
            // dest may differ from existing_path (identity matched by type_id under a
            // different filename) — if a *third*, unrelated file already sits at dest,
            // this is a genuine name clash and must not be silently overwritten.
            if dest.exists() && dest != existing_path {
                return conflict_err();
            }
            if existing_path != dest {
                std::fs::remove_file(&existing_path)
                    .map_err(|e| format!("Failed to remove previous version '{existing_filename}': {e}"))?;
                let _ = std::fs::remove_file(sig_sidecar_path(&existing_path));
            }
        }
        None if dest.exists() => return conflict_err(),
        None => {}
    }

    // Re-selecting a plugin that already lives in `plugin_dir` points
    // `src_path` straight at `dest`; its content is already in place.
    let same_file = std::fs::canonicalize(src)
        .ok()
        .zip(std::fs::canonicalize(&dest).ok())
        .is_some_and(|(a, b)| a == b);

    if !same_file {
        let sig_dest = sig_sidecar_path(&dest);
        staged.persist(&dest).map_err(|e| format!("Failed to install plugin: {}", e.error))?;
        if sig_bytes.is_some() {
            std::fs::rename(&staged_sig.0, &sig_dest)
                .map_err(|e| format!("Failed to install plugin signature file: {e}"))?;
        } else {
            // A sidecar left by an earlier install would be matched to this file.
            match std::fs::remove_file(&sig_dest) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Failed to remove the old plugin signature file: {e}")),
            }
        }
    }

    Ok(filename)
}

pub(super) fn install_pack_blocking(src_path: String, plugin_dir: String, overwrite: bool) -> Result<String, String> {
    let src = Path::new(&src_path);

    if src.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("aerinipkg")) != Some(true) {
        return Err("Selected file is not a .aerinipkg package".to_string());
    }
    if !src.is_file() {
        return Err(format!("Source file not found: {src_path}"));
    }

    let (manifest_bytes, sig_bytes, members) = extract_pack_zip(src)?;
    let manifest = parse_pack_manifest(&manifest_bytes)?;
    let expected_files: HashSet<&str> = manifest.files.iter().map(String::as_str).collect();
    let actual_files: HashSet<&str> = members.keys().map(String::as_str).collect();
    if expected_files != actual_files {
        return Err(format!(
            "pack_invalid: package contents do not match its manifest for '{}'",
            manifest.pack_id
        ));
    }

    // Entries the signature (if any) must cover: the manifest itself, plus
    // every member — same `{name, blake3}` shape as the single-file path,
    // just N of them instead of one.
    let mut actual_entries = vec![SignedFileEntry {
        name: PACK_MANIFEST_ENTRY_NAME.to_string(),
        blake3: blake3::hash(&manifest_bytes).to_hex().to_string(),
    }];
    for (name, bytes) in &members {
        actual_entries.push(SignedFileEntry { name: name.clone(), blake3: blake3::hash(bytes).to_hex().to_string() });
    }
    let sig_check = check_pack_signature(sig_bytes.as_deref(), &actual_entries);

    let dir = PathBuf::from(&plugin_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create plugin directory: {e}"))?;

    let trust_key = format!("pack:{}", manifest.pack_id);
    {
        // Same lock, same read-decide-write shape as `install_plugin_from_path`
        // above — see `TRUST_STORE_LOCK`'s doc comment. A `pack:` prefix on
        // the trust-store key keeps pack identity in a disjoint namespace
        // from plain `type_id` pins sharing the same flat store, since a
        // `pack_id` and a `type_id` both pass the same charset check and
        // could otherwise collide on the same string.
        let _trust_guard = TRUST_STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        match sig_check {
            SigCheckResult::IntegrityMismatch => {
                return Err(
                    "pack_integrity_failed: the package's contents do not match its signed checksums (corrupted or tampered download)"
                        .to_string(),
                );
            }
            SigCheckResult::SignatureInvalid => {
                return Err("pack_signature_invalid: the package's signature file is present but does not verify".to_string());
            }
            SigCheckResult::Malformed(reason) => {
                return Err(format!("pack_signature_invalid: the package's signature file is malformed ({reason})"));
            }
            SigCheckResult::Valid(verifying_key) => {
                let mut store = read_trust_store(&dir)?;
                let incoming_key_b64 = BASE64.encode(verifying_key.as_bytes());
                match store.get(&trust_key) {
                    Some(pinned) if *pinned != incoming_key_b64 => {
                        return Err(format!(
                            "pack_key_mismatch: '{}' is signed by a different publisher key than the one previously \
                             trusted for this pack. Remove the existing install first if you intend to trust the new key.",
                            manifest.pack_id
                        ));
                    }
                    Some(_) => {}
                    None => {
                        store.insert(trust_key.clone(), incoming_key_b64);
                        save_trust_store(&dir, &store)?;
                    }
                }
            }
            SigCheckResult::Unsigned | SigCheckResult::Unrecognized => {
                if read_trust_store(&dir)?.contains_key(&trust_key) {
                    return Err(format!(
                        "pack_signature_downgrade: '{}' was previously installed with a verified publisher signature; \
                         this package has none. Remove the existing install first if this is intentional.",
                        manifest.pack_id
                    ));
                }
            }
        }
    }

    let existing_packs = scan_installed_packs(&dir);
    let existing_manifest = existing_packs.get(&manifest.pack_id).cloned();
    if existing_manifest.is_some() && !overwrite {
        return Err(format!("pack_already_installed: '{}' is already installed", manifest.pack_id));
    }

    // Everything past this point needs to instantiate each member to read
    // its `type_id` for conflict checking, which requires a real path on
    // disk (`PluginLoader::describe_plugin` takes `&Path`, with no
    // in-memory-bytes entry point). Staged into a tempdir
    // rather than `plugin_dir` itself so a member that fails every check
    // below never touches the real install directory.
    let loader = PluginLoader::shared()?;
    let staging = tempfile::tempdir().map_err(|e| format!("Failed to create a staging directory: {e}"))?;
    let mut member_type_ids: HashMap<String, String> = HashMap::new();
    for (name, bytes) in &members {
        let staged_path = staging.path().join(name);
        std::fs::write(&staged_path, bytes).map_err(|e| format!("Failed to stage package member '{name}': {e}"))?;
        if let Ok(desc) = loader.describe_plugin(&staged_path) {
            member_type_ids.insert(name.clone(), desc.type_id);
        }
        loader.forget_path(&staged_path);
        // A member that fails to describe is still installed (matching
        // `install_plugin_from_path`'s "install a broken plugin so the UI
        // can surface why" behavior) — it just has no `type_id` to run a
        // conflict check against below.
    }

    // Internal duplicate: two members of *this same* incoming pack claiming
    // the same `type_id`. Not a soft, last-loaded-wins collision like a
    // plugin-directory-wide `type_id` clash — a self-consistent package
    // should never ship this, so it's rejected as malformed rather than
    // silently resolved by load order.
    let mut seen_type_ids: HashMap<&str, &str> = HashMap::new();
    for (name, type_id) in &member_type_ids {
        if let Some(other_name) = seen_type_ids.insert(type_id.as_str(), name.as_str()) {
            return Err(format!(
                "pack_invalid: members '{other_name}' and '{name}' both export type_id '{type_id}'"
            ));
        }
    }

    // filename -> owning pack_id, across every *other* installed pack (this
    // pack's own prior install, if any, is expected to already own its own
    // member filenames/type_ids — that's an update, not a conflict).
    let mut filename_owner: HashMap<&str, &str> = HashMap::new();
    for (pid, m) in &existing_packs {
        for f in &m.files {
            filename_owner.insert(f.as_str(), pid.as_str());
        }
    }
    let is_own_pack = |owner: Option<&&str>| owner == Some(&manifest.pack_id.as_str());

    let existing_type_ids = scan_type_ids(&dir, loader);
    let mut type_id_owner: HashMap<&str, &str> = HashMap::new();
    for (f, tid) in &existing_type_ids {
        type_id_owner.insert(tid.as_str(), f.as_str());
    }

    for (name, type_id) in &member_type_ids {
        if let Some(owner_filename) = type_id_owner.get(type_id.as_str()) {
            if !is_own_pack(filename_owner.get(owner_filename)) {
                return Err(format!(
                    "pack_member_conflict: '{type_id}' (member '{name}') is already provided by another installed plugin or pack"
                ));
            }
        }
    }
    for name in members.keys() {
        if dir.join(name).exists() && !is_own_pack(filename_owner.get(name.as_str())) {
            return Err(format!(
                "pack_member_conflict: filename '{name}' is already used by another installed plugin or pack"
            ));
        }
    }

    // All checks passed — mutate. If this is an update, drop members the
    // new manifest no longer lists first (best-effort; the manifest/member
    // writes below are what must succeed for the install to count as done).
    if let Some(old) = &existing_manifest {
        for old_file in &old.files {
            if !expected_files.contains(old_file.as_str()) {
                let _ = std::fs::remove_file(dir.join(old_file));
                let _ = std::fs::remove_file(sig_sidecar_path(&dir.join(old_file)));
            }
        }
    }
    for (name, bytes) in &members {
        write_atomic(&dir.join(name), bytes).map_err(|e| format!("Failed to install package member '{name}': {e}"))?;
    }
    let manifest_dest = pack_manifest_path(&dir, &manifest.pack_id);
    write_atomic(&manifest_dest, &manifest_bytes).map_err(|e| format!("Failed to write pack manifest: {e}"))?;

    let sig_dest = sig_sidecar_path(&manifest_dest);
    match &sig_bytes {
        Some(bytes) => write_atomic(&sig_dest, bytes).map_err(|e| format!("Failed to write pack signature file: {e}"))?,
        None if sig_dest.is_file() => {
            let _ = std::fs::remove_file(&sig_dest);
        }
        None => {}
    }

    Ok(manifest.pack_id)
}
