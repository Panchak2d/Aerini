#!/usr/bin/env bash
# Builds latest.json for a release and uploads it to that release.
#
# Usage: GH_TOKEN=... publish-updater-manifest.sh OWNER/NAME vX.Y.Z
#
# Works on a draft or a published release, and is safe to run again: an existing
# latest.json is replaced. Needs gh, jq and python3.
set -euo pipefail

REPO="${1:?usage: publish-updater-manifest.sh OWNER/NAME vX.Y.Z}"
TAG="${2:?usage: publish-updater-manifest.sh OWNER/NAME vX.Y.Z}"
: "${GH_TOKEN:?GH_TOKEN must be set}"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/sigs"

# Drafts have no tag URL until they are published, so they are found by listing.
gh api --paginate "repos/${REPO}/releases?per_page=100" | jq -s 'add // []' > "$WORK/releases.json"
MATCHES="$(jq --arg tag "$TAG" '[.[] | select(.tag_name == $tag)] | length' "$WORK/releases.json")"
if [ "$MATCHES" -ne 1 ]; then
  echo "ERROR: expected exactly one release for ${TAG}, found ${MATCHES}. Delete duplicate drafts and re-run." >&2
  exit 1
fi
jq --arg tag "$TAG" '.[] | select(.tag_name == $tag)' "$WORK/releases.json" > "$WORK/release.json"

RELEASE_ID="$(jq -r '.id' "$WORK/release.json")"
if [ "$(jq -r '.draft' "$WORK/release.json")" = "true" ]; then
  echo "release ${TAG} is a draft (id ${RELEASE_ID})"
else
  echo "NOTICE: release ${TAG} is already published; replacing latest.json changes what users are offered now."
fi

jq -r '.assets[].name' "$WORK/release.json" > "$WORK/assets.txt"

while IFS=$'\t' read -r id name; do
  gh api -H "Accept: application/octet-stream" "repos/${REPO}/releases/assets/${id}" > "$WORK/sigs/${name}"
done < <(jq -r '.assets[] | select(.name | endswith(".sig")) | [.id, .name] | @tsv' "$WORK/release.json")

python3 -I "$HERE/build-updater-manifest.py" \
  --tag "$TAG" --repo "$REPO" \
  --assets "$WORK/assets.txt" --sig-dir "$WORK/sigs" \
  --out "$WORK/latest.json" \
  ${GITHUB_STEP_SUMMARY:+--summary "$GITHUB_STEP_SUMMARY"}

while read -r stale_id; do
  gh api --method DELETE "repos/${REPO}/releases/assets/${stale_id}" > /dev/null
done < <(jq -r '.assets[] | select(.name == "latest.json") | .id' "$WORK/release.json")

UPLOADED_ID="$(curl --fail --silent --show-error --location \
  --request POST \
  --header "Authorization: Bearer ${GH_TOKEN}" \
  --header "Accept: application/vnd.github+json" \
  --header "Content-Type: application/json" \
  --data-binary "@${WORK}/latest.json" \
  "https://uploads.github.com/repos/${REPO}/releases/${RELEASE_ID}/assets?name=latest.json" \
  | jq -r '.id')"

gh api -H "Accept: application/octet-stream" "repos/${REPO}/releases/assets/${UPLOADED_ID}" > "$WORK/roundtrip.json"
cmp "$WORK/latest.json" "$WORK/roundtrip.json"
echo "latest.json uploaded to ${TAG}"
