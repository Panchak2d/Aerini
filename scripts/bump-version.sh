#!/usr/bin/env bash
# Sets one version everywhere a release needs it, regenerates the reference docs
# that embed the engine version, then runs the same check the release workflow runs.
#
# Usage: scripts/bump-version.sh X.Y.Z[-id.N] [--skip-docs]
#   scripts/bump-version.sh 0.4.4-rc.2
#   scripts/bump-version.sh 0.4.4
#
# --skip-docs leaves docs/reference/* alone (a full engine build takes a long time).
# CI fails the commit if they are stale, so only use it when you regenerate them yourself.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

usage() { echo "usage: scripts/bump-version.sh X.Y.Z[-id.N] [--skip-docs]" >&2; exit 2; }

VERSION="${1:-}"
[ -n "$VERSION" ] || usage
SKIP_DOCS=false
case "${2:-}" in
  "") ;;
  --skip-docs) SKIP_DOCS=true ;;
  *) usage ;;
esac
[ "$#" -le 2 ] || usage

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+\.[0-9]+)?$ ]]; then
  echo "ERROR: '$VERSION' must look like X.Y.Z or X.Y.Z-id.N (for example 0.4.4 or 0.4.4-rc.2)" >&2
  exit 2
fi

for tool in python3 npm cargo; do
  command -v "$tool" >/dev/null 2>&1 || { echo "ERROR: $tool is required but not installed" >&2; exit 1; }
done

echo "Setting version $VERSION"

npm version "$VERSION" --no-git-tag-version --allow-same-version >/dev/null

python3 - "$VERSION" <<'PY'
import json, re, sys

version = sys.argv[1]
core, _, pre = version.partition("-")

def rewrite(path, edit):
    with open(path, encoding="utf-8", newline="") as handle:
        text = handle.read()
    new = edit(text)
    with open(path, "w", encoding="utf-8", newline="") as handle:
        handle.write(new)

def sub_once(pattern, repl, text, what):
    new, count = re.subn(pattern, repl, text, count=1, flags=re.M)
    if count != 1:
        sys.exit(f"ERROR: could not find {what}")
    return new

rewrite("Cargo.toml", lambda t: sub_once(
    r'(^\[workspace\.package\]\n(?:(?!\[).*\n)*?version\s*=\s*")[^"]*(")',
    lambda m: m.group(1) + version + m.group(2), t, "[workspace.package] version in Cargo.toml"))

WIX_BLOCK = re.compile(r'    "windows": \{\n      "wix": \{\n        "version": "[^"]*"\n      \}\n    \},\n')

def tauri_conf(text):
    text = sub_once(r'^(  "version": ")[^"]*(")', lambda m: m.group(1) + version + m.group(2),
                    text, "version in src-tauri/tauri.conf.json")
    text = WIX_BLOCK.sub("", text)
    if pre:
        # MSI versions are numeric only: X.Y.Z-rc.N becomes X.Y.Z.N.
        build = int(pre.rsplit(".", 1)[1])
        if build > 65535:
            sys.exit("ERROR: the number after the last '.' of the pre-release must be 65535 or less (MSI limit)")
        block = ('    "windows": {\n      "wix": {\n        "version": "%s.%d"\n      }\n    },\n' % (core, build))
        text = sub_once(r'(^    "externalBin": \[[^\]]*\],\n)', lambda m: m.group(1) + block,
                        text, '"externalBin" line in src-tauri/tauri.conf.json')
    return text

rewrite("src-tauri/tauri.conf.json", tauri_conf)
json.load(open("src-tauri/tauri.conf.json", encoding="utf-8"))
PY

cargo update --workspace

if [ "$SKIP_DOCS" = false ]; then
  touch aerini-engine/src/lib.rs
  AERINI_REGEN_DOCS=1 cargo test -p aerini-engine --locked --features schema \
    --test reference_docs --test workflow_schema
else
  echo "Skipped regenerating docs/reference/*; regenerate them before you commit."
fi

bash scripts/verify-release-tag.sh "v$VERSION"

echo ""
echo "Version files are set to $VERSION. Remaining by hand:"
case "$VERSION" in
  *-*) echo "  - nothing else for a pre-release tag" ;;
  *)   echo "  - move the [Unreleased] entries in CHANGELOG.md under '## [$VERSION]' (the release is refused without it)" ;;
esac
echo "  - commit on a branch, open a pull request, tag after CI is green on main"
