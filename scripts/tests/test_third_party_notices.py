"""Third-party notice generation must follow release-only dependency edges."""

import importlib.util
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "generate_third_party_notices", ROOT / "scripts/generate_third_party_notices.py"
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load third-party notice generator")
NOTICES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NOTICES)


class ThirdPartyNoticeTest(unittest.TestCase):
    def test_cargo_tree_selection_excludes_unselected_and_workspace_packages(self):
        root = "path+file:///repo/daemon#tdmcp-daemon@1.0.0"
        internal = "path+file:///repo/core#tdmcp-core@1.0.0"
        normal = "registry+index#normal@1.0.0"
        build = "registry+index#build@1.0.0"
        dev = "registry+index#dev@1.0.0"
        packages = [
            {"id": root, "name": "tdmcp-daemon", "version": "1.0.0", "source": None},
            {"id": internal, "name": "tdmcp-core", "version": "1.0.0", "source": None},
            {"id": normal, "name": "normal", "version": "1.0.0", "source": "registry+index"},
            {"id": build, "name": "build", "version": "1.0.0", "source": "registry+index"},
            {"id": dev, "name": "dev", "version": "1.0.0", "source": "registry+index"},
        ]
        metadata = {
            "packages": packages,
            "resolve": {
                "nodes": [
                    {
                        "id": root,
                        "deps": [
                            {"pkg": internal, "dep_kinds": [{"kind": None}]},
                            {"pkg": dev, "dep_kinds": [{"kind": "dev"}]},
                        ],
                    },
                    {
                        "id": internal,
                        "deps": [
                            {"pkg": normal, "dep_kinds": [{"kind": None}]},
                            {"pkg": build, "dep_kinds": [{"kind": "build"}]},
                        ],
                    },
                    {"id": normal, "deps": []},
                    {"id": build, "deps": []},
                    {"id": dev, "deps": []},
                ]
            },
        }
        identities = {
            ("tdmcp-daemon", "1.0.0"),
            ("tdmcp-core", "1.0.0"),
            ("normal", "1.0.0"),
            ("build", "1.0.0"),
        }
        self.assertEqual(
            [
                (item["name"], item["version"])
                for item in NOTICES.select_release_packages(metadata, identities)
            ],
            [("build", "1.0.0"), ("normal", "1.0.0")],
        )

    def test_checked_in_canonical_texts_match_pinned_hashes(self):
        expected = {
            "Apache-2.0.txt": "074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff",
            "BSL-1.0.txt": "84c6ef3ea9e3254a54d0acf5d3e0c61ae011b8fef7dd6940591cf060e6380a8f",
            "CC0-1.0.txt": "a2010f343487d3f7618affe54f789f5487602331c0a8d03f49e9a7c547cf0499",
            "MIT.txt": "b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5",
        }
        self.assertEqual(
            {path.name: NOTICES.sha256_file(path) for path in NOTICES.CANONICAL_ROOT.glob("*.txt")},
            expected,
        )


if __name__ == "__main__":
    unittest.main()
