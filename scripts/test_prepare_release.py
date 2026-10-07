import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import unittest

from prepare_release import PACKAGES, prepare


class ReleaseAssetsTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.artifacts = self.root / "artifacts"
        self.output = self.root / "release"
        for target, suffixes in PACKAGES.items():
            folder = self.artifacts / f"craftlauncher-{target}"
            folder.mkdir(parents=True)
            stem = f"craftlauncher-0.1.0-{target}"
            sums = []
            for suffix in suffixes:
                file = folder / (stem + suffix)
                file.write_bytes(f"native fixture {target} {suffix}".encode())
                sums.append(f"{hashlib.sha256(file.read_bytes()).hexdigest()}  {file.name}\n")
            (folder / f"{stem}-SHA256SUMS.txt").write_text("".join(sums))
            archive = folder / (stem + (".zip" if target.startswith("windows") else ".tar.gz"))
            targets = ["macos-x86_64", "macos-aarch64"] if target == "macos-universal" else [target]
            for native in targets:
                manifest = dict(schema=1, version="0.1.0", target=native, channel="stable",
                                archive=archive.name, bytes=archive.stat().st_size,
                                sha256=hashlib.sha256(archive.read_bytes()).hexdigest())
                (folder / f"manifest-{native}-stable.json").write_text(json.dumps(manifest))

    def check_rejected(self):
        with self.assertRaises(ValueError):
            prepare(self.artifacts, self.output, "0.1.0")
        self.assertFalse(self.output.exists(), "Invalid artifacts must never reach publication staging")

    def test_complete_artifacts_are_copied_without_modification(self):
        names = prepare(self.artifacts, self.output, "0.1.0")
        self.assertEqual(len(names), 30)
        for source in self.artifacts.glob("*/*"):
            self.assertEqual(source.read_bytes(), (self.output / source.name).read_bytes())

    def test_missing_platform_is_rejected(self):
        shutil.rmtree(self.artifacts / "craftlauncher-freebsd-x86_64")
        self.check_rejected()

    def test_tampered_package_is_rejected(self):
        next(self.artifacts.glob("*/*.zip")).write_bytes(b"tampered")
        self.check_rejected()

    def test_escaping_checksum_path_is_rejected(self):
        next(self.artifacts.glob("*/*SHA256SUMS.txt")).write_text("a" * 64 + "  ../outside\n")
        self.check_rejected()

    def test_manifest_for_wrong_version_is_rejected(self):
        path = next(self.artifacts.glob("*/manifest*.json"))
        manifest = json.loads(path.read_text())
        manifest["version"] = "99.0.0"
        path.write_text(json.dumps(manifest))
        self.check_rejected()

    def test_unexpected_asset_is_rejected(self):
        next(self.artifacts.iterdir()).joinpath("unexpected.exe").write_bytes(b"unexpected")
        self.check_rejected()


if __name__ == "__main__":
    unittest.main()
