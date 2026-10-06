# PLAN_UPDATER.md — In-app updater (batch series U)

> Separate from the main `PLAN.md` v2 so it cannot overwrite it. When approved, merge the batch list into `PLAN.md` as a new section.
> Baseline: Aerini 0.4.1, Tauri 2 (lockfile: `tauri` 2.11.5), Panchak2d/aerini, releases published manually from GitHub drafts.
> Confidence tags per `rules.md` Rule 13. Nothing here was compiled or run. Only source-level and doc-level verification was possible.

---

## 0. Batch status

- [x] **U0** — delivered; owner build/test passed (`cargo test -p aerini-engine`: 794 passed, 0 failed, 4 ignored live-DB; `cargo build --workspace`: OK). Manual macOS Deny test not run: no Mac available
- [x] **U1** — delivered; owner build/test/audit all passed
- [x] **U2** — delivered, not compiled (owner build/test pending; see U2 section)
- [x] **U3** — delivered; JS side run here (`tsc --noEmit` clean, `vitest run` 679 passed). One Rust edit (removed `is_newer` from `update.rs`) not compiled; owner `cargo build`/`cargo test --workspace` pending. Owner also re-runs `npm run vite:build` and recommits `dist/`
- [x] U4 — delivered, not run: no workflow, script or build was executed on GitHub or the owner's machine. Python script tests and the manifest job's jq/shell logic were run in the sandbox only. Owner: push to a test fork and run the real release flow (U5) before any production tag
- [ ] U5 (human-run)

---

## 1. Goal and non-goals

**Goal:** a user clicks one button in Settings → About → Updates and Aerini downloads, verifies, installs, and restarts into the newer version. No manual trip to GitHub.

**Non-goals (this series):**
- Automatic or background update checks (privacy contract, see D1).
- Apple Developer ID signing/notarization, Windows code signing, Intel Mac build, beta channels (see §9 backlog).
- Any change to the workflow engine or server crate.

---

## 2. Decisions (made, with reasons and rejected alternatives)

| # | Decision | Why (UX / security / maintainability / scalability) | Rejected |
|---|---|---|---|
| D1 | **Click-only check.** No startup check, no poller. The existing "no automatic outbound connection" claim in `docs/guide/security.md` stays true. | Privacy is the product's stated posture; keeps docs honest. Opt-in startup check can be added later with no migration (B-U1). | Default-on startup check (breaks the claim). |
| D2 | **Official `tauri-plugin-updater`**, used **Rust-side only**. Register the plugin, call `UpdaterExt` from our own commands. **No `updater:*` capability** granted to the webview, **no `@tauri-apps/plugin-updater` npm dependency**. | Webview (renders workflow/chat content) can't trigger plugin install commands directly; every install passes our guards. Mirrors the existing `tauri-plugin-fs` Rust-only convention in `Cargo.toml`. Fewer deps. | JS plugin API (wider webview privilege); custom downloader (reinvents signature verification, per-OS install logic). |
| D3 | **Two-step user flow:** `check_for_update` (click) → shows version → user clicks **Install** → `install_update`. Update handle from the check is kept in managed state; `install_update` takes **no parameters** (webview never supplies a URL or version). | User sees what they're installing; nothing remote-controlled crosses IPC. | Single "check and install" button (no consent step). |
| D4 | **`download()` then `install(bytes)`**, not `download_and_install`. | Signature is verified inside `download` (VERIFIED, plugin source). Workflows keep running during a slow download; we only stop work at the instant of install, after verification passed. | `download_and_install` (stops nothing until it exits, but gives no seam to guard). |
| D5 | **`requireSignedVersion: true` from the first updater release.** | The manifest is not signed; without this a crafted manifest can pair a high version with an older validly-signed artifact (forced downgrade). No legacy releases exist to re-sign. Needs tauri CLI ≥ 2.12.0 to record the version in each signature (VERIFIED, CLI changelog). | Leaving it off (default). |
| D6 | **Endpoint:** `https://github.com/Panchak2d/aerini/releases/latest/download/latest.json`. | "Latest" excludes drafts and prereleases, so the **manual publish of the draft is the release gate** and stable users never see `-rc` tags. Browser-download URLs are served via CDN redirect, not the REST API. | Own server/Pages (extra infra, extra attack surface). Lock-in note: endpoints are compiled into installed apps; moving hosts later needs a bridging release (array allows fallbacks). |
| D7 | **Generate `latest.json` in our own final CI job**, not via `tauri-action`'s `uploadUpdaterJson`. Set `uploadUpdaterJson: false` on the matrix. | tauri-action 1.0.0 writes `api.github.com/.../releases/assets/{id}` URLs into `latest.json` (VERIFIED, its source + changelog). Unauthenticated REST API is limited to 60 req/h per IP (HIGH; header observed) → shared-NAT users (offices, campuses) would hit failures at scale. It also read-modify-writes one file from 4 parallel matrix jobs (race risk, HIGH, untested). A single-writer job uses stable `https://github.com/.../releases/download/<tag>/<name>` URLs, and can assert completeness. | Keep tauri-action's JSON (rate-limit + race exposure). |
| D8 | **Typed platform keys only** (`{os}-{arch}-{bundle}`: `darwin-aarch64-app`, `windows-x86_64-nsis`, `windows-x86_64-msi`, `linux-x86_64-appimage`/`deb`/`rpm`, same for `aarch64`). No generic `{os}-{arch}` fallback keys. | Each installed binary is stamped with its own bundle type by the bundler and looks up its own key (VERIFIED, bundler + plugin source). An MSI install gets the MSI, an NSIS install gets the NSIS, a `.deb` gets the `.deb`: no cross-installer duplicates. A generic fallback could feed AppImage bytes to `dpkg`. If a type isn't found, the plugin errors and the UI falls back to "download manually". | Generic fallback keys. |
| D9 | **Fail-safe fallback to manual download** on any unsupported/failed path, with a link built from constants (`https://github.com/Panchak2d/aerini/releases/tag/v{version}`), not from API data. | An updater must never leave the user stuck. Removes the need for the current GitHub-API call + host validation. | Surface raw error only. |
| D10 | **Install only after confirming no workflow is running** (see §4 U2). Same shutdown semantics as the tray's Quit. | In-flight runs/Code-node sidecar must not be killed silently; Windows file locks on `node-bundled.exe`. | Silent install. |
| D11 | **Key in an Actions *environment* (`release`) with required reviewer + password-protected key.** Private key only in the build matrix job; manifest job needs no secret. | Limits who/what can reach the key. If lost: existing installs can never update (reinstall by hand). If leaked: attacker can push malicious updates. Treat as the most valuable secret. | Repo-level secret (any workflow edit on a tag can read it). |
| D12 | **Marker file** `data_dir/.update-pending` written just before install; read at next startup to (a) force-show the window even if launched `--minimized`, (b) show "Updated to vX", (c) detect an install that did not take effect. | Autostart passes `--minimized`; a restart after a button click must not vanish into the tray. | Nothing (surprise-hidden app). |
| D13 | **Notes are plain text only** (`textContent`), fixed manifest `notes` string + "View release notes" link. | Manifest content is remote-controlled; never `innerHTML`. Maintainer edits release body on the page after the fact, so a copied body would go stale. | Rendering manifest notes as HTML/Markdown. |
| D14 | **Keychain hardening before enabling macOS in-app updates (batch U0).** | See R1. A pre-existing hazard that the updater makes likely. | Ship updater and document it. |
| D15 | **macOS in-app update stays disabled until a Mac owner verifies it.** The `.app.tar.gz` is still built and signed, but `darwin-aarch64-app` is left out of the manifest's expected-key list (one constant in `scripts/build-updater-manifest.py`). macOS users take the manual-download path. No `cfg`-gating in the binary. Enabling later = add the key to that constant after the U5 macOS rows pass. | The owner has no Mac. The macOS replace-in-place, Keychain re-prompt and Gatekeeper behavior is tagged HIGH, not VERIFIED (§3, R1, R10). An update path that can re-prompt Keychain or fail on an untested OS is worse than a manual download. One constant keeps the switch cheap and reversible. | (a) Include the darwin key unverified. (c) Hosted macOS CI as a substitute for a human test: it can only build and unit-test, so it complements this decision but does not replace the U5 macOS rows. |
| D16 | **`bundle.createUpdaterArtifacts` moves from U1 to U4.** U1 ships the dependency, plugin registration and `plugins.updater` config only. | tauri-cli 2.12.1 `bundle.rs::sign_updaters` fails the build ("A public key has been found, but no private key") when the flag is on, a pubkey is configured and `TAURI_SIGNING_PRIVATE_KEY` is unset, unless `--no-sign` is passed (VERIFIED, CLI source). CI `tauri-build` (`npm run build`) and `release.yml` have no key until U4, so the flag in U1 would turn both red and break local `npx tauri build`. Moving it keeps U1 behavior-neutral and green, and the flag lands with the key plumbing it needs. | Keep it in U1 and also edit `ci.yml`/`release.yml` (breaks the U1/U4 separation; no release could be cut in between). `--no-sign` in the npm script (hides a missing key in the release build). |

---

## 3. Verified facts (sources)

| Fact | Source | Tag |
|---|---|---|
| `tauri-plugin-updater` 2.13.1 is latest stable (2026-09-30); requires `tauri ^2.12`; MSRV 1.90 (repo: 1.96) | crates.io API + crate `Cargo.toml` | VERIFIED |
| Repo lockfile has `tauri` 2.11.5; `@tauri-apps/cli ^2.11.3`; `@tauri-apps/api ~2.11.1` → all must move to ≥ 2.12 | `Cargo.lock`, `package.json` | VERIFIED |
| Latest `tauri` crate and `@tauri-apps/cli` are 2.12.1 | crates.io, npm registry | VERIFIED |
| Plugin supports AppImage, **deb, rpm** (since 2.10.0), NSIS, MSI, macOS `.app` | plugin CHANGELOG + `updater.rs` | VERIFIED |
| Lookup order: `{os}-{arch}-{bundle}` then `{os}-{arch}` | `updater.rs::get_urls` | VERIFIED |
| Signature verified inside `download()`; `Accept: application/octet-stream` set by default | `updater.rs` | VERIFIED |
| `Update.timeout` is a public field; reqwest `timeout` is total request time, so a short timeout would kill a large download | `updater.rs` + reqwest semantics | VERIFIED / HIGH |
| Windows: install ends in `std::process::exit(0)`; `on_before_exit` hook exists; NSIS gets `/UPDATE`; `restart_after_install` option since 2.11.0 | `updater.rs`, CHANGELOG | VERIFIED |
| macOS: extracts `.app.tar.gz`, `rename` over the bundle; if that fails, `osascript … with administrator privileges` | `updater.rs` | VERIFIED |
| Linux deb/rpm install via `pkexec` → zenity GUI sudo → `sudo` | `updater.rs` | VERIFIED |
| `requireSignedVersion` exists in plugin 2.13.1 config; CLI records version in the signature's trusted comment from CLI 2.12.0 | crate source; CLI CHANGELOG | VERIFIED (section attribution HIGH) |
| `allowDowngrades` is config-only, default false (since 2.12.0) | plugin CHANGELOG | VERIFIED |
| tauri-action 1.0.0: `latest.json` URLs use GitHub API asset endpoint; `uploadUpdaterJson` default true; `.sig` files uploaded by default | tauri-action source/CHANGELOG/`action.yml` | VERIFIED |
| `createUpdaterArtifacts: true` signs `.deb`, `.rpm`, AppImage, MSI, NSIS, and `.app.tar.gz` directly | CLI CHANGELOG + bundler source | VERIFIED |
| Ad-hoc-signed macOS apps lose Keychain/TCC "Always Allow" on every update (cdhash changes) | 3 independent projects (Driven PR #214, EVE-industry wiki, ClawChat release notes) | HIGH — not tested on Aerini |
| Updater runs in Rust (reqwest), so **no CSP `connect-src` change** is needed | plugin architecture | HIGH |
| `releases/latest` excludes drafts and prereleases | GitHub semantics | HIGH |
| Windows default install mode is `passive`; NSIS default is per-user (no UAC), MSI prompts UAC | Tauri docs | HIGH — confirm in U5 |
| After a plain HTTP download, macOS doesn't quarantine the file, so Gatekeeper shouldn't re-fire on update | reasoning | HIGH — confirm in U5 |

Repo facts relied on: close-to-tray + background scheduler (`lib.rs`), `force_quit`/tray Quit call `daemon.stop_all()` then `app.exit(0)`, `SchedulerDaemon::active_runs()` and `drain_all()` exist, autostart passes `--minimized`, Windows ships MSI **and** NSIS, Linux ships `.deb` + AppImage (`targets: "all"` may also produce `.rpm`: confirm), macOS is arm64-only, Node sidecar `node-bundled`, credential key in OS keychain with file fallback.

---

## 4. Risk register

| ID | Risk | Likelihood / impact | Mitigation | Test |
|---|---|---|---|---|
| **R1** | **macOS Keychain re-prompt after each update (ad-hoc signing) + existing fallback hazard.** In `store.rs::key_from_keychain`, any keychain read error other than `NoEntry` falls through to `key_from_file`, which **generates a fresh key** if no file exists. If a user clicks **Deny** (or the prompt fails) after an update, credentials become undecryptable and new ones get encrypted with a different key (split-brain). | HIGH likelihood on macOS / HIGH impact (credential loss) | **U0 (as built)**: a new key is generated only when the credentials table is empty, on every path (keychain `NoEntry`, any other keychain error, file source). With rows present and no key anywhere, `open` returns `EngineError::KeyUnavailable`; desktop startup shows a dialog + clean exit instead of a panic. This also covers `NoEntry` + rows + no file, which the original wording would have regenerated. Docs: warn macOS users to choose **Always Allow**; the real fix is Developer ID signing (B-U4). | Manual macOS: update, Deny, relaunch, Allow; unit tests for key-source decision. |
| R2 | Update installs while workflows run; Windows `node-bundled.exe` locked. | MED / MED | Active-run guard (U2): `active_runs()` + manual-run tokens; confirm dialog; `stop_all()` then install via `on_before_exit`. | Win: update during a long Code node. |
| R3 | Restart races: new instance binds webhook/OAuth ports before old releases them; always-on jobs fail to restore. | MED / MED | `stop_all()` + `on_before_exit` cleanup before restart; verify listeners closed. | Always-on webhook survives update on all OSes. |
| R4 | `--minimized` autostart → app returns hidden after update. | MED / LOW | D12 marker. | Autostart-launched update test. |
| R5 | Signing key lost/leaked. | LOW / CRITICAL | D11; offline backup in 2 places; rotation procedure documented (single `pubkey` in config → bridging release signed with old key carrying the new key). | Dry-run rotation on test fork. |
| R6 | A release ships with empty/wrong `pubkey` or no updater artifacts → that version can never self-update. | LOW / HIGH | `verify` job asserts `createUpdaterArtifacts == true`, `pubkey` non-empty, endpoint matches repo. | CI failure on a bad config. |
| R7 | Matrix build misses a platform artifact → `latest.json` incomplete → some users get "not found". | MED / MED | D7 job fails the workflow if any expected key is missing; summary lists keys before publish. | Remove an asset, expect failure. |
| R8 | Large download (bundle includes Node, tens of MB) on flaky network. | MED / LOW | Own timeouts (check 15 s, download generous), progress events, Cancel, error → manual link; temp file cleanup. | Kill network mid-download. |
| R9 | Linux `.deb`/`.rpm` update needs polkit/zenity/sudo; fails on minimal systems. | MED / LOW | Pre-warn "admin password required"; on failure show manual link. | Ubuntu with and without polkit. |
| R10 | macOS run from DMG / Downloads (App Translocation) or non-writable location → replace fails or admin prompt. | MED / LOW | Detect `/AppTranslocation/` or `/Volumes/` → "Move to Applications first" and manual link. | Run from DMG. |
| R11 | Bootstrap: 0.4.1 has no updater. First updater release must be installed by hand once. | CERTAIN / LOW | State it in release notes + `updating.md`; 0.4.1's link-out check still works. | n/a |
| R12 | Tauri 2.11.5 → 2.12.x bump regresses something. | LOW / MED | Isolated in U1 with full CI/build before anything depends on it. | CI + smoke. |
| R13 | New transitive deps: duplicate TLS stack (rustls via plugin vs. existing reqwest), `cargo audit` failures, binary size. | MED / LOW | `cargo tree -d`; check features; `cargo audit`; note size delta. | CI `security-audit`. |
| R14 | DB schema migrates forward on first run of new version; no downgrade. | LOW / LOW | Already how releases behave; mention in `updating.md` only if not already covered. | n/a |
| R15 | Prerelease users can't use in-app update (by design). | CERTAIN / LOW | Document. | n/a |

---

## 5. Batches

Order is strict. One batch per chat. Each batch follows `AGENT_PROTOCOL.md` (research only what is new, trace, self-audit, retrospective scan, docs-sync check, patch notes, `MANIFEST.txt`). Docs ship in the same patch as the code that changes them (Rule 30). No batch ID, rule citation, or change narration in code comments (Rule 1).

### U0 — Keychain-denied hardening (precondition) — DELIVERED, not compiled
- [x] `EngineError::KeyUnavailable`; `open` counts rows before loading the key; guard in `key_from_file`, `key_from_keychain` and a shared failure helper
- [x] Desktop: dialog + exit for credential-store and workflow-DB open failures
- [x] Tests: refuse-with-rows, reuse-existing-with-rows, restore-after-loss rewritten to expect refusal then recovery (zero-rows generation already covered by the existing generate/reload test)
- [x] Docs: `credentials.md`, `known-issues.md`, `server-deploy.md`, `embedding.md`, `CHANGELOG.md`
- [x] Owner: `cargo test -p aerini-engine` (794 passed, 0 failed, 4 ignored live-DB), `cargo build --workspace`: OK
- [ ] Manual macOS Deny → relaunch → Allow (no Mac available; hidden-webview race in the failure path is untested)
- Repo corrections: the server also uses `KeySource::OsKeychain` with `--keychain`; the server now exits on `KeyUnavailable` via its existing `fatal()`. No server code changed.

- **Files:** `aerini-engine/src/store.rs` (+ its tests); desktop startup path in `src-tauri/src/lib.rs` where `CredentialStore::open` is called; `docs/guide/credentials.md`, `docs/known-issues.md`.
- **Do:** implement the rule in R1. First read how `open` failure propagates today in `lib.rs` setup and how the DB reports "has encrypted rows"; keep the change minimal.
- **Must not break:** Linux with no Secret Service (file fallback keeps working, existing key file reused); server (`KeySource::File`); keychain-to-file migration branch.
- **Open check (UNCERTAIN):** which `keyring` error variant macOS returns on Deny; handle all non-`NoEntry` errors identically.
- **Tests:** (a) non-`NoEntry` error + rows present + no file → error, no file created; (b) error + file exists → file key used; (c) error + zero rows → file key generated.
- **Done when:** cargo tests pass (on user's machine), docs updated.

### U1 — Dependency and config foundation
- **Files:** `Cargo.toml` (workspace + `src-tauri`), `Cargo.lock`, `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`, `src-tauri/src/lib.rs` (plugin registration only), `.cargo/audit.toml` if needed.
- **Do:** bump `tauri`, `@tauri-apps/cli`, `@tauri-apps/api` to a matching ≥ 2.12 set; add `tauri-plugin-updater` 2.13.x as a **desktop-only** dependency; register the plugin; config (`bundle.createUpdaterArtifacts` is deferred to U4, see D16):
  ```json
  "plugins": { "updater": {
    "pubkey": "<from tauri signer generate>",
    "endpoints": ["https://github.com/Panchak2d/aerini/releases/latest/download/latest.json"],
    "requireSignedVersion": true } }
  ```
  Do not add any `updater:*` permission to `capabilities/default.json`. Do not add the npm plugin package.
- **Human prerequisite (before this batch can finish):** owner runs `npx tauri signer generate -w <path>` with a **password**, supplies the **public** key only. Private key stays out of every chat and the repo.
- **Check:** `cargo tree -d` (duplicate TLS), `cargo audit`, `.cargo/audit.toml` ignores, `.deb`/`.rpm` bundle targets actually produced on Linux CI.
- **Tests:** none new (no behavior); existing suites must pass. Real build is on the user's machine (LAUNCH GATE).
- **As built (not compiled):** `src-tauri/Cargo.toml` floors tauri 2.12, tauri-build 2.7, dialog 2.8, fs 2.6, autostart 2.7, `tauri-plugin-updater` 2.13 in a `cfg(not(any(target_os = "android", target_os = "ios")))` table; `lib.rs` registers the plugin under `#[cfg(desktop)]`; `tauri.conf.json` gains `plugins.updater` (pubkey, endpoint, `requireSignedVersion`) and loses the unused `plugins.shell` entry (no `tauri-plugin-shell` crate or JS use exists); `package.json` cli/api `^2.12.1`; both lockfiles resolved (`Cargo.lock`: tauri 2.12.1, updater 2.13.1, wry 0.57.0, tao 0.37.1; reqwest/rustls/aws-lc-rs/ring unchanged; `zip` 4.6.1 added beside 8.6.0 via the updater's `zip` feature); `.cargo/audit.toml`: five `unic-*` ignores removed (crates no longer in the lock), tauri version text corrected.
- **Sandbox-verified:** `npm ci`, `tsc --noEmit` clean, `vitest run` 89 files / 676 tests passed on `@tauri-apps/api` 2.12.1; `cargo metadata --locked` succeeds. Not run anywhere: any `cargo build/test/audit`.
- **Owner:** run `npm run vite:build` and recommit `dist/` (the api bump can change the bundle); `cargo tree -d`; `cargo audit`; compare binary size before/after.
- [x] U1 delivered; owner ran the full U1 checklist, every step passed

### U2 — Rust update commands
- **Files:** `src-tauri/src/commands/update.rs` (rewrite), `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs` (managed state, handler list, startup marker handling), `docs/development/desktop-ipc-reference.md`.
- **Commands:**
  - `check_for_update() -> { current_version, available, latest_version?, install_support, release_url }`: updater with 15 s timeout; stores `Update` in managed state; `install_support` from `tauri::utils::platform::bundle_type()` + OS checks (macOS translocated/DMG path → manual; Linux AppImage needs writable `$APPIMAGE`; `None` bundle type → manual).
  - `install_update() -> ()`: requires stored `Update`; single-flight guard; set `update.timeout` generous; `download()` with progress emitted as `update-progress {downloaded,total}`; Cancel via `tokio_util` cancellation; **then** active-run guard; write marker; `stop_all()`; `install(bytes)`; on non-Windows `app.restart()`.
  - `cancel_update_download()`.
  - Active-run guard: if `active_runs() > 0` (or manual run tokens), return a distinct result so the UI can confirm and re-invoke with an explicit `force: true`.
  - Use the updater builder's `on_before_exit` for cleanup.
- **Startup:** if marker exists → show window regardless of `--minimized`, emit `update-installed {version}` (or `update-incomplete`), delete marker.
- **Remove:** the old GitHub-API check and `parse_semver` if now unused (Rule 25); keep a pure `release_url(version)` helper.
- **Tests (pure logic only):** support classification (bundle type × path), `release_url` builder, marker parse/expiry. Don't unit-test the plugin.
- **Docs:** `desktop-ipc-reference.md` command table.
- **As built (not compiled):**
  - `update.rs` rewritten: `check_for_update`, `install_update(force)`, `cancel_update_download`, plus `take_update_notice`. Old GitHub-API check, `parse_semver` and the `reqwest`/`url` dependencies of `src-tauri` removed (`Cargo.toml`, `Cargo.lock` aerini entry; `cargo metadata --locked` passes).
  - **Deviation 1, pull instead of event for the startup notice:** an event emitted from `setup` can fire before the page registers a listener and is never replayed. `take_update_notice()` returns `{kind: "installed"|"incomplete", version}` once. U3 must call it at startup instead of listening for `update-installed`/`update-incomplete`.
  - **D12(a) premise is false:** nothing reads `--minimized` (the flag is passed by autostart only; the window is created hidden and shown by `show_main_window` plus a 3 s fallback). No forced show was added. If a later batch implements `--minimized`, it must also show the window when `take_update_notice()` returns a notice.
  - **D15 mechanism hazard (fixed in U2):** the plugin resolves the platform entry before it compares versions, so a manifest without `darwin-aarch64-app` makes every macOS check fail, even when up to date. `check_for_update` installs a version comparator that records the announced version (the comparator runs before the platform lookup, VERIFIED, `updater.rs::check`). On `TargetNotFound`/`TargetsNotFound` it reports `available` plus `install_support: manual / no_release_for_platform` when the version is newer, or "up to date" when not.
  - **Staged download:** the verified bytes stay in managed state after the active-run prompt and after a failed install, so confirming or retrying does not download again. `cancel_update_download` while idle discards them.
  - **Jobs are stopped late, not early:** on Unix `stop_all` and manual-run cancellation happen only after `install` succeeded, so a cancelled admin prompt leaves everything running. On Windows the plugin's `on_before_exit` hook does it immediately before the installer launches; if launching fails the jobs are re-armed with `SchedulerDaemon::start`.
  - `ActiveRunToken` gained `active_count()` and `cancel_all()`.
  - Download stall limit 45 s (no bytes), hard limit 30 min (`update.timeout`; the plugin leaves it `None`), check timeout 15 s.
  - `check_for_update` also returned `is_newer` (= `available`) for the old toolbar handler. Removed in U3 (Rust struct, TS interface, IPC reference).
- **Until U4 publishes a `latest.json`, every check fails with "No update information is published yet"** (a 404 on the endpoint). Do not cut a release between U2 and U4.
- **Docs left stale on purpose (U3):** `updating.md`, `security.md`, `faq.md` still said the check calls GitHub's Releases API. Fixed in U3.

### U3 — Frontend and user-facing docs — DELIVERED, Rust edit not compiled
- **Files:** `src/ipc/update.ts`, `src/toolbar.ts` (handler at ~line 346), `src/index.html` (Updates row, ~line 636), related CSS, new `src/__tests__/update-ui.test.ts`; `docs/operations/updating.md`, `docs/guide/security.md` (rewrite the update paragraph; "no auto-updater" sentence becomes false), `docs/faq.md` (update question), `docs/getting-started/installation.md` (one note), `docs/known-issues.md` (macOS Keychain re-prompt until Developer ID signing), `CHANGELOG.md` (`[Unreleased]` → Added/Changed, matching existing entry style).
- **IPC contract from U2:** `install_update` resolves only for `{status: "active_runs", scheduled, manual}` (confirm, then re-invoke with `force: true`) or `{status: "cancelled"}`; on success the app restarts and the promise never resolves. `update-progress` payload is `{stage: "downloading"|"installing", downloaded, total|null}`. `install_support.kind === "manual"` carries a `message` to show verbatim via `textContent`; `install_support.admin_prompt` drives the "admin password required" pre-warning. Remove the `is_newer` field from `src/ipc/update.ts`.
- **UI states:** idle → checking → up-to-date | available (version, "View release notes", **Install and restart**) | manual-only (reason + "Download from GitHub") → confirm-if-running → downloading (progress, Cancel) → installing/restarting → error (message + manual link). Whole row `aria-live="polite"`. Extract state rendering into a pure function for testing. Notes/version/reason inserted via `textContent` only.
- **Post-restart:** call `take_update_notice` once at startup (not an event listener, see U2 deviation 1) and toast "Updated to vX" for `installed`, a "did not complete" message for `incomplete`.
- **Tests (1–3):** state→DOM mapping, no HTML injection from version/reason strings, manual-only branch hides Install.
- **Docs wording rule:** claim only what U5 confirms (esp. macOS Gatekeeper and admin prompts). While D15 holds, the docs say macOS updates are manual (download from the releases page); do not describe an in-app macOS update. Mark first updater version as "install by hand once" (R11).
- **As built:**
  - New `src/update-ui.ts` (state machine plus pure `stateFromCheck`/`viewFor`/`applyView`, `bindUpdateUi`); `src/ipc/update.ts` now wraps all four commands plus the `update-progress` listener; `toolbar.ts` handler replaced by one `bindUpdateUi` call; Updates row in `index.html` (`role="status"`, `aria-live="polite"`, `<progress>`, Cancel / Install and restart / Check buttons); CSS in `overlays.css` (`.btn-sm-primary`, progress bar). Row is hidden outside Tauri.
  - **Addition beyond plan: unsaved-changes confirm.** Before the download, if `wfManager.hasUnsaved`, a danger confirm is shown (autosave is 30 s, so edits can be lost on restart). Declining returns to the "available" state.
  - Active-run flow: first `installUpdate(false)`; on `active_runs` a confirm names the counts, then `installUpdate(true)`. The verified download is reused (U2 staging), so no second download.
  - Post-restart: `takeUpdateNotice()` at bind time; `installed` → success toast; `incomplete` → error toast plus persistent error state with a releases link.
  - Links: every href passes a `https://github.com/Panchak2d/aerini/` prefix check, else falls back to the releases page; all strings via `textContent`.
  - **D15 consequence:** the UI needs no macOS special case. Until `darwin-aarch64-app` is in the manifest, U2 returns `manual / no_release_for_platform` on macOS and the UI shows "Download from GitHub". Docs say macOS updates are manual.
  - Tests: `src/__tests__/update-ui.test.ts`, 3 tests (state→DOM mapping, no markup/URL injection, manual-only hides Install).
  - Docs updated: `updating.md` (desktop section rewritten), `security.md` (section renamed "Updates and the network", anchor `#updates-and-the-network`; every link to the old anchor updated; the "exactly one automatic outbound connection" sentence kept), `faq.md`, `installation.md`, `known-issues.md` (macOS Keychain re-prompt after replacing the app), `desktop-ipc-reference.md` (callers, `is_newer` removed, `update-progress` noted), `CHANGELOG.md` (Added + Changed).
  - **Correction made during U3 review:** first draft of the docs described macOS in-app updates, contradicting D15 and the wording rule below. Fixed before delivery of the final patch; macOS is documented as manual everywhere.
  - **Wording still unconfirmed (U5):** admin-password prompts per installer type (from code flags `admin_prompt`, plus Tauri docs), Keychain re-prompt (external reports only), "Windows `.exe` and AppImage don't normally need a password". Recheck these against U5 results before launch.
  - **Known limit:** edits made after the last autosave and during a long download can still be lost; the confirm at click time reduces but cannot remove this.
  - **Open for U4:** `dist/` is committed; owner rebuilds and recommits it after this patch.

### U4 — CI/release pipeline and maintainer docs — DELIVERED, not run
- **Files:** `.github/workflows/release.yml`, `.github/workflows/ci.yml` (run script tests), `scripts/verify-release-tag.sh` (or new sibling), new `scripts/build-updater-manifest.py` + `scripts/tests/` fixtures, `CONTRIBUTING.md` (release + signing + rotation section), `docs/operations/` only if a maintainer page already exists.
- **`bundle.createUpdaterArtifacts: true` lands here (D16)**, together with the signing secrets. CI `tauri-build` (`npm run build`, no key, nothing published) must then skip signing, e.g. `npm run build -- --no-sign`, or override the flag with `--config`; pick one and note it in `CONTRIBUTING.md` so local `npx tauri build` is not a surprise.
- **macOS (D15):** the expected-key constant omits `darwin-aarch64-app`; the `.app.tar.gz` and its `.sig` are still built and uploaded. The `manifest` job must not fail on that omission, and must not list a darwin entry in `latest.json`.
- **`release.yml`:**
  - matrix `release` job: add `environment: release`; pass `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; `uploadUpdaterJson: false`; keep `uploadUpdaterSignatures` default; keep `releaseDraft: true`.
  - new `manifest` job (`needs: release`, `contents: write`, **no signing secret**): lists the draft release's assets by tag, downloads `*.sig`, runs `build-updater-manifest.py`, uploads `latest.json` with `--clobber`, writes the key list to `$GITHUB_STEP_SUMMARY`. Idempotent on re-run.
  - `verify` job additions (R6): `bundle.createUpdaterArtifacts == true` (present from U4 on), `plugins.updater.pubkey` non-empty, endpoint contains `github.com/Panchak2d/aerini`.
- **Manifest script contract:**
  - Input: asset list (name + local `.sig` path), tag. Output `latest.json`: `{version (no "v"), notes (fixed short string), pub_date (RFC 3339 UTC), platforms}`.
  - Mapping by asset suffix: `.app.tar.gz` → `darwin-{arch}-app`; `.AppImage` → `linux-{arch}-appimage`; `.deb` → `-deb`; `.rpm` → `-rpm`; `.exe` → `windows-{arch}-nsis`; `.msi` → `windows-{arch}-msi`. Normalize arch tokens (`amd64`/`x86_64`, `arm64`/`aarch64`).
  - `signature` = full `.sig` file text. `url` = `https://github.com/Panchak2d/aerini/releases/download/{tag}/{asset name as GitHub stores it}`.
  - **Expected-key list lives in one constant.** Exit non-zero if any key is missing, any `.sig` is empty, or a signature's trusted comment doesn't carry the release version (inspect a real `.sig` first and write the parser to its actual format; if it isn't parseable, rely on U5's runtime check instead and say so).
  - Fixtures: a complete asset set (passes), one missing key (fails), empty sig (fails).
- **First step of this batch:** run a release on the test fork (§6) and record the **actual asset names** per platform, including whether `.rpm` and arm64 AppImage exist. Encode the expected keys from that, not from assumptions.
- **Human prerequisites:** create GitHub environment `release` with a required reviewer and the two secrets; add a tag-protection ruleset for `v*`; keep the key backup in two places.
- **Docs (maintainer):** release steps now include: verify the manifest summary, publish the draft (publishing is what releases the update to users), rotation procedure, "never change `pubkey` without the bridging procedure".

- **As built (not run on GitHub):**
  - `tauri.conf.json`: `bundle.createUpdaterArtifacts: true` (D16). `package.json`: new `build:unsigned` (`tauri build --no-sign`; flag VERIFIED in tauri-cli 2.12.1 `build.rs`/`bundle.rs`, where it skips updater signing). CI `tauri-build` uses it; `npm run build` stays the signing form. The release job uses neither script (tauri-action runs the CLI itself), so a missing key still fails the release build.
  - `release.yml`: `verify` also runs `scripts/verify-updater-config.py` (flag on, pubkey is a well-formed minisign Ed25519 key, first endpoint equals `https://github.com/{github.repository}/releases/latest/download/latest.json` case-insensitively, `requireSignedVersion` true). `release` matrix job: `environment: release`, signing secrets only on the tauri-action step, `uploadUpdaterJson: false`. New `manifest` job (`needs: [verify, release]`, no signing secret).
  - **Deviation 1, repo from `github.repository`, not a hard-coded slug:** manifest URLs and the endpoint check follow the repository the workflow runs in. Same result in production, and the U5 fork needs no script edit. Consequence: the fork must commit its own pubkey and endpoint (the `--config` override named in U5 has no hook in the workflow).
  - **Deviation 2, release looked up by id, not `gh release ... <tag>`:** the by-tag endpoint does not return drafts, so the job lists releases, requires exactly one with the tag (a duplicate draft from the matrix race fails loudly), downloads each `.sig` through the asset API, and uploads `latest.json` to `uploads.github.com` with curl after deleting any old copy. Re-running is idempotent.
  - `scripts/build-updater-manifest.py` (stdlib only): maps asset suffix + arch token to `{os}-{arch}-{bundle}`. `EXPECTED_KEYS` (8: linux x86_64/aarch64 appimage/deb/rpm, windows x86_64 nsis/msi) must all be present. `HELD_BACK_KEYS` (`darwin-aarch64-app`) may be present and is never listed (D15). Two assets for one key, a missing/empty/unreadable `.sig`, a signature with no `version:` field, or one whose version differs from the tag all fail. Any other mapped key is skipped with a warning in the summary. `signature` is the trimmed `.sig` text; `url` is `https://github.com/{repo}/releases/download/{tag}/{asset}`.
  - Trusted comment format VERIFIED against tauri-cli 2.12.1 `updater_signature.rs` and plugin 2.13.1 `updater.rs`: `timestamp:N<TAB>file:NAME<TAB>version:V`. `file:` is deliberately not checked (the action renames macOS archives).
  - Tests: `scripts/tests/` (19 tests, run in the sandbox: pass), new CI job `script-tests`.
  - Docs: `CONTRIBUTING.md` (build commands, script tests, new "Releasing (maintainers)" section: setup, release steps, rotation, rules), `docs/development/testing.md`, `README.md`. No `CHANGELOG.md` entry: the repo logs user-visible changes only.
  - Also fixed: removed the byte-identical stray `dockerignore` at the repo root; `MANIFEST.txt` added to `.gitignore` (patch manifests were being left at the root).
- **Asset names are from source, not from a real run (HIGH, not VERIFIED):** the plan's first step (run a release on the test fork) could not be done here. Names come from tauri-bundler 2.10.1 (`Aerini_V_amd64.deb`, `Aerini_V_arm64.deb`, `Aerini-V-1.x86_64.rpm`, `Aerini-V-1.aarch64.rpm`, `Aerini_V_amd64.AppImage`, `Aerini_V_aarch64.AppImage`, `Aerini_V_x64-setup.exe`, `Aerini_V_x64_en-US.msi`) and tauri-action 1.0.0 (keeps the CLI names; `.sig` = name + `.sig`; `.app.tar.gz` gets version and arch). If the first real run reports a missing key, fix `EXPECTED_KEYS`, not the workflow. Check specifically that `.rpm` and the arm64 AppImage exist.
- **Unverified in practice, check in U5:** `gh api` octet-stream download of draft assets; the `uploads.github.com` upload with `GITHUB_TOKEN`; one approval of the `release` environment covering all four matrix jobs; the signed `.sig` really carrying `version:`.
- **Known limit:** the draft can still be published after a failed `manifest` job. With no `latest.json` on the new release, checks fail with "No update information is published yet" until it is fixed; the previous release's manifest is not used.

### U5 — Real-machine verification (human-run, implementer prepares)
- Not a code batch. The implementer produces the test fork setup and checklist results template; the owner runs it. **No production tag until every row passes.**
- **Setup:** private test repo mirroring the workflow, **separate test keypair** and test endpoint via a build-time `--config` override; build version X and X+1; publish X+1 as a non-prerelease release.
- **Matrix** (each OS/installer type × scenario):

| Scenario | mac arm64 | Win NSIS | Win MSI | Linux AppImage | Linux deb | Linux rpm |
|---|:-:|:-:|:-:|:-:|:-:|:-:|
| Happy path: check → install → restart on X+1 | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Data intact: workflows, credentials, settings | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Update while a workflow runs (confirm shown, stopped cleanly) | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Update during a Code (JS) node run | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Launched via autostart `--minimized` → window shown after update | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Always-on webhook workflow works after restart (ports free) | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Offline at check / network killed mid-download → clear error, manual link | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Tampered artifact / wrong key → rejected, nothing installed | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Manifest announces higher version with older artifact → rejected (`requireSignedVersion`) | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Same version / no update → "up to date" | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Draft or prerelease not offered | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Double-click Install / click during download → single flight | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Cancel during download → no partial state | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ |
| Unsupported location (DMG/translocated, read-only AppImage, no polkit) → manual path | ☐ | n/a | n/a | ☐ | ☐ | ☐ |
| macOS Keychain: prompt after update; Allow works; **Deny keeps credentials intact (U0)** | ☐ | n/a | n/a | n/a | n/a | n/a |
| macOS Gatekeeper does not re-fire after in-app update | ☐ | n/a | n/a | n/a | n/a | n/a |
| Windows: UAC behavior, SmartScreen, restart-after-install | n/a | ☐ | ☐ | n/a | n/a | n/a |
| Rotation dry run: bridging release then new key | ☐ (one OS is enough) |  |  |  |  |  |

- macOS rows are not runnable until a Mac owner is available (D15); until then they stay unticked and macOS ships on the manual-download path. Passing them is the precondition for adding `darwin-aarch64-app` to the expected-key list.
- Record results in the PR/batch notes; any failure becomes its own fix batch before launch.

---

## 6. Launch gate additions (extends `AGENT_PROTOCOL.md` LAUNCH GATE)

1. U0–U4 merged; real `cargo build/test --workspace`, `npm run build`, `npm test` pass on the owner's machine.
2. U5 matrix fully green on the test fork.
3. First production release with the updater is tagged, **draft reviewed** (manifest summary shows every expected key), then published.
4. Smoke: install the *previous* build, update to the new one through the UI, confirm.
5. Release notes for that first updater version say: install by hand this once; future updates are in-app.

---

## 7. Docs impact checklist (Rule 30)

`docs/operations/updating.md` · `docs/guide/security.md` · `docs/faq.md` · `docs/getting-started/installation.md` · `docs/development/desktop-ipc-reference.md` · `docs/known-issues.md` · `docs/guide/credentials.md` (U0) · `CONTRIBUTING.md` · `CHANGELOG.md`. Grep `README.md`, `SECURITY.md`, `docs/development/architecture.md`, `docs/index.md` for "update", "auto-update", "releases" and fix anything the change contradicts. The "exactly one automatic outbound connection" sentence stays true under D1; do not weaken it.

---

## 8. Open items needing a human (cannot be done by the implementer)

1. Generate the signing keypair with a password; give the implementer only the public key.
2. Create the `release` environment, required reviewer, secrets, and `v*` tag ruleset.
3. Create the private test fork and a test keypair.
4. Run U5 on real macOS (arm64), Windows, and Linux machines.
5. Back up the private key and password in two separate places.

---

## 9. Backlog (not in this series; logged per Rule 6)

| ID | Item | Note |
|---|---|---|
| B-U1 | Opt-in "Check on startup" setting, default off, one request per launch | Needs a privacy-doc update; no migration required. |
| B-U2 | Isolate signing from the build (sign in a separate job so build scripts/npm deps never see the key) | Stronger than D11; costs hand-rolled `.app.tar.gz` and signing steps. |
| B-U3 | Intel Mac build (`universal-apple-darwin` or an x86_64 job) | Adds `darwin-x86_64-app` to the expected-key list. |
| B-U4 | Apple Developer ID signing + notarization | The only real fix for repeated Keychain prompts and Gatekeeper warnings. |
| B-U5 | Windows code signing | Removes SmartScreen friction. |
| B-U6 | Prerelease/beta channel with its own endpoint | Needs a second manifest path. |
| B-U7 | Move the endpoint behind a project-owned URL | Removes GitHub lock-in; requires a bridging release. |

---

## 10. Definition of done (series)

- A user on version N clicks Install and lands on N+1 with data intact on every shipped OS/installer type.
- A tampered, downgraded, or wrong-key update is rejected and nothing is installed.
- No workflow is killed without an explicit confirmation.
- No webview-reachable plugin install command; no remote-controlled string reaches `innerHTML`.
- `latest.json` is generated by one job, with stable URLs, and the build fails if any expected platform is missing.
- Docs match behavior; `security.md` still says exactly what is automatic and what is not.
