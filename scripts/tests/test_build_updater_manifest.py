import json
import tempfile
import unittest
from pathlib import Path

from helpers import fake_sig, load

mod = load("build-updater-manifest.py")

TAG = "v0.5.0"
REPO = "Panchak2d/aerini"
V = "0.5.0"

COMPLETE = [
    f"Aerini_{V}_amd64.AppImage",
    f"Aerini_{V}_aarch64.AppImage",
    f"Aerini_{V}_amd64.deb",
    f"Aerini_{V}_arm64.deb",
    f"Aerini-{V}-1.x86_64.rpm",
    f"Aerini-{V}-1.aarch64.rpm",
    f"Aerini_{V}_x64-setup.exe",
    f"Aerini_{V}_x64_en-US.msi",
    f"Aerini_{V}_aarch64.app.tar.gz",
    f"Aerini_{V}_aarch64.dmg",
]


class Base(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.sigs = Path(self.dir.name)

    def write_sigs(self, names, version=V):
        for name in names:
            if not name.endswith(".dmg"):
                (self.sigs / f"{name}.sig").write_text(fake_sig(version, name) + "\n")

    def build(self, names, **kwargs):
        return mod.build(kwargs.get("tag", TAG), REPO, names, self.sigs, "2026-10-05T12:00:00Z")


class ManifestTests(Base):
    def test_complete_set_lists_expected_platforms_and_holds_back_macos(self):
        self.write_sigs(COMPLETE)
        manifest, held, unlisted = self.build(COMPLETE)
        self.assertEqual(set(manifest["platforms"]), set(mod.EXPECTED_KEYS))
        self.assertNotIn("darwin-aarch64-app", manifest["platforms"])
        self.assertEqual(held, ["darwin-aarch64-app"])
        self.assertEqual(unlisted, [])
        self.assertEqual(manifest["version"], V)
        entry = manifest["platforms"]["windows-x86_64-msi"]
        self.assertEqual(
            entry["url"],
            f"https://github.com/{REPO}/releases/download/{TAG}/Aerini_{V}_x64_en-US.msi",
        )
        self.assertEqual(entry["signature"], fake_sig(V, f"Aerini_{V}_x64_en-US.msi"))

    def test_architecture_aliases_map_to_expected_keys(self):
        self.assertEqual(mod.classify(f"Aerini-{V}-1.x86_64.rpm"), ("linux", "x86_64", "rpm"))
        self.assertEqual(mod.classify(f"Aerini_{V}_arm64.deb"), ("linux", "aarch64", "deb"))
        self.assertEqual(mod.classify(f"Aerini_{V}_x64-setup.exe"), ("windows", "x86_64", "nsis"))

    def test_missing_platform_fails(self):
        names = [n for n in COMPLETE if not n.endswith(".rpm") or "aarch64" not in n]
        self.write_sigs(names)
        with self.assertRaisesRegex(mod.ManifestError, "linux-aarch64-rpm"):
            self.build(names)

    def test_empty_signature_fails(self):
        self.write_sigs(COMPLETE)
        (self.sigs / f"Aerini_{V}_amd64.deb.sig").write_text("\n")
        with self.assertRaisesRegex(mod.ManifestError, "is empty"):
            self.build(COMPLETE)

    def test_signature_for_other_version_fails(self):
        self.write_sigs(COMPLETE)
        (self.sigs / f"Aerini_{V}_amd64.deb.sig").write_text(fake_sig("0.4.1"))
        with self.assertRaisesRegex(mod.ManifestError, "made for version '0.4.1'"):
            self.build(COMPLETE)

    def test_signature_without_version_fails(self):
        self.write_sigs(COMPLETE, version=None)
        with self.assertRaisesRegex(mod.ManifestError, "carry no version"):
            self.build(COMPLETE)

    def test_missing_signature_file_fails(self):
        self.write_sigs(COMPLETE)
        (self.sigs / f"Aerini_{V}_amd64.deb.sig").unlink()
        with self.assertRaisesRegex(mod.ManifestError, "no signature file"):
            self.build(COMPLETE)

    def test_two_assets_for_one_platform_fail(self):
        names = COMPLETE + [f"Aerini_{V}_amd64.extra.deb"]
        self.write_sigs(names)
        with self.assertRaisesRegex(mod.ManifestError, "both map to platform"):
            self.build(names)

    def test_bad_tag_fails(self):
        self.write_sigs(COMPLETE)
        with self.assertRaisesRegex(mod.ManifestError, "must look like"):
            self.build(COMPLETE, tag="0.5.0")

    def test_prerelease_tag_keeps_full_version(self):
        names = [n.replace(V, "0.5.0-rc.1") for n in COMPLETE]
        self.write_sigs(names, version="0.5.0-rc.1")
        manifest, _, _ = self.build(names, tag="v0.5.0-rc.1")
        self.assertEqual(manifest["version"], "0.5.0-rc.1")


class CliTests(Base):
    def test_cli_writes_manifest_and_nothing_on_failure(self):
        self.write_sigs(COMPLETE)
        assets = self.sigs / "assets.txt"
        assets.write_text("\n".join(COMPLETE) + "\n")
        out = self.sigs / "latest.json"
        summary = self.sigs / "summary.md"
        args = ["--tag", TAG, "--repo", REPO, "--assets", str(assets),
                "--sig-dir", str(self.sigs), "--out", str(out), "--summary", str(summary)]
        self.assertEqual(mod.main(args), 0)
        self.assertEqual(json.loads(out.read_text())["version"], V)
        self.assertIn("darwin-aarch64-app", summary.read_text())

        out.unlink()
        assets.write_text("\n".join(COMPLETE[:-3]) + "\n")
        self.assertEqual(mod.main(args), 1)
        self.assertFalse(out.exists())


if __name__ == "__main__":
    unittest.main()
