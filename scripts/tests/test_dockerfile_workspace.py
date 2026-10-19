import re
import shutil
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path

from helpers import SCRIPTS

ROOT = SCRIPTS.parent


def dockerfile_text():
    return (ROOT / "Dockerfile").read_text(encoding="utf-8")


def copied_crates():
    return set(re.findall(r"^COPY (aerini-[\w-]+) \./\1$", dockerfile_text(), re.M))


def workspace_edit_command():
    joined = re.sub(r"\\\n\s*", " ", dockerfile_text())
    match = re.search(r"^RUN (sed -i .*)$", joined, re.M)
    assert match, "workspace-members RUN instruction not found in Dockerfile"
    return match.group(1)


def run_edit(workdir):
    return subprocess.run(
        ["sh", "-c", workspace_edit_command()], cwd=workdir, capture_output=True, text=True
    )


@unittest.skipUnless(
    subprocess.run(["sed", "--version"], capture_output=True).returncode == 0,
    "the Dockerfile's sed edit needs GNU sed, as in the rust build image",
)
class DockerfileWorkspaceTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.manifest = Path(self.tmp.name) / "Cargo.toml"
        shutil.copy(ROOT / "Cargo.toml", self.manifest)

    def load(self):
        return tomllib.loads(self.manifest.read_text(encoding="utf-8"))["workspace"]

    def test_members_are_exactly_the_crates_the_image_copies(self):
        result = run_edit(self.tmp.name)
        self.assertEqual(result.returncode, 0, result.stderr)
        crates = copied_crates()
        self.assertTrue(crates)
        self.assertEqual(set(self.load()["members"]), crates)
        for crate in crates:
            self.assertTrue((ROOT / crate / "Cargo.toml").is_file(), crate)

    def test_edit_keeps_the_rest_of_the_workspace_and_is_repeatable(self):
        before = tomllib.loads(self.manifest.read_text(encoding="utf-8"))
        self.assertEqual(run_edit(self.tmp.name).returncode, 0)
        self.assertEqual(run_edit(self.tmp.name).returncode, 0)
        after = tomllib.loads(self.manifest.read_text(encoding="utf-8"))
        self.assertEqual(after["workspace"]["exclude"], before["workspace"]["exclude"])
        self.assertEqual(after["workspace"]["resolver"], before["workspace"]["resolver"])
        self.assertEqual(after["workspace"]["package"], before["workspace"]["package"])

    def test_guard_fails_when_the_members_block_is_missing(self):
        self.manifest.write_text('[workspace]\nresolver = "2"\n', encoding="utf-8")
        self.assertNotEqual(run_edit(self.tmp.name).returncode, 0)


if __name__ == "__main__":
    unittest.main()
