"""Distribution checks must fail before starting a broken package."""

import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from package_smoke import check_project_license, check_third_party_records


class DistributionTest(unittest.TestCase):
    def test_project_license_must_be_present_and_match(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with self.assertRaisesRegex(RuntimeError, "LICENSE"):
                check_project_license(directory)
            license_path = directory / "LICENSE"
            license_path.write_text("Wrong license", encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "LICENSE"):
                check_project_license(directory)
            license_path.write_bytes((ROOT / "LICENSE").read_bytes())
            check_project_license(directory)
            # An archive made from a Windows checkout has equivalent CRLF text.
            license_path.write_bytes((ROOT / "LICENSE").read_text(encoding="utf-8").replace("\n", "\r\n").encode())
            check_project_license(directory)

    def test_third_party_records_verify_content_and_source_hashes(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            third_party = directory / "third-party"
            (third_party / "licenses").mkdir(parents=True)
            (third_party / "sources").mkdir()
            license_content = b"license text"
            source_content = b"source archive"
            license_hash = hashlib.sha256(license_content).hexdigest()
            source_hash = hashlib.sha256(source_content).hexdigest()
            (third_party / "licenses" / f"{license_hash}.txt").write_bytes(license_content)
            (third_party / "sources" / "covered-1.0.0.crate").write_bytes(source_content)
            lock_hash = "a" * 64
            inventory = {
                "schemaVersion": 1,
                "rootPackage": "tdmcp-daemon",
                "rootVersion": "1.0.0",
                "rootSourceUrl": "https://example.test/tdmcp/archive/v1.0.0.tar.gz",
                "cargoLockSha256": lock_hash,
                "packages": [
                    {
                        "name": "covered",
                        "version": "1.0.0",
                        "source": "registry+https://github.com/rust-lang/crates.io-index",
                        "cargoLockChecksum": source_hash,
                        "licenseFiles": [
                            {
                                "packagedPath": f"licenses/{license_hash}.txt",
                                "sha256": license_hash,
                            }
                        ],
                        "bundledSource": {
                            "path": "sources/covered-1.0.0.crate",
                            "sha256": source_hash,
                            "modifiedByTdmcp": False,
                            "reason": "test covered source",
                        },
                    }
                ],
            }
            (third_party / "inventory.json").write_text(json.dumps(inventory), encoding="utf-8")
            (directory / "THIRD_PARTY_NOTICES.txt").write_text(lock_hash, encoding="utf-8")
            (directory / "SOURCE_OFFER.md").write_text(lock_hash, encoding="utf-8")
            check_third_party_records(directory)

            (third_party / "licenses" / f"{license_hash}.txt").write_text("changed", encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "license text mismatch"):
                check_third_party_records(directory)


if __name__ == "__main__":
    unittest.main()
