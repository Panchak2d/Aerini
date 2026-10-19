import hashlib
import os
import re
import subprocess
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

from helpers import SCRIPTS, load

package_mcp = load("package-mcp.py")
manifest = load("build-updater-manifest.py")

UPLOAD = SCRIPTS / "upload-release-assets.sh"
ASSET_NAME_RE = re.compile(r"^[A-Za-z0-9._-]+$")
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
)
NAMES = ("aerini-mcp", "aerini-mcp-offline")


def stage(tmp, content=b"program"):
    binary = Path(tmp) / "bin"
    binary.write_bytes(content)
    license_file = Path(tmp) / "LICENSE"
    license_file.write_text("license text\n", encoding="utf-8")
    return binary, license_file


class AssetNameTest(unittest.TestCase):
    def test_no_mcp_asset_is_read_as_an_updater_installer(self):
        for name in NAMES:
            for target in TARGETS:
                for version in ("1.2.3", "1.2.3-rc.1"):
                    archive = package_mcp.archive_name(name, version, target)
                    for asset in (archive, archive + ".sha256"):
                        self.assertIsNone(manifest.classify(asset), asset)

    def test_every_mcp_asset_name_is_accepted_by_the_upload_script(self):
        for name in NAMES:
            for target in TARGETS:
                archive = package_mcp.archive_name(name, "1.2.3-rc.1", target)
                self.assertRegex(archive, ASSET_NAME_RE)
                self.assertRegex(archive + ".sha256", ASSET_NAME_RE)


class PackageTest(unittest.TestCase):
    def test_tar_holds_the_program_and_license_and_the_checksum_matches(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary, license_file = stage(tmp)
            archive, checksum = package_mcp.package(
                binary, "aerini-mcp-offline", TARGETS[0], "1.2.3", license_file, Path(tmp) / "out"
            )
            self.assertEqual(archive.name, "aerini-mcp-offline_1.2.3_x86_64-unknown-linux-gnu.tar.gz")
            with tarfile.open(archive) as handle:
                members = {m.name: m for m in handle.getmembers()}
                self.assertEqual(set(members), {"aerini-mcp-offline", "LICENSE"})
                self.assertEqual(members["aerini-mcp-offline"].mode, 0o755)
                self.assertEqual(handle.extractfile("aerini-mcp-offline").read(), b"program")
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            self.assertEqual(checksum.read_text(encoding="utf-8"), f"{digest}  {archive.name}\n")
            self.assertEqual(checksum.name, archive.name + ".sha256")

    def test_windows_zip_names_the_program_with_exe(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary, license_file = stage(tmp)
            archive, _ = package_mcp.package(
                binary, "aerini-mcp", TARGETS[3], "1.2.3-rc.1", license_file, Path(tmp) / "out"
            )
            with zipfile.ZipFile(archive) as handle:
                self.assertEqual(sorted(handle.namelist()), ["LICENSE", "aerini-mcp.exe"])
                self.assertEqual(handle.read("aerini-mcp.exe"), b"program")

    def test_output_is_reproducible(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary, license_file = stage(tmp)
            first, _ = package_mcp.package(binary, "aerini-mcp", TARGETS[0], "1.2.3", license_file, Path(tmp) / "a")
            second, _ = package_mcp.package(binary, "aerini-mcp", TARGETS[0], "1.2.3", license_file, Path(tmp) / "b")
            self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_bad_input_is_refused_and_writes_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary, license_file = stage(tmp)
            empty = Path(tmp) / "empty"
            empty.write_bytes(b"")
            out = Path(tmp) / "out"
            cases = (
                (binary, "aerini-server", TARGETS[0], "1.2.3", license_file),
                (binary, "aerini-mcp", TARGETS[0], "v1.2.3", license_file),
                (binary, "aerini-mcp", "linux", "1.2.3", license_file),
                (Path(tmp) / "missing", "aerini-mcp", TARGETS[0], "1.2.3", license_file),
                (empty, "aerini-mcp", TARGETS[0], "1.2.3", license_file),
                (binary, "aerini-mcp", TARGETS[0], "1.2.3", Path(tmp) / "missing"),
            )
            for case in cases:
                with self.assertRaises(package_mcp.PackageError, msg=str(case)):
                    package_mcp.package(*case, out)
            self.assertFalse(out.exists())


class UploadScriptArgumentTest(unittest.TestCase):
    def run_upload(self, *args):
        env = {**os.environ, "GH_TOKEN": "not-used"}
        return subprocess.run(["bash", str(UPLOAD), *args], env=env, capture_output=True, text=True)

    def test_rejects_a_malformed_repository_tag_or_file_before_any_network_call(self):
        with tempfile.TemporaryDirectory() as tmp:
            good = Path(tmp) / "a.tar.gz"
            good.write_bytes(b"x")
            bad_name = Path(tmp) / "a b.tar.gz"
            bad_name.write_bytes(b"x")
            for args in (
                ("nope", "v1.2.3", str(good)),
                ("owner/name", "1.2.3", str(good)),
                ("owner/name", "v1.2.3", str(Path(tmp) / "missing")),
                ("owner/name", "v1.2.3", str(bad_name)),
                ("owner/name", "v1.2.3"),
            ):
                result = self.run_upload(*args)
                self.assertEqual(result.returncode, 1, args)
                self.assertIn("ERROR", result.stderr)


if __name__ == "__main__":
    unittest.main()
