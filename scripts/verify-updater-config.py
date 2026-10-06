#!/usr/bin/env python3
"""Fail if src-tauri/tauri.conf.json would ship a build that cannot self-update.

Usage: verify-updater-config.py OWNER/NAME [path/to/tauri.conf.json]
"""

import base64
import binascii
import json
import sys

DEFAULT_CONF = "src-tauri/tauri.conf.json"


def problems(conf, repo):
    found = []

    if conf.get("bundle", {}).get("createUpdaterArtifacts") is not True:
        found.append("bundle.createUpdaterArtifacts must be true")

    updater = conf.get("plugins", {}).get("updater")
    if not isinstance(updater, dict):
        return found + ["plugins.updater is missing"]

    found.extend(pubkey_problems(updater.get("pubkey")))

    expected = f"https://github.com/{repo}/releases/latest/download/latest.json"
    endpoints = updater.get("endpoints")
    if not isinstance(endpoints, list) or not endpoints:
        found.append("plugins.updater.endpoints must be a non-empty list")
    else:
        if not all(isinstance(e, str) and e.startswith("https://") for e in endpoints):
            found.append("every plugins.updater endpoint must be an https URL")
        elif endpoints[0].lower() != expected.lower():
            found.append(f"first plugins.updater endpoint must be {expected}, got {endpoints[0]}")

    if updater.get("requireSignedVersion") is not True:
        found.append("plugins.updater.requireSignedVersion must be true")

    return found


def pubkey_problems(pubkey):
    if not isinstance(pubkey, str) or not pubkey.strip():
        return ["plugins.updater.pubkey is empty"]
    try:
        lines = base64.b64decode(pubkey, validate=True).decode("utf-8").splitlines()
    except (binascii.Error, UnicodeDecodeError):
        return ["plugins.updater.pubkey is not base64 text"]
    if len(lines) < 2 or not lines[0].startswith("untrusted comment:"):
        return ["plugins.updater.pubkey is not a minisign public key file"]
    try:
        raw = base64.b64decode(lines[1], validate=True)
    except binascii.Error:
        return ["plugins.updater.pubkey key line is not base64"]
    if len(raw) != 42 or raw[:2] != b"Ed":
        return ["plugins.updater.pubkey key line is not an Ed25519 minisign key"]
    return []


def main(argv):
    if len(argv) not in (2, 3):
        print(__doc__.strip(), file=sys.stderr)
        return 2
    repo = argv[1]
    path = argv[2] if len(argv) == 3 else DEFAULT_CONF
    try:
        with open(path, encoding="utf-8") as handle:
            conf = json.load(handle)
    except (OSError, ValueError) as err:
        print(f"ERROR: cannot read {path}: {err}", file=sys.stderr)
        return 1

    found = problems(conf, repo)
    for item in found:
        print(f"ERROR: {item}", file=sys.stderr)
    if not found:
        print(f"updater config OK ({path})")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
