# Installation

Aerini installs like any other desktop app: download a file, open it, and you're running. No terminal, no compiler, no account to create. Building from source is possible too, but it's a separate path for contributors, covered lower on this page.

## Download the installer

Every release is published on the [Releases page](https://github.com/Panchak2d/aerini/releases). Pick the file that matches your operating system.

### macOS

1. Download the `.dmg` file from Releases.
2. Open it and drag Aerini into your Applications folder.
3. Launch Aerini from Applications (or Spotlight).

Aerini's macOS build isn't signed with an Apple Developer certificate, so the first launch triggers Gatekeeper. Instead of double-clicking, right-click (or Control-click) the Aerini icon and choose **Open**, then choose **Open** again in the dialog that follows. You only need to do this once; after that it opens normally.

> [!NOTE]
> The current macOS release is built for Apple Silicon (M1 and later). An Intel Mac build isn't published yet. If you're on an Intel Mac, building from source (below) is the only option for now.

### Windows

1. Download either the `.msi` or the `.exe` from Releases. Both install the same app; pick whichever you're used to running.
2. Run the installer.

The installer isn't code-signed, so Windows Defender SmartScreen will probably step in with a warning the first time you run it. Click **More info**, then **Run anyway** to continue. This is normal for a small open-source project without a paid code-signing certificate, not a sign that anything is wrong.

### Linux

1. Download the `.deb` (Debian, Ubuntu, and derivatives) or the `.AppImage` (most other distributions) from Releases.
2. For `.deb`: double-click it to open your distribution's graphical package installer, or install it from a terminal with `sudo apt install ./<the-file-you-downloaded>.deb`.
3. For `.AppImage`: make it executable (`chmod +x Aerini*.AppImage`) and run it directly. No installation step needed.

Aerini's window is rendered with `webkit2gtk` (version 4.1). Most desktop Linux installations already have it, since plenty of other apps depend on it too. If Aerini fails to launch with an error mentioning a missing shared library, install your distribution's `webkit2gtk-4.1` runtime package (for example, `libwebkit2gtk-4.1-0` on Debian and Ubuntu) and try again.

## Building from source

This section is for contributors, or for a platform the installer doesn't cover yet (like Intel Macs today). Most people should use the installer above instead.

You'll need:

| Tool | Version |
|---|---|
| Rust (stable) | 1.96+ |
| Node.js | 20.19+, 22.13+, or 24+ |
| Tauri CLI | 2.x |

```bash
git clone https://github.com/Panchak2d/aerini
cd aerini
npm install
./scripts/fetch-node-binaries.sh
npm run dev
```

`fetch-node-binaries.sh` downloads and verifies the Node.js runtime that Aerini bundles for its Code (JS) node. It's a one-time step per clone, and the build fails without it. The first build takes a few minutes while Rust compiles; later changes rebuild in seconds. Full details, including how to produce a standalone installer yourself, live in `CONTRIBUTING.md` at the repository root.

## Uninstalling

Removing the app itself is the usual process for your OS: drag it to the Trash on macOS, use *Add or Remove Programs* on Windows, or remove the package with your package manager on Linux (for example, `sudo apt remove aerini`), or just delete the AppImage file if that's how you installed it.

Uninstalling the app doesn't delete your workflows, credentials, or settings. Those live in a separate data folder:

| OS | Location |
|---|---|
| macOS | `~/Library/Application Support/org.aerini.desktop` |
| Windows | `%APPDATA%\org.aerini.desktop` |
| Linux | `~/.local/share/org.aerini.desktop` |

Delete that folder too if you want a completely clean removal. One thing it won't contain: the encryption key for your stored credentials sits in your OS [keyring](../glossary.md#keyring) (macOS Keychain, Windows Credential Manager, or Linux Secret Service) rather than in that folder, so a thorough wipe means clearing it from there as well if your keyring manager makes that easy to do.

If you turned on **Launch at login** in Settings, it's worth switching that off before you uninstall, so nothing tries to start Aerini after it's gone.

## What's next

Head to [Getting Started](getting-started.md) to build and run your first workflow.
