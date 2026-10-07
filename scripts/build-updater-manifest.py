#!/usr/bin/env python3
"""Build latest.json for the Tauri updater from a release's assets and signatures.

Usage:
  build-updater-manifest.py --tag vX.Y.Z --repo OWNER/NAME --assets assets.txt \
      --sig-dir sigs/ --out latest.json [--summary summary.md] [--pub-date RFC3339]

assets.txt holds one release asset name per line, exactly as GitHub stores it.
sig-dir holds one "<asset name>.sig" file per installer that has a signature.

Exits non-zero, writing nothing, if any expected platform is missing, any signature
is empty or unparseable, or a signature was made for a different version than the tag.
"""

import argparse
import base64
import binascii
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import quote

# Platforms every release must provide. Keys are "{os}-{arch}-{bundle}", the form each
# installed binary looks itself up by.
EXPECTED_KEYS = frozenset(
    {
        "linux-x86_64-appimage",
        "linux-x86_64-deb",
        "linux-x86_64-rpm",
        "linux-aarch64-appimage",
        "linux-aarch64-deb",
        "linux-aarch64-rpm",
        "windows-x86_64-nsis",
        "windows-x86_64-msi",
    }
)

# Built and signed on every release but deliberately left out of latest.json, so those
# installs take the manual-download path. Move a key into EXPECTED_KEYS to enable it.
HELD_BACK_KEYS = frozenset({"darwin-aarch64-app"})

NOTES = "See the release notes on GitHub for what changed in this version."

SUFFIX_TO_BUNDLE = (
    (".app.tar.gz", "darwin", "app"),
    (".AppImage", "linux", "appimage"),
    (".deb", "linux", "deb"),
    (".rpm", "linux", "rpm"),
    (".exe", "windows", "nsis"),
    (".msi", "windows", "msi"),
)

ARCH_ALIASES = {
    "x86_64": "x86_64",
    "amd64": "x86_64",
    "x64": "x86_64",
    "aarch64": "aarch64",
    "arm64": "aarch64",
}

ARCH_RE = re.compile(r"(?<![A-Za-z0-9])(x86_64|amd64|x64|aarch64|arm64)(?![A-Za-z0-9])")
TAG_RE = re.compile(r"^v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)$")
REPO_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")


class ManifestError(Exception):
    pass


def classify(name):
    """Return (os, arch, bundle) for an installer asset name, or None if it isn't one."""
    for suffix, os_name, bundle in SUFFIX_TO_BUNDLE:
        if name.endswith(suffix):
            stem = name[: -len(suffix)]
            break
    else:
        return None
    matches = ARCH_RE.findall(stem)
    if not matches:
        raise ManifestError(f"cannot find an architecture in asset name '{name}'")
    return os_name, ARCH_ALIASES[matches[-1]], bundle


def signed_version(sig_text, asset):
    """Read the version recorded in a signature's trusted comment, or None if absent."""
    try:
        decoded = base64.b64decode(sig_text, validate=True).decode("utf-8")
    except (binascii.Error, UnicodeDecodeError) as err:
        raise ManifestError(f"signature for '{asset}' is not valid base64 text: {err}")
    comments = [
        line[len("trusted comment: "):]
        for line in decoded.splitlines()
        if line.startswith("trusted comment: ")
    ]
    if len(comments) != 1:
        raise ManifestError(f"signature for '{asset}' has no single trusted comment")
    for field in comments[0].split("\t"):
        if field.startswith("version:"):
            return field[len("version:"):]
    return None


def build(tag, repo, asset_names, sig_dir, pub_date):
    match = TAG_RE.match(tag)
    if not match:
        raise ManifestError(f"tag '{tag}' must look like vMAJOR.MINOR.PATCH[-prerelease]")
    version = match.group(1)
    if not REPO_RE.match(repo):
        raise ManifestError(f"repository '{repo}' must look like OWNER/NAME")

    found = {}
    for name in asset_names:
        classified = classify(name)
        if classified is None:
            continue
        key = "-".join(classified)
        if key in found:
            raise ManifestError(
                f"assets '{found[key]}' and '{name}' both map to platform '{key}'"
            )
        found[key] = name

    missing = sorted(EXPECTED_KEYS - found.keys())
    if missing:
        raise ManifestError("missing platforms: " + ", ".join(missing))

    platforms = {}
    recorded_versions = {}
    for key in sorted(EXPECTED_KEYS):
        name = found[key]
        sig_path = Path(sig_dir) / f"{name}.sig"
        if not sig_path.is_file():
            raise ManifestError(f"no signature file for '{name}'")
        signature = sig_path.read_text(encoding="utf-8").strip()
        if not signature:
            raise ManifestError(f"signature for '{name}' is empty")
        recorded = signed_version(signature, name)
        if recorded is not None and recorded != version:
            raise ManifestError(
                f"signature for '{name}' was made for version '{recorded}', tag is '{version}'"
            )
        platforms[key] = {
            "signature": signature,
            "url": f"https://github.com/{repo}/releases/download/{quote(tag, safe='')}/{quote(name, safe='')}",
        }
        recorded_versions[key] = recorded

    unversioned = sorted(k for k, v in recorded_versions.items() if v is None)
    if unversioned:
        raise ManifestError(
            "signatures carry no version (build with tauri CLI 2.12.0 or newer): "
            + ", ".join(unversioned)
        )

    manifest = {
        "version": version,
        "notes": NOTES,
        "pub_date": pub_date,
        "platforms": platforms,
    }
    unlisted = sorted(k for k in found if k not in EXPECTED_KEYS and k not in HELD_BACK_KEYS)
    held = sorted(k for k in HELD_BACK_KEYS if k in found)
    return manifest, held, unlisted


def render_summary(tag, manifest, held, unlisted):
    lines = [f"### Updater manifest for {tag}", "", "| Platform key | Asset |", "|---|---|"]
    for key, entry in sorted(manifest["platforms"].items()):
        lines.append(f"| `{key}` | `{entry['url'].rsplit('/', 1)[-1]}` |")
    lines.append("")
    lines.append(f"{len(manifest['platforms'])} platforms listed.")
    if held:
        lines.append("Built but not listed (manual download): " + ", ".join(f"`{k}`" for k in held))
    if unlisted:
        lines.append("WARNING: built but neither listed nor held back: " + ", ".join(f"`{k}`" for k in unlisted))
    return "\n".join(lines) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--assets", required=True, help="file with one asset name per line")
    parser.add_argument("--sig-dir", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--summary")
    parser.add_argument(
        "--pub-date",
        default=datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    )
    args = parser.parse_args(argv)

    try:
        names = [
            line.strip()
            for line in Path(args.assets).read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        manifest, held, unlisted = build(args.tag, args.repo, names, args.sig_dir, args.pub_date)
    except (ManifestError, OSError) as err:
        print(f"ERROR: {err}", file=sys.stderr)
        return 1

    Path(args.out).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    summary = render_summary(args.tag, manifest, held, unlisted)
    if args.summary:
        with open(args.summary, "a", encoding="utf-8") as handle:
            handle.write(summary)
    print(summary)
    return 0


if __name__ == "__main__":
    sys.exit(main())
