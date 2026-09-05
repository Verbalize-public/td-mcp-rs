"""Exercise packaging filesystem behavior, not native signing correctness."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
DMG_NAME = "tdmcp-rs-0.1.4-aarch64-apple-darwin.dmg"
FAKE_TOOL = '''#!/usr/bin/env python3
import os
from pathlib import Path
import sys

name = Path(sys.argv[0]).name
failure = os.environ.get("FAKE_FAIL_AT", "")
if name == failure or (name == "mv" and failure == "publish-dmg" and sys.argv[1] == "-f"):
    sys.exit(23)
if name == "mv":
    os.execv(os.environ["REAL_MV"], ["mv", *sys.argv[1:]])
elif name == "sips":
    Path(sys.argv[sys.argv.index("--out") + 1]).write_bytes(b"icon")
elif name == "iconutil":
    Path(sys.argv[sys.argv.index("-o") + 1]).write_bytes(b"icns")
elif name == "hdiutil" and sys.argv[1] == "create":
    Path(sys.argv[-1]).write_bytes(b"new disk image")
elif name == "hdiutil" and sys.argv[1] == "verify":
    assert Path(sys.argv[-1]).is_file()
'''


@unittest.skipIf(os.name == "nt", "macOS shell packaging tests require a Unix shell")
class MacPackagingTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="tdmcp-macos-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.output = self.root / "output with spaces"
        self.output.mkdir()
        self.app = self.output / "tdmcp.app"
        self.app.mkdir()
        (self.app / "previous").write_text("previous bundle")
        self.dmg = self.output / DMG_NAME
        self.dmg.write_text("previous dmg")
        self.binary = self.root / "daemon"
        self.binary.write_text("new binary")
        fake_bin = self.root / "tools"
        fake_bin.mkdir()
        for name in ["sips", "iconutil", "codesign", "hdiutil", "xcrun", "mv"]:
            tool = fake_bin / name
            tool.write_text(FAKE_TOOL)
            tool.chmod(0o755)
        self.env = {key: value for key, value in os.environ.items() if not key.startswith("APPLE_")}
        self.env.update(PATH=str(fake_bin) + os.pathsep + os.environ["PATH"], REAL_MV=shutil.which("mv"))

    def package(self, failure="", version="v0.1.4"):
        env = {**self.env, "FAKE_FAIL_AT": failure}
        if failure == "xcrun":
            env.update(APPLE_DEVELOPER_ID_IDENTITY="fixture", APPLE_NOTARY_PROFILE="fixture")
        return subprocess.run(
            ["bash", str(ROOT / "packaging/macos/make_app.sh"), str(self.binary),
             version, "aarch64-apple-darwin", str(self.output)],
            cwd=ROOT, env=env, capture_output=True, text=True, timeout=15,
        )

    def test_native_tool_failures_preserve_previous_artifacts(self):
        for tool in ["sips", "iconutil", "codesign", "hdiutil", "xcrun", "publish-dmg"]:
            with self.subTest(tool=tool):
                result = self.package(tool)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual((self.app / "previous").read_text(), "previous bundle")
                self.assertEqual(self.dmg.read_text(), "previous dmg")
                self.assertEqual(list(self.output.glob(".tdmcp-macos.*")), [])

    def test_success_replaces_artifacts_and_cleans_all_staging(self):
        result = self.package()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((self.app / "Contents/MacOS/tdmcp-daemon").read_text(), "new binary")
        self.assertEqual((self.app / "Contents/Resources/LICENSE").read_bytes(), (ROOT / "LICENSE").read_bytes())
        self.assertEqual(self.dmg.read_text(), "new disk image")
        self.assertFalse((self.app / "previous").exists())
        self.assertEqual(list(self.output.glob(".tdmcp-macos.*")), [])

    def test_invalid_version_is_rejected_without_touching_output(self):
        result = self.package(version="../not-a-version")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.dmg.read_text(), "previous dmg")
        self.assertEqual(list(self.output.glob(".tdmcp-macos.*")), [])


if __name__ == "__main__":
    unittest.main()
