# Updating

This page covers three separate things: updating the desktop app, updating a self-hosted `aerini-server` deployment, and what happens when a workflow file's schema version doesn't match the app or server opening it. For why the desktop update only happens when you click, and how it's verified, see [Security §Updates and the network](../guide/security.md#updates-and-the-network).

## Desktop app

Open **Settings**, find **Updates** under **About**, and click **Check for Updates**. Aerini never checks on its own: there's no startup check, no background poller, and no scheduled interval. It contacts GitHub for an update only when you click the button.

The check reads a small manifest from the latest published release on GitHub and compares it to your installed version. One of three things happens:

- **You're up to date.** The status line shows your current version.
- **A newer version exists.** The status line shows the new version next to a **View release notes** link and an **Install and restart** button.
- **A newer version exists, but Aerini can't install it itself.** The status line says why and offers **Download from GitHub** instead. See [When Aerini can't update itself](#when-aerini-cant-update-itself).

Click **Install and restart** to update. Aerini downloads the installer for your platform from the release, shows a progress bar (with **Cancel** while it downloads), verifies its signature, installs it, and restarts. Your workflows, credentials, and settings aren't touched; they live in a separate per-OS data folder that the app bundle never overwrites (see [Installation §Uninstalling](../getting-started/installation.md#uninstalling) for exactly where). After the restart, a short message confirms the version you're now on.

Aerini only installs a package whose signature checks out against a public key built into the app. A download that fails verification is discarded and nothing is installed. How this is protected is covered in [Security §Updates and the network](../guide/security.md#updates-and-the-network).

### Before it installs

- **Unsaved changes.** Installing restarts Aerini. If you have changes that may not be saved yet, Aerini asks before it starts.
- **Running workflows.** If any workflow is running, including background and scheduled runs, Aerini tells you how many and asks before going further. Confirming stops them and then installs; declining leaves everything running and the update waiting.
- **Administrator password.** Installing the `.msi` on Windows, or a `.deb` or `.rpm` package on Linux, can make your system ask for an administrator password. The status line tells you when that's expected. The Windows `.exe` installer and the macOS and AppImage builds don't normally need one.

### When Aerini can't update itself

In some setups the in-app install isn't possible. Aerini then shows the new version with a **Download from GitHub** link, and you update by hand as described in [Installation](../getting-started/installation.md#download-the-installer). The usual causes:

| Situation | What to do |
|---|---|
| macOS: running from the disk image or from a temporary location | Move Aerini into Applications, then reopen it |
| Linux AppImage: the folder holding the file is read-only, or the AppImage was started in a way that hides its location | Move it to a writable folder, or install the new one by hand |
| An installer type Aerini doesn't recognize, such as a build you compiled yourself | Install the new version by hand |
| The release has no package for your platform yet | Install by hand |

If a check or an install fails (no connection, a stalled download, a failed signature check), the status line says what happened and keeps the **Download from GitHub** link as a fallback. Checking again starts clean.

> [!NOTE]
> **Aerini 0.4.1 and earlier can't update themselves.** Install the first release that includes in-app updates by hand, as described in [Installation](../getting-started/installation.md#download-the-installer). From then on, updates install from Settings.

> [!NOTE]
> Releases are published as GitHub drafts first, and a maintainer publishes them manually. Only published releases are offered, so a version that was just tagged may not show up in Check for Updates until it's published. Prereleases, such as release candidates, are never offered. A prerelease build has to be installed by hand.

> [!NOTE]
> On macOS, Aerini isn't signed with an Apple Developer certificate, so macOS can ask for Keychain access again after an update. Choose **Always Allow**; see [Known Issues](../known-issues.md#macos-asks-for-keychain-access-again-after-an-update).

## Server (`aerini-server`)

A prebuilt `aerini-server` container image is published to GHCR with every release (see [Server Deployment §Getting the `aerini-server` binary](server-deploy.md#getting-the-aerini-server-binary)), so the official Docker image now updates by pulling a new tag rather than rebuilding. There's still no standalone, downloadable binary for the other two deployment shapes below — the [Releases page](https://github.com/Panchak2d/aerini/releases) only carries desktop installers — so bare metal and the desktop's Docker export still update by rebuilding from source.

### Official Docker image

If you're running the published image (`docker pull ghcr.io/panchak2d/aerini-server:<version>`, see [Server Deployment §Getting the `aerini-server` binary](server-deploy.md#getting-the-aerini-server-binary)), update by pulling the new tag and recreating the container:

```bash
docker pull ghcr.io/panchak2d/aerini-server:<newer-version>
docker compose up -d
```

(or `docker stop`/`docker rm` and `docker run` again with the new tag, if you started it with `docker run` instead of Compose).

If you built the image locally instead — `git clone` plus `docker compose up --build`, for customizing the `Dockerfile` — update the same way as before, by pulling the source and rebuilding:

```bash
git pull
docker compose up -d --build
```

This image's build stage copies your source into the image with `COPY`, so a `git pull` that changes any tracked file invalidates the layers built from it, and the rebuild picks up the new code correctly. No `--no-cache` needed here. Either way, the named data volume isn't touched by an update, so your workflows and run history persist across it.

### Bare metal (Linux, built from source)

```bash
git pull
sudo apt install -y libdbus-1-dev   # already present if you built it before
cargo build --release -p aerini-server
```

Then replace the running binary and restart:

- **`serve` mode installed via `install.sh`:** copy the new binary over `~/.aerini-server/<workflow>/aerini-server`, then restart with `systemctl --user restart aerini-<workflow>`, the service name `install.sh` created for you.
- **`api` mode, or anything started by hand:** stop the running process, replace the binary at whatever path you launch it from, start it again. There's no generated service file for `api` mode; whatever you used to keep it running (a systemd unit you wrote yourself, a process supervisor) is what you restart.

### The desktop's "Export for Server → Docker" package

The package generated by **Export → Export for Server → Docker** builds differently from the official image above: it clones the Aerini repository inside the Docker build itself (`git clone --branch $AERINI_REF`, defaulting to `main`), rather than building from source already on disk. That distinction matters for updates specifically. Docker caches that clone step by the Dockerfile instruction's own text, not by what the remote branch currently holds, so re-running `docker compose build` alone can silently reuse the old cached layer and rebuild the same old code even though `main` has moved on. Force a fresh clone with:

```bash
docker compose build --no-cache
docker compose up -d
```

or bump `--build-arg AERINI_REF=v<newer-version>` to a specific tag each time, which also busts the cache since the build argument itself changed. Either way restart afterward; the mounted `aerini-server.json` and the data volume are unaffected.

## Opening older or newer workflow files

Every `.aerini` file, and every workflow saved in Aerini's own database, carries a `schema_version` field. Whichever app or server loads that workflow checks the field before anything else runs:

| Situation | What happens |
|---|---|
| File's version matches what this build understands | Loads normally, nothing extra happens |
| File's version is older, and this build has a migration path for it | Upgrades the file's JSON automatically on load, no action needed from you |
| File's version is older, but no migration path reaches this build's version | Load fails with an error naming both versions |
| File's version is newer than what this build understands | Load fails; the error tells you to upgrade Aerini |
| File has no `schema_version` field at all (saved before the field existed) | Treated as the oldest known version and loads normally |

As of Aerini 0.4.0 the workflow file format hasn't changed since it was introduced: `schema_version` is `"1.0"` everywhere, and no migrations are registered yet. Every file loads as a same-version no-op today. The table above describes the mechanism that will run the moment that changes, not a backlog of migrations already waiting to fire. A full version history, once the format has one worth documenting, will live in `schema-migrations.md`.

There's no downgrade path either way: opening a workflow saved by a newer Aerini in an older one always fails with a message telling you to upgrade, never a partial or best-effort load.

## What's next

- [Server Deployment](server-deploy.md), for getting `aerini-server` running the first time
- [Security §Updates and the network](../guide/security.md#updates-and-the-network), for why updating is click-only and how a downloaded update is verified
- [Installation](../getting-started/installation.md), for the platform-specific install steps an update reuses
