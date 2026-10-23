# Install the MCP Program

This page gets the `aerini-mcp` program onto your computer: pick the right file from a long release list, check that the download is intact, unpack it, and clear the warnings macOS and Windows show for unsigned programs. It does not configure an AI client; for that, see the [MCP Quickstart](mcp-quickstart.md) or [Client configuration](mcp.md#client-configuration).

> [!TIP]
> **Short version.** If you only use the desktop app, you want the archive whose name starts with `aerini-mcp-offline_` for your platform, plus the `.sha256` file with the same name. If you run `aerini-server api`, you want the one that starts with `aerini-mcp_`. Not sure which? See [Choosing a mode](mcp.md#choosing-a-mode).

Every release publishes both programs for each platform on the [Releases page](https://github.com/Panchak2d/aerini/releases), as an archive that holds the program and the license text, plus a `.sha256` file beside it.

| Platform | Online | Offline |
|---|---|---|
| Linux x86_64 | `aerini-mcp_<version>_x86_64-unknown-linux-gnu.tar.gz` | `aerini-mcp-offline_<version>_x86_64-unknown-linux-gnu.tar.gz` |
| Linux arm64 | `aerini-mcp_<version>_aarch64-unknown-linux-gnu.tar.gz` | `aerini-mcp-offline_<version>_aarch64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `aerini-mcp_<version>_aarch64-apple-darwin.tar.gz` | `aerini-mcp-offline_<version>_aarch64-apple-darwin.tar.gz` |
| Windows x64 | `aerini-mcp_<version>_x86_64-pc-windows-msvc.zip` | `aerini-mcp-offline_<version>_x86_64-pc-windows-msvc.zip` |

Those four platforms are all that is published. There is no prebuilt program for Intel Macs, Windows on Arm, or Linux systems that do not use glibc (such as Alpine); [build it from source](#build-it-from-source) there.

### Step 1: Find the right files in the release list

A release lists dozens of files, because the desktop app's installers are attached to the same page. Only some are for `aerini-mcp`. The release notes have an **MCP server** table with a link for each platform and mode, plus the matching `.sha256`, which saves the search. In the **Assets** list, use the name to tell them apart:

| File name starts with | What it is | Do you need it? |
|---|---|---|
| `aerini-mcp_` | The MCP program that talks to a server (online mode) | If you run a server |
| `aerini-mcp-offline_` | The MCP program with the engine built in (offline mode) | If you have no server |
| `Aerini_` or `Aerini-` (capital A) | The desktop app: installers, packages, AppImage, app bundles | No, that is the app itself |
| anything ending in `.sig` | A signature file used by the desktop app's updater | No |
| anything ending in `.sha256` | A checksum for the archive with the same name | Yes, download the one that matches your archive |

The name of an MCP archive has four parts. For example:

```text
aerini-mcp-offline_0.4.3_x86_64-pc-windows-msvc.zip
|__________________| |____| |___________________| |__|
   which program    version    platform (target)   format
```

- **Program:** `aerini-mcp` (online) or `aerini-mcp-offline` (offline).
- **Version:** match it to the version of your Aerini server, if you use one.
- **Platform:** the CPU and operating system, as described in the next step.
- **Format:** `.tar.gz` for Linux and macOS, `.zip` for Windows.

### Step 2: Match the platform to your computer

| Name contains | Means | You have it if... |
|---|---|---|
| `x86_64-unknown-linux-gnu` | Linux, Intel/AMD 64-bit | `uname -m` prints `x86_64` |
| `aarch64-unknown-linux-gnu` | Linux, Arm 64-bit | `uname -m` prints `aarch64` or `arm64` |
| `aarch64-apple-darwin` | macOS, Apple Silicon (M1 and later) | `uname -m` prints `arm64` on a Mac |
| `x86_64-pc-windows-msvc` | Windows, Intel/AMD 64-bit | PowerShell `$env:PROCESSOR_ARCHITECTURE` prints `AMD64` |

On a Mac, `uname -m` printing `x86_64` means an Intel Mac, which has no prebuilt program. On Windows, `ARM64` means Windows on Arm, which also has none; whether the x64 program runs there under Windows's own emulation has not been tested.

Offline or online? Use the [table above](mcp.md#choosing-a-mode). The offline program contains the whole workflow engine, so its download is several times larger than the online one (on the `0.4.3` release page, roughly 10 to 16 MB against 3 to 4 MB per archive). That is why it is separate: an agent that only talks to a server does not need it. Both can sit in one folder, because the offline archive holds a program named `aerini-mcp-offline`.

### Step 3: Download the archive and its checksum

Download the archive and the `.sha256` file with the same name plus `.sha256` at the end. Keep them in the same folder. The examples below use version `0.4.3`. Replace it, and the platform, with yours.

### Step 4: Verify the download

The `.sha256` file contains one line, in the format that `sha256sum -c` reads: a hash, two spaces, and the archive's file name.

Linux:

```bash
sha256sum -c aerini-mcp-offline_0.4.3_x86_64-unknown-linux-gnu.tar.gz.sha256
```

macOS (use `shasum -a 256 -c` instead of `sha256sum -c`):

```bash
shasum -a 256 -c aerini-mcp-offline_0.4.3_aarch64-apple-darwin.tar.gz.sha256
```

Both print `<file name>: OK` on a match. Anything else, including `FAILED`, means do not use the file: download it again.

Windows has no `sha256sum`. In PowerShell, print the hash of your download and the line in the checksum file, and compare the two by eye. `Get-FileHash` prints upper-case letters, so convert it to lower case first:

```powershell
(Get-FileHash .\aerini-mcp-offline_0.4.3_x86_64-pc-windows-msvc.zip -Algorithm SHA256).Hash.ToLower()
Get-Content .\aerini-mcp-offline_0.4.3_x86_64-pc-windows-msvc.zip.sha256
```

The first line printed must equal the long hex string at the start of the second.

A checksum that sits next to the archive in the same release proves the file was not damaged or cut short in transit. It does not prove the release itself was not tampered with, because anyone who could change the archive could change the checksum too.

### Step 5: Unpack it into its own folder

An archive holds exactly two files, the program and `LICENSE`, with no folder around them. Unpack into a new folder so they do not land among your other files.

Linux and macOS:

```bash
mkdir -p ~/aerini-mcp
tar -xzf aerini-mcp-offline_0.4.3_x86_64-unknown-linux-gnu.tar.gz -C ~/aerini-mcp
```

The program is already marked executable. If your system reports "permission denied", run `chmod +x ~/aerini-mcp/aerini-mcp-offline`.

Windows (PowerShell):

```powershell
Expand-Archive -Path .\aerini-mcp-offline_0.4.3_x86_64-pc-windows-msvc.zip -DestinationPath $HOME\aerini-mcp
```

The program is `aerini-mcp-offline.exe` there.

Write down the **full path** of the program. Clients do not search your `PATH` for it, and Claude Desktop needs an absolute path. On Linux and macOS, `realpath ~/aerini-mcp/aerini-mcp-offline` prints it. On Windows, `(Resolve-Path $HOME\aerini-mcp\aerini-mcp-offline.exe).Path` does.

### Step 6: Clear the macOS and Windows warnings

The builds are not code-signed, like the desktop installers.

- **macOS:** a program downloaded through a browser carries a quarantine flag, and Gatekeeper refuses to start it from an MCP client. Clear the flag once, on the unpacked program:

  ```bash
  xattr -d com.apple.quarantine ~/aerini-mcp/aerini-mcp-offline
  ```

  If the command reports that the attribute does not exist, the flag was not set and you can go on.
- **Windows:** SmartScreen may warn the first time. Click **More info**, then **Run anyway**. As with the desktop installer, this is expected for a program without a paid code-signing certificate. If your client starts the program without showing you a prompt and the tools never appear, run the program once from PowerShell so the warning has a window to appear in.

### Step 7: Check that it starts

```bash
~/aerini-mcp/aerini-mcp-offline --version
```

This prints the program's name and version. `--help` lists every flag, with the same defaults as in [Configuration](mcp.md#configuration). If both work, the download is good and your system can run it.

To see how it behaves when a client starts it, run it with no arguments. The offline program prints nothing and waits, because it is listening for a client on standard input. Press Ctrl+C to stop it. The online program with no `AERINI_URL` instead exits at once with an error message and exit code `2`. That is not a fault in the file; see the [offline build note](mcp.md#choosing-a-mode).

### Build it from source

You only need this for a platform with no prebuilt program, or if you want to audit what you run. It needs a Rust toolchain. From the repository root:

```bash
cargo build --release -p aerini-mcp
cargo build --release -p aerini-mcp --features offline
```

Each command leaves `target/release/aerini-mcp`; the second one overwrites the first, so copy the first result somewhere before you run the second. The offline build compiles the engine and its dependencies and takes much longer. A source build of the offline program is called `aerini-mcp`, like the online one, not `aerini-mcp-offline`; rename the copy if you keep both.

### Keep the program and the server on the same release

The server's REST surface only grows, so an older `aerini-mcp` works against a newer server, but a newer `aerini-mcp` against an older server fails on the routes the server does not have; see [Troubleshooting](mcp.md#troubleshooting).
