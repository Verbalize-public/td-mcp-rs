"""Distribution checks must fail before starting a broken package."""

from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from package_smoke import check_project_license


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


if __name__ == "__main__":
    unittest.main()
