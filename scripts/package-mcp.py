#!/usr/bin/env python3
"""Package a built aerini-mcp or aerini-server program as a release archive with a checksum file.

Usage:
  package-mcp.py --binary PATH --name aerini-mcp|aerini-mcp-offline|aerini-server --target TRIPLE \
      --version X.Y.Z[-pre] --license LICENSE --out-dir DIR

Writes "<name>_<version>_<target>.tar.gz" (".zip" for a Windows target) holding the
program, named "<name>" ("<name>.exe" on Windows), and the license text, plus the same
name with ".sha256" appended, in the format "sha256sum -c" reads.

The archive name only uses characters that scripts/upload-release-assets.sh accepts, and
never ends in a suffix that scripts/build-updater-manifest.py reads as an installer.
"""

import argparse
import hashlib
import re
import sys
import tarfile
import zipfile
from pathlib import Path

NAMES = ("aerini-mcp", "aerini-mcp-offline", "aerini-server")
VERSION_RE = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$")
TARGET_RE = re.compile(r"^[a-z0-9_]+(?:-[a-z0-9_]+){2,3}$")
FIXED_MTIME = 0


class PackageError(Exception):
    pass


def archive_name(name, version, target):
    suffix = ".zip" if "windows" in target else ".tar.gz"
    return f"{name}_{version}_{target}{suffix}"


def write_tar(path, program, program_name, license_path):
    with tarfile.open(path, "w:gz") as archive:
        for source, member, mode in (
            (program, program_name, 0o755),
            (license_path, "LICENSE", 0o644),
        ):
            info = archive.gettarinfo(str(source), arcname=member)
            info.mode = mode
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mtime = FIXED_MTIME
            with open(source, "rb") as handle:
                archive.addfile(info, handle)


def write_zip(path, program, program_name, license_path):
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        for source, member in ((program, program_name), (license_path, "LICENSE")):
            info = zipfile.ZipInfo(member, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, Path(source).read_bytes())


def package(binary, name, target, version, license_path, out_dir):
    if name not in NAMES:
        raise PackageError(f"name must be one of {', '.join(NAMES)}")
    if not VERSION_RE.match(version):
        raise PackageError(f"version '{version}' must look like MAJOR.MINOR.PATCH[-prerelease]")
    if not TARGET_RE.match(target):
        raise PackageError(f"target '{target}' is not a target triple")
    for label, path in (("binary", binary), ("license", license_path)):
        if not Path(path).is_file():
            raise PackageError(f"{label} '{path}' is not a file")
    if Path(binary).stat().st_size == 0:
        raise PackageError(f"binary '{binary}' is empty")

    windows = "windows" in target
    program_name = name + (".exe" if windows else "")
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    archive_path = out / archive_name(name, version, target)
    (write_zip if windows else write_tar)(archive_path, binary, program_name, license_path)

    digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
    checksum_path = out / (archive_path.name + ".sha256")
    checksum_path.write_text(f"{digest}  {archive_path.name}\n", encoding="utf-8")
    return archive_path, checksum_path


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--license", required=True)
    parser.add_argument("--out-dir", required=True)
    args = parser.parse_args(argv)
    try:
        archive_path, checksum_path = package(
            args.binary, args.name, args.target, args.version, args.license, args.out_dir
        )
    except PackageError as err:
        print(f"ERROR: {err}", file=sys.stderr)
        return 1
    print(archive_path)
    print(checksum_path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
