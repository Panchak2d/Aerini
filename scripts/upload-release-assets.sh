#!/usr/bin/env bash
# Uploads files to the release for a tag, draft or published.
#
# Usage: GH_TOKEN=... upload-release-assets.sh OWNER/NAME vX.Y.Z FILE...
#
# Safe to run again: an asset with the same name is replaced. Needs gh and curl.
# Drafts have no tag URL until they are published, so the release is found by
# listing, as publish-updater-manifest.sh does.
set -euo pipefail

REPO="${1:?usage: upload-release-assets.sh OWNER/NAME vX.Y.Z FILE...}"
TAG="${2:?usage: upload-release-assets.sh OWNER/NAME vX.Y.Z FILE...}"
shift 2
[ "$#" -gt 0 ] || { echo "ERROR: no files given" >&2; exit 1; }
: "${GH_TOKEN:?GH_TOKEN must be set}"

if ! [[ "$REPO" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]; then
  echo "ERROR: repository '${REPO}' must look like OWNER/NAME" >&2
  exit 1
fi
if ! [[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "ERROR: tag '${TAG}' must look like vMAJOR.MINOR.PATCH[-prerelease]" >&2
  exit 1
fi

for file in "$@"; do
  [ -f "$file" ] || { echo "ERROR: '${file}' is not a file" >&2; exit 1; }
  if ! [[ "$(basename "$file")" =~ ^[A-Za-z0-9._-]+$ ]]; then
    echo "ERROR: '${file}' has characters that are not allowed in a release asset name here" >&2
    exit 1
  fi
done

IDS="$(gh api --paginate "repos/${REPO}/releases?per_page=100" --jq ".[] | select(.tag_name == \"${TAG}\") | .id")"
MATCHES="$(printf '%s\n' "$IDS" | grep -c . || true)"
if [ "$MATCHES" -ne 1 ]; then
  echo "ERROR: expected exactly one release for ${TAG}, found ${MATCHES}. Delete duplicate drafts and re-run." >&2
  exit 1
fi
RELEASE_ID="$IDS"

asset_field() {
  gh api --paginate "repos/${REPO}/releases/${RELEASE_ID}/assets?per_page=100" \
    --jq ".[] | select(.name == \"$1\") | .$2"
}

for file in "$@"; do
  name="$(basename "$file")"
  case "$name" in
    *.zip) ctype="application/zip" ;;
    *.tar.gz) ctype="application/gzip" ;;
    *.sha256) ctype="text/plain" ;;
    *) ctype="application/octet-stream" ;;
  esac

  while read -r stale_id; do
    [ -n "$stale_id" ] || continue
    gh api --method DELETE "repos/${REPO}/releases/assets/${stale_id}" > /dev/null
  done < <(asset_field "$name" id)

  curl --fail --silent --show-error \
    --request POST \
    --header "Authorization: Bearer ${GH_TOKEN}" \
    --header "Accept: application/vnd.github+json" \
    --header "Content-Type: ${ctype}" \
    --data-binary "@${file}" \
    "https://uploads.github.com/repos/${REPO}/releases/${RELEASE_ID}/assets?name=${name}" > /dev/null

  expected="$(wc -c < "$file" | tr -d ' ')"
  stored="$(asset_field "$name" size)"
  if [ "$stored" != "$expected" ]; then
    echo "ERROR: ${name} is ${stored:-missing} bytes on the release, expected ${expected}" >&2
    exit 1
  fi
  echo "uploaded ${name} (${expected} bytes) to ${TAG}"
done
