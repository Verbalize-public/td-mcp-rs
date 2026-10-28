"""Release admission must fail closed without contacting GitHub."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("release", Path(__file__).parents[1] / "release.py")
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)


class Qualification(unittest.TestCase):
    def test_only_complete_matching_platforms_are_admitted(self):
        for target in RELEASE.TARGETS:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as root:
                root = Path(root)
                files = RELEASE.expected_files("0.3.0", target)
                for name in files:
                    (root / name).write_bytes(b"validated package")
                manifest = {"sha": "a" * 40, "version": "0.3.0", "target": target,
                            "files": {name: RELEASE.digest(root / name) for name in files}}
                path = root / f"qualified-{target}.json"
                path.write_text(json.dumps(manifest))
                assets, qualified = RELEASE.validated_assets(root, "a" * 40, "0.3.0")
                self.assertEqual(qualified, {target})
                self.assertEqual(set(assets), files | {path.name})
                with self.assertRaises(ValueError):
                    RELEASE.validated_assets(root, "b" * 40, "0.3.0")
                (root / next(iter(files))).write_bytes(b"tampered")
                with self.assertRaises(ValueError):
                    RELEASE.validated_assets(root, "a" * 40, "0.3.0")

    def test_unqualified_files_never_enter_checksums(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            (root / "unvalidated.zip").write_bytes(b"partial")
            self.assertEqual(RELEASE.validated_assets(root, "a" * 40, "0.3.0"), ({}, set()))

    def test_missing_installer_or_path_traversal_is_refused(self):
        target = "x86_64-pc-windows-msvc"
        for files in [{"tdmcp-rs-0.3.0-x86_64-pc-windows-msvc.zip": "abc"}, {"../other": "abc"}]:
            with tempfile.TemporaryDirectory() as root:
                root = Path(root)
                (root / f"qualified-{target}.json").write_text(json.dumps({
                    "sha": "a", "version": "0.3.0", "target": target, "files": files}))
                with self.assertRaises(ValueError):
                    RELEASE.validated_assets(root, "a", "0.3.0")

    def test_manual_commit_mismatch_is_refused(self):
        with mock.patch.dict(RELEASE.os.environ, {"GITHUB_EVENT_NAME": "workflow_dispatch",
                                                  "REQUESTED_COMMIT": "other"}), \
             mock.patch.object(RELEASE, "run", return_value=mock.Mock(stdout="a" * 40)):
            with self.assertRaisesRegex(ValueError, "full SHA"):
                RELEASE.prepare()

    def test_existing_tag_is_never_moved(self):
        with mock.patch.dict(RELEASE.os.environ, {"GITHUB_EVENT_NAME": "push",
                "GITHUB_REF_NAME": "v0.3.0", "REQUESTED_COMMIT": ""}), \
             mock.patch.object(RELEASE, "run", side_effect=[mock.Mock(stdout="a" * 40),
                 mock.Mock(returncode=0, stdout="b" * 40)]):
            with self.assertRaisesRegex(ValueError, "another commit"):
                RELEASE.prepare()
