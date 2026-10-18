import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from helpers import SCRIPTS

SCRIPT = SCRIPTS / "verify-release-tag.sh"


def run_in_repo(tag, changelog, version="1.2.3"):
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "src-tauri").mkdir()
        (root / "package.json").write_text('{"version": "%s"}' % version, encoding="utf-8")
        (root / "src-tauri" / "tauri.conf.json").write_text('{"version": "%s"}' % version, encoding="utf-8")
        (root / "Cargo.toml").write_text('[workspace.package]\nversion = "%s"\n' % version, encoding="utf-8")
        (root / "CHANGELOG.md").write_text(changelog, encoding="utf-8")
        output = root / "github_output"
        env = {**os.environ, "GITHUB_OUTPUT": str(output)}
        result = subprocess.run(
            ["bash", str(SCRIPT), tag], cwd=root, env=env, capture_output=True, text=True
        )
        written = output.read_text(encoding="utf-8") if output.exists() else ""
        return result, written


class ChangelogCheckTest(unittest.TestCase):
    def test_stable_tag_passes_with_dated_changelog_heading(self):
        result, written = run_in_repo("v1.2.3", "# Changelog\n\n## [Unreleased]\n\n## [1.2.3] - 2026-01-01\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("prerelease=false", written)

    def test_stable_tag_fails_without_its_own_changelog_section(self):
        result, _ = run_in_repo("v1.2.3", "# Changelog\n\n## [Unreleased]\n\n## [1.2.30]\n")
        self.assertEqual(result.returncode, 1)
        self.assertIn("CHANGELOG.md has no '## [1.2.3]' section", result.stderr)

    def test_prerelease_tag_does_not_need_a_changelog_section(self):
        result, written = run_in_repo("v1.2.3-rc.1", "# Changelog\n\n## [Unreleased]\n", version="1.2.3-rc.1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("prerelease=true", written)


if __name__ == "__main__":
    unittest.main()
