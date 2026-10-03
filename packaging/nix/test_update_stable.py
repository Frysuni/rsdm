import base64
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "update_stable", Path(__file__).with_name("update-stable.py")
)
updater = importlib.util.module_from_spec(spec)
spec.loader.exec_module(updater)


class StableReleaseTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.manifest = self.root / "stable.json"
        self.original = '{"version": "1.1.0", "hashes": {}}\n'
        self.manifest.write_text(self.original)

    def archive(self, arch):
        path = self.root / f"rsdm-2.0.0-{arch}-unknown-linux-gnu.tar.gz"
        path.write_bytes(arch.encode())
        return "sha256-" + base64.b64encode(hashlib.sha256(arch.encode()).digest()).decode()

    def test_publishes_both_hashes_and_can_be_repeated(self):
        expected = {
            f"{arch}-linux": self.archive(arch)
            for arch in ("x86_64", "aarch64")
        }
        updater.update_stable("v2.0.0", self.root, self.manifest)
        self.assertEqual(json.loads(self.manifest.read_text()), {
            "version": "2.0.0", "hashes": expected,
        })
        updater.update_stable("v2.0.0", self.root, self.manifest)

    def test_missing_architecture_does_not_modify_manifest(self):
        self.archive("x86_64")
        with self.assertRaises(FileNotFoundError):
            updater.update_stable("v2.0.0", self.root, self.manifest)
        self.assertEqual(self.manifest.read_text(), self.original)

    def test_rejects_prereleases_invalid_tags_and_downgrades(self):
        for tag in ("v2.0.0-rc.1", "2.0.0", "v02.0.0", "v1.0.0"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                updater.update_stable(tag, self.root, self.manifest)
        self.assertEqual(self.manifest.read_text(), self.original)

    def test_existing_version_cannot_change_hashes(self):
        for arch in ("x86_64", "aarch64"):
            self.archive(arch)
        updater.update_stable("v2.0.0", self.root, self.manifest)
        published = self.manifest.read_text()
        (self.root / "rsdm-2.0.0-x86_64-unknown-linux-gnu.tar.gz").write_bytes(b"changed")
        with self.assertRaises(ValueError):
            updater.update_stable("v2.0.0", self.root, self.manifest)
        self.assertEqual(self.manifest.read_text(), published)


if __name__ == "__main__":
    unittest.main()
