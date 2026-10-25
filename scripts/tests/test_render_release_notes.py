import tempfile
import unittest
from pathlib import Path
from urllib.parse import quote

from helpers import load

render = load("render-release-notes.py")

REPO = "owner/name"
TAG = "v0.4.3"
VERSION = "0.4.3"

INSTALLERS = [
    f"Aerini-{VERSION}-1.aarch64.rpm",
    f"Aerini-{VERSION}-1.x86_64.rpm",
    f"Aerini_{VERSION}_aarch64.app.tar.gz",
    f"Aerini_{VERSION}_aarch64.AppImage",
    f"Aerini_{VERSION}_aarch64.dmg",
    f"Aerini_{VERSION}_amd64.AppImage",
    f"Aerini_{VERSION}_amd64.deb",
    f"Aerini_{VERSION}_arm64.deb",
    f"Aerini_{VERSION}_x64-setup.exe",
    f"Aerini_{VERSION}_x64_en-US.msi",
]
SIGNATURES = [name + ".sig" for name in INSTALLERS if not name.endswith(".dmg")]
MCP = [
    f"{program}_{VERSION}_{target}.{ext}{suffix}"
    for program in ("aerini-mcp", "aerini-mcp-offline")
    for target, ext in (
        ("aarch64-apple-darwin", "tar.gz"),
        ("aarch64-unknown-linux-gnu", "tar.gz"),
        ("x86_64-pc-windows-msvc", "zip"),
        ("x86_64-unknown-linux-gnu", "tar.gz"),
    )
    for suffix in ("", ".sha256")
]
ALL = INSTALLERS + SIGNATURES + MCP + ["latest.json"]
SERVER = [
    f"aerini-server_{VERSION}_x86_64-unknown-linux-musl.tar.gz",
    f"aerini-server_{VERSION}_x86_64-unknown-linux-musl.tar.gz.sha256",
]


def sections(block):
    """Map each '### ' heading to the text under it, ignoring the collapsed update files."""
    parts = block.split("\n### ")[1:]
    return {part.split("\n", 1)[0]: part for part in parts}


class RenderTest(unittest.TestCase):
    def test_every_asset_is_linked_exactly_once(self):
        block = render.render(TAG, REPO, ALL)
        for name in ALL:
            target = f"/releases/download/{TAG}/{quote(name, safe='')})"
            self.assertEqual(block.count(target), 1, name)

    def test_installers_sit_under_their_own_system_and_nowhere_else(self):
        groups = sections(render.render(TAG, REPO, ALL))
        expected = {
            "Windows": ("_x64-setup.exe", "_x64_en-US.msi"),
            "macOS": ("_aarch64.dmg",),
            "Linux": (".AppImage", ".deb", ".rpm"),
        }
        for system, endings in expected.items():
            for name in INSTALLERS:
                belongs = name.endswith(endings)
                self.assertEqual(f"/{name})" in groups[system], belongs, f"{name} in {system}")

    def test_one_format_per_system_is_recommended_for_every_architecture(self):
        groups = sections(render.render(TAG, REPO, ALL))
        recommended = {
            system: {
                line.split("`")[1]
                for line in text.splitlines()
                if "**Recommended.**" in line
            }
            for system, text in groups.items()
            if system in ("Windows", "macOS", "Linux")
        }
        self.assertEqual(
            recommended,
            {
                "Windows": {f"Aerini_{VERSION}_x64-setup.exe"},
                "macOS": {f"Aerini_{VERSION}_aarch64.dmg"},
                "Linux": {f"Aerini_{VERSION}_aarch64.AppImage", f"Aerini_{VERSION}_amd64.AppImage"},
            },
        )

    def test_mac_app_bundle_moves_to_updater_files_only_when_a_disk_image_exists(self):
        bundle = f"Aerini_{VERSION}_aarch64.app.tar.gz"
        folded = render.render(TAG, REPO, [bundle, f"Aerini_{VERSION}_aarch64.dmg"])
        self.assertNotIn("### macOS\n\n| Download | Architecture | Notes |\n|---|---|---|\n| [`" + bundle, folded)
        self.assertEqual(folded.count(f"/{bundle})"), 1)
        self.assertGreater(folded.index(f"/{bundle})"), folded.index("<summary>Auto-update files"))

        alone = render.render(TAG, REPO, [bundle])
        self.assertLess(alone.index("### macOS"), alone.index(f"/{bundle})"))
        self.assertNotIn("Auto-update files", alone)

    def test_server_download_has_its_own_section_with_its_checksum(self):
        block = render.render(TAG, REPO, ALL + SERVER)
        for name in SERVER:
            self.assertEqual(block.count(f"/releases/download/{TAG}/{quote(name, safe='')})"), 1, name)
        server = sections(block)["Server (`aerini-server`)"]
        self.assertIn("Linux x64", server)
        self.assertIn(".sha256", server)
        self.assertNotIn("### Other files", block)

    def test_server_checksum_without_its_archive_is_listed_as_other(self):
        block = render.render(TAG, REPO, [SERVER[1]])
        self.assertNotIn("### Server", block)
        self.assertIn(SERVER[1], sections(block)["Other files"])

    def test_mcp_table_is_collapsed_under_its_heading(self):
        mcp_section = sections(render.render(TAG, REPO, ALL))["MCP server (`aerini-mcp`)"]
        self.assertLess(mcp_section.index("<details>"), mcp_section.index("| Platform | Online | Offline |"))
        self.assertLess(mcp_section.index("| Linux ARM64 |"), mcp_section.index("</details>"))

    def test_mcp_programs_land_in_the_row_and_column_for_their_platform(self):
        block = render.render(TAG, REPO, ALL)
        rows = {line.split(" | ")[0].lstrip("| "): line for line in block.splitlines() if "[.tar.gz]" in line or "[.zip]" in line}
        self.assertEqual(
            set(rows), {"Windows x64", "macOS Apple Silicon", "Linux x64", "Linux ARM64"}
        )
        online, offline = rows["Windows x64"].split(" | ")[1:3]
        self.assertIn(f"aerini-mcp_{VERSION}_x86_64-pc-windows-msvc.zip)", online)
        self.assertIn(f"aerini-mcp-offline_{VERSION}_x86_64-pc-windows-msvc.zip)", offline)
        self.assertIn(".zip.sha256)", online)

    def test_prerelease_version_does_not_confuse_the_mcp_split(self):
        names = [
            "aerini-mcp-offline_1.2.3-rc.1_x86_64-unknown-linux-gnu.tar.gz",
            "aerini-mcp-offline_1.2.3-rc.1_x86_64-unknown-linux-gnu.tar.gz.sha256",
        ]
        block = render.render("v1.2.3-rc.1", REPO, names)
        self.assertIn("| Linux x64 | - | [.tar.gz](", block)
        self.assertNotIn("Other files", block)

    def test_unrecognised_and_orphaned_assets_are_listed_not_dropped(self):
        names = [
            "notes.txt",
            "Aerini_0.4.3_setup.exe",
            "aerini-mcp_0.4.3_x86_64-unknown-freebsd.tar.gz",
            "aerini-mcp_0.4.3_x86_64-unknown-linux-gnu.tar.gz.sha256",
        ]
        block = render.render(TAG, REPO, names)
        other = sections(block)["Other files"]
        for name in names:
            self.assertIn(f"/{quote(name, safe='')})", other, name)
        self.assertNotIn("### Windows", block)
        self.assertNotIn("MCP server", block)

    def test_bad_tag_or_repo_is_refused(self):
        for tag, repo in (("0.4.3", REPO), (TAG, "nope")):
            with self.assertRaises(render.RenderError):
                render.render(tag, repo, ALL)


class MergeTest(unittest.TestCase):
    def test_existing_text_is_kept_and_a_second_run_changes_nothing(self):
        intro = "See the changelog.\n\n**macOS note:** unsigned.\n"
        first = render.merge(intro, render.render(TAG, REPO, ALL))
        self.assertTrue(first.startswith(intro.rstrip() + "\n\n" + render.START))
        again = render.merge(first, render.render(TAG, REPO, ALL))
        self.assertEqual(first, again)

    def test_an_old_list_is_replaced_and_text_after_it_survives(self):
        old = render.merge("Intro\n", render.render(TAG, REPO, ["latest.json"])) + "\nHand-written footer\n"
        new = render.merge(old, render.render(TAG, REPO, ALL))
        self.assertEqual(new.count(render.START), 1)
        self.assertIn("### Windows", new)
        self.assertTrue(new.startswith("Intro\n"))
        self.assertTrue(new.endswith("\nHand-written footer\n"))

    def test_windows_line_endings_are_normalised_and_broken_markers_are_refused(self):
        self.assertNotIn("\r", render.merge("a\r\nb\r\n", render.render(TAG, REPO, ALL)))
        for body in (render.START, render.END, render.END + render.START, render.START * 2 + render.END * 2):
            with self.assertRaises(render.RenderError, msg=body):
                render.merge(body, "x")


class CommandLineTest(unittest.TestCase):
    def test_writes_the_merged_notes_and_reports_bad_input_without_writing(self):
        with tempfile.TemporaryDirectory() as tmp:
            assets = Path(tmp) / "assets.txt"
            assets.write_text("\n".join(ALL) + "\n", encoding="utf-8")
            body = Path(tmp) / "body.md"
            body.write_text("Intro\n", encoding="utf-8")
            out = Path(tmp) / "out.md"
            args = ["--tag", TAG, "--repo", REPO, "--assets", str(assets), "--body", str(body), "--out", str(out)]
            self.assertEqual(render.main(args), 0)
            self.assertTrue(out.read_text(encoding="utf-8").startswith("Intro\n\n" + render.START))

            bad = Path(tmp) / "bad.md"
            self.assertEqual(render.main(["--tag", "nope", *args[2:6], "--out", str(bad)]), 1)
            self.assertFalse(bad.exists())


if __name__ == "__main__":
    unittest.main()
