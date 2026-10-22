#!/usr/bin/env bash
# Writes the download list, grouped by system, into the notes of the release for a tag.
#
# Usage: GH_TOKEN=... publish-release-notes.sh OWNER/NAME vX.Y.Z
#
# Works on a draft or a published release, and is safe to run again: the list written
# before is replaced and the rest of the notes are left alone. Needs gh, jq and python3.
set -euo pipefail

REPO="${1:?usage: publish-release-notes.sh OWNER/NAME vX.Y.Z}"
TAG="${2:?usage: publish-release-notes.sh OWNER/NAME vX.Y.Z}"
: "${GH_TOKEN:?GH_TOKEN must be set}"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Drafts have no tag URL until they are published, so they are found by listing.
gh api --paginate "repos/${REPO}/releases?per_page=100" | jq -s 'add // []' > "$WORK/releases.json"
MATCHES="$(jq --arg tag "$TAG" '[.[] | select(.tag_name == $tag)] | length' "$WORK/releases.json")"
if [ "$MATCHES" -ne 1 ]; then
  echo "ERROR: expected exactly one release for ${TAG}, found ${MATCHES}. Delete duplicate drafts and re-run." >&2
  exit 1
fi
jq --arg tag "$TAG" '.[] | select(.tag_name == $tag)' "$WORK/releases.json" > "$WORK/release.json"

RELEASE_ID="$(jq -r '.id' "$WORK/release.json")"
jq -j '.body // ""' "$WORK/release.json" > "$WORK/body.md"

# Assets come from their own paged endpoint, not from the release object.
gh api --paginate "repos/${REPO}/releases/${RELEASE_ID}/assets?per_page=100" --jq '.[].name' > "$WORK/assets.txt"
if [ ! -s "$WORK/assets.txt" ]; then
  echo "ERROR: release ${TAG} has no assets to list" >&2
  exit 1
fi

python3 -I "$HERE/render-release-notes.py" \
  --tag "$TAG" --repo "$REPO" \
  --assets "$WORK/assets.txt" --body "$WORK/body.md" \
  --out "$WORK/notes.md"

jq -n --rawfile body "$WORK/notes.md" '{body: $body}' \
  | gh api --method PATCH "repos/${REPO}/releases/${RELEASE_ID}" --input - > /dev/null

gh api "repos/${REPO}/releases/${RELEASE_ID}" --jq '.body // ""' > "$WORK/stored.md"
if [ "$(tr -d '\r' < "$WORK/stored.md")" != "$(tr -d '\r' < "$WORK/notes.md")" ]; then
  echo "ERROR: the notes stored on ${TAG} differ from what was sent" >&2
  exit 1
fi
echo "download list written to the notes of ${TAG}"
