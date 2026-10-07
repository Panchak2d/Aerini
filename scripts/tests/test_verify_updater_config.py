import base64
import copy
import json
import unittest
from pathlib import Path

from helpers import SCRIPTS, load

mod = load("verify-updater-config.py")
REPO = "Panchak2d/aerini"
CONF = json.loads((SCRIPTS.parent / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))


def pubkey(raw=b"Ed" + bytes(40), comment="untrusted comment: minisign public key: AB"):
    key_line = base64.b64encode(raw).decode()
    return base64.b64encode(f"{comment}\n{key_line}\n".encode()).decode()


class VerifyConfigTests(unittest.TestCase):
    def conf(self):
        return copy.deepcopy(CONF)

    def test_committed_config_passes(self):
        self.assertEqual(mod.problems(CONF, REPO), [])

    def test_repo_comparison_ignores_case(self):
        self.assertEqual(mod.problems(CONF, "panchak2d/AERINI"), [])

    def test_flag_off_fails(self):
        conf = self.conf()
        conf["bundle"]["createUpdaterArtifacts"] = False
        self.assertIn("bundle.createUpdaterArtifacts must be true", mod.problems(conf, REPO))

    def test_empty_pubkey_fails(self):
        conf = self.conf()
        conf["plugins"]["updater"]["pubkey"] = ""
        self.assertIn("plugins.updater.pubkey is empty", mod.problems(conf, REPO))

    def test_malformed_pubkeys_fail(self):
        for bad in ("not base64!", pubkey(raw=b"Xx" + bytes(40)), pubkey(comment="hello"),
                    base64.b64encode(b"untrusted comment: x\n").decode()):
            conf = self.conf()
            conf["plugins"]["updater"]["pubkey"] = bad
            self.assertTrue(mod.problems(conf, REPO), bad)

    def test_wrong_endpoint_fails(self):
        conf = self.conf()
        conf["plugins"]["updater"]["endpoints"] = ["https://github.com/other/repo/releases/latest/download/latest.json"]
        self.assertTrue(any("first plugins.updater endpoint" in p for p in mod.problems(conf, REPO)))

    def test_http_endpoint_fails(self):
        conf = self.conf()
        conf["plugins"]["updater"]["endpoints"] = ["http://github.com/x"]
        self.assertTrue(any("https" in p for p in mod.problems(conf, REPO)))

    def test_missing_require_signed_version_fails(self):
        conf = self.conf()
        del conf["plugins"]["updater"]["requireSignedVersion"]
        self.assertIn("plugins.updater.requireSignedVersion must be true", mod.problems(conf, REPO))


if __name__ == "__main__":
    unittest.main()
