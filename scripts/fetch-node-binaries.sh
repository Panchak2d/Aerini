#!/usr/bin/env bash
set -euo pipefail

NODE_VERSION="$(head -n1 NODE_VERSION 2>/dev/null | tr -d '\r' || true)"
if ! [[ "$NODE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "ERROR: NODE_VERSION file (repo root, run this script from repo root) must have a bare semver as its first line, got: '${NODE_VERSION}'" >&2
  exit 1
fi
DIST_BASE="https://nodejs.org/dist/v${NODE_VERSION}"
OUT_DIR="src-tauri/binaries"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

# rust_triple:archive_filename:kind:in_archive_binary_path
# kind=tar -> node-v.../bin/node, two levels under the extracted top-level dir
# kind=zip -> node-v.../node.exe, one level under the extracted top-level dir
TARGETS=(
  "x86_64-unknown-linux-gnu:node-v${NODE_VERSION}-linux-x64.tar.xz:tar:bin/node"
  "aarch64-unknown-linux-gnu:node-v${NODE_VERSION}-linux-arm64.tar.xz:tar:bin/node"
  "aarch64-apple-darwin:node-v${NODE_VERSION}-darwin-arm64.tar.xz:tar:bin/node"
  "x86_64-pc-windows-msvc:node-v${NODE_VERSION}-win-x64.zip:zip:node.exe"
)

if [ -n "${RUNNER_OS:-}" ]; then
  case "${RUNNER_OS}:${RUNNER_ARCH:-}" in
    Linux:X64)   want="x86_64-unknown-linux-gnu" ;;
    Linux:ARM64) want="aarch64-unknown-linux-gnu" ;;
    macOS:ARM64) want="aarch64-apple-darwin" ;;
    Windows:X64) want="x86_64-pc-windows-msvc" ;;
    *)
      echo "ERROR: no known Node target for RUNNER_OS=${RUNNER_OS} RUNNER_ARCH=${RUNNER_ARCH:-<unset>}" >&2
      exit 1
      ;;
  esac
  filtered=()
  for entry in "${TARGETS[@]}"; do
    [[ "$entry" == "${want}:"* ]] && filtered+=("$entry")
  done
  TARGETS=("${filtered[@]}")
fi

command -v curl >/dev/null 2>&1 || { echo "ERROR: 'curl' is required but not installed" >&2; exit 1; }
command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 || {
  echo "ERROR: need 'sha256sum' or 'shasum' to verify downloads, neither is installed" >&2; exit 1
}
need_tar=false
need_zip=false
for entry in "${TARGETS[@]}"; do
  case "$entry" in
    *:tar:*) need_tar=true ;;
    *:zip:*) need_zip=true ;;
  esac
done
if $need_tar; then
  command -v tar >/dev/null 2>&1 || { echo "ERROR: 'tar' is required but not installed" >&2; exit 1; }
fi
if $need_zip; then
  command -v unzip >/dev/null 2>&1 || command -v powershell.exe >/dev/null 2>&1 || {
    echo "ERROR: need 'unzip' or 'powershell.exe' to extract the win-x64 archive, neither is installed" >&2; exit 1
  }
fi

mkdir -p "$OUT_DIR"
curl -fsSL "${DIST_BASE}/SHASUMS256.txt" -o "$WORK_DIR/SHASUMS256.txt"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

verify_sha256() {
  local file="$1" remote_name="$2"
  local expected actual
  expected="$(awk -v f="$remote_name" '$2==f {print $1; exit}' "$WORK_DIR/SHASUMS256.txt")"
  if [ -z "$expected" ]; then
    echo "ERROR: no SHASUMS256.txt entry for ${remote_name}" >&2
    exit 1
  fi
  actual="$(sha256_of "$file")"
  if [ "$expected" != "$actual" ]; then
    echo "ERROR: checksum mismatch for ${remote_name}" >&2
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    exit 1
  fi
}

for entry in "${TARGETS[@]}"; do
  IFS=':' read -r triple archive kind inner_path <<< "$entry"
  local_archive="$WORK_DIR/$archive"
  extract_dir="$WORK_DIR/extract-${triple}"
  mkdir -p "$extract_dir"

  curl -fsSL "${DIST_BASE}/${archive}" -o "$local_archive"
  verify_sha256 "$local_archive" "$archive"

  if [ "$kind" = "tar" ]; then
    tar -xf "$local_archive" -C "$extract_dir"
    src_bin="$(find "$extract_dir" -mindepth 3 -maxdepth 3 -type f -path "*/${inner_path}" | head -n1)"
  else
    if command -v unzip >/dev/null 2>&1; then
      unzip -q "$local_archive" -d "$extract_dir"
    else
      powershell.exe -NoProfile -Command "Expand-Archive -LiteralPath '$(cygpath -w "$local_archive")' -DestinationPath '$(cygpath -w "$extract_dir")' -Force -ErrorAction Stop"
    fi
    src_bin="$(find "$extract_dir" -mindepth 2 -maxdepth 2 -type f -name "${inner_path}" | head -n1)"
  fi
  if [ -z "$src_bin" ]; then
    echo "ERROR: ${inner_path} not found inside extracted ${archive} - archive layout may have changed" >&2
    exit 1
  fi

  suffix=""
  [ "$kind" = "zip" ] && suffix=".exe"
  staged="${OUT_DIR}/node-bundled-${triple}${suffix}"
  cp "$src_bin" "$staged"
  [ "$kind" != "zip" ] && chmod +x "$staged"

  size_mb=$(( $(wc -c < "$staged") / 1024 / 1024 ))
  echo "staged ${staged} (${size_mb} MB, sha256 verified against ${archive})"
done

echo
if [ -f "${OUT_DIR}/node-bundled-x86_64-unknown-linux-gnu" ]; then
  echo "Server target (linux-x64) reuses: ${OUT_DIR}/node-bundled-x86_64-unknown-linux-gnu"
  echo "The Docker image stages its own copy independently in its own build stage -"
  echo "this script's output here is not consumed by that build."
fi
