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


class ResumePublication(unittest.TestCase):
    def test_rerun_reuses_identical_assets_and_refuses_changed_ones(self):
        from contextlib import chdir
        import shutil
        import subprocess

        target, version, sha = RELEASE.TARGETS[0], "0.3.0", "a" * 40
        for changed in (False, True):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as root:
                root = Path(root)
                dist, old = root / "dist", root / "previous"
                dist.mkdir()
                old.mkdir()
                names = RELEASE.expected_files(version, target)
                for name in names:
                    (old / name).write_bytes(b"original archive")
                manifest = {"sha": sha, "version": version, "target": target,
                            "files": {name: RELEASE.digest(old / name) for name in names}}
                manifest_name = f"qualified-{target}.json"
                (old / manifest_name).write_text(json.dumps(manifest))
                for path in old.iterdir():
                    shutil.copy(path, dist / path.name)
                if changed:
                    name = next(iter(names))
                    (dist / name).write_bytes(b"different archive at the same commit")
                    manifest["files"][name] = RELEASE.digest(dist / name)
                    (dist / manifest_name).write_text(json.dumps(manifest))
                calls = []

                def command(*args, **kwargs):
                    calls.append(args)
                    output = ""
                    if args[:2] == ("gh", "api"):
                        output = json.dumps({"object": {"sha": sha, "type": "commit"}})
                    elif args[:3] == ("gh", "release", "view"):
                        output = json.dumps({"isDraft": False, "assets": [
                            {"name": path.name} for path in old.iterdir()]})
                    elif args[:3] == ("gh", "release", "download"):
                        dest = Path(args[args.index("--dir") + 1])
                        for path in old.iterdir():
                            shutil.copy(path, dest / path.name)
                    return subprocess.CompletedProcess(args, 0, output, "")

                with chdir(root), mock.patch.object(RELEASE, "run", side_effect=command), \
                     mock.patch.object(RELEASE, "report_failures"), \
                     mock.patch.dict(RELEASE.os.environ, {"RELEASE_SHA": sha,
                         "RELEASE_VERSION": version, "RELEASE_MODE": "publish",
                         "GH_REPO": "example/repo", "GITHUB_RUN_ID": "123"}):
                    if changed:
                        with self.assertRaisesRegex(ValueError, "refusing to replace"):
                            RELEASE.publish()
                        self.assertFalse(any(call[:3] == ("gh", "release", "upload") for call in calls))
                    else:
                        RELEASE.publish()
                        uploads = [call for call in calls if call[:3] == ("gh", "release", "upload")]
                        self.assertEqual(len(uploads), 1)
                        self.assertEqual(uploads[0][4], "dist/SHA256SUMS.txt")
                        self.assertIn(next(iter(names)), (dist / "SHA256SUMS.txt").read_text())
                    self.assertFalse(any(call[:3] == ("gh", "release", "create") for call in calls))
