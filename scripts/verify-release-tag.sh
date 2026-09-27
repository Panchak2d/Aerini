#!/usr/bin/env bash
set -euo pipefail

TAG="${1:?usage: verify-release-tag.sh <tag>}"

if ! [[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "ERROR: tag '$TAG' must look like vMAJOR.MINOR.PATCH or vMAJOR.MINOR.PATCH-prerelease" >&2
  exit 1
fi
EXPECTED="${TAG#v}"

PACKAGE_JSON_VERSION="$(python3 -c 'import json; print(json.load(open("package.json", encoding="utf-8"))["version"])')"
TAURI_CONF_VERSION="$(python3 -c 'import json; print(json.load(open("src-tauri/tauri.conf.json", encoding="utf-8"))["version"])')"
CARGO_VERSION="$(awk '
  /^\[workspace\.package\]/ { in_section = 1; next }
  /^\[/ { in_section = 0 }
  in_section && /^version[[:space:]]*=/ { gsub(/["[:space:]]/, "", $0); split($0, kv, "="); print kv[2]; exit }
' Cargo.toml)"

echo "tag:                    ${TAG} (expects ${EXPECTED})"
echo "package.json:           ${PACKAGE_JSON_VERSION}"
echo "src-tauri/tauri.conf:   ${TAURI_CONF_VERSION}"
echo "Cargo.toml workspace:   ${CARGO_VERSION}"

STATUS=0
for pair in "package.json=${PACKAGE_JSON_VERSION}" "src-tauri/tauri.conf.json=${TAURI_CONF_VERSION}" "Cargo.toml [workspace.package]=${CARGO_VERSION}"; do
  file="${pair%=*}"
  value="${pair##*=}"
  if [ "$value" != "$EXPECTED" ]; then
    echo "ERROR: ${file} version is '${value}', tag expects '${EXPECTED}'" >&2
    STATUS=1
  fi
done
[ "$STATUS" -eq 0 ] || exit 1

PRERELEASE=false
case "$EXPECTED" in
  *-*) PRERELEASE=true ;;
esac
echo "prerelease: ${PRERELEASE}"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  echo "prerelease=${PRERELEASE}" >> "$GITHUB_OUTPUT"
fi
