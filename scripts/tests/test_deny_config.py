import tomllib
import unittest

from helpers import SCRIPTS

ROOT = SCRIPTS.parent


def ignored_ids(path):
    with open(path, "rb") as handle:
        entries = tomllib.load(handle)["advisories"]["ignore"]
    return {entry if isinstance(entry, str) else entry["id"] for entry in entries}


class DenyConfigTest(unittest.TestCase):
    def test_advisory_ignores_match_cargo_audit(self):
        self.assertEqual(
            ignored_ids(ROOT / "deny.toml"),
            ignored_ids(ROOT / ".cargo" / "audit.toml"),
        )


if __name__ == "__main__":
    unittest.main()
