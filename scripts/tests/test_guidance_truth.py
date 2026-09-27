"""Current-tool semantic regressions for guidance; fake TD, no IPC or live project."""
import importlib
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bridge"))
execute = importlib.import_module("tdmcp_bridge.execute")
inspect_bridge = importlib.import_module("tdmcp_bridge.inspect")
mutate = importlib.import_module("tdmcp_bridge.mutate")


def fake_node():
    return SimpleNamespace(
        valid=True, path="/owned/control", name="control", family="COMP",
        opType="baseCOMP", comment="", children=[], inputs=[], outputs=[],
        errors=lambda: "", warnings=lambda: "",
        pars=MagicMock(side_effect=AssertionError("unexpected parameter enumeration")),
    )


class GuidanceTruthTests(unittest.TestCase):
    def test_execution_namespace_and_explicit_import(self):
        td = SimpleNamespace(op=lambda path: path, root="root-alias")
        with patch.dict(sys.modules, {"td": td}):
            missing = execute.handle_execute_python({
                "script": "result = math.sqrt(4)", "includeLogs": False,
            })
            good = execute.handle_execute_python({
                "script": "import math\nresult = [math.sqrt(4), op('/owned'), root]",
                "includeLogs": False,
            })
            names = execute.handle_execute_python({
                "script": "result = [n for n in ('math','json','sys','me','parent','noiseTOP') if n in globals()]",
                "includeLogs": False,
            })
        self.assertFalse(missing["ok"])
        self.assertEqual(missing["exception"]["type"], "NameError")
        self.assertTrue(good["ok"])
        self.assertEqual(good["result"], [2.0, "/owned", "root-alias"])
        self.assertEqual(names["result"], [])

    def test_native_sample_fixture_and_documented_keywords(self):
        def sample(*, x=None, y=None, u=None, v=None):
            self.assertIn((x, y, u, v), ((0, 0, None, None), (None, None, 0.5, 0.5)))
            return (1.0, 1.0, 1.0, 1.0)

        node = SimpleNamespace(path="/project1/guidance_probe/pixels", width=256,
                               height=256, sample=sample)
        source = (ROOT / "scripts/fixtures/guidance/top_sample.py").read_text()
        with patch.dict(sys.modules, {"td": SimpleNamespace(op=lambda path: node)}):
            observed = execute.handle_execute_python({"script": source, "includeLogs": False})
        self.assertTrue(observed["ok"])
        self.assertTrue(observed["result"]["legacyPositional"]["rejected"])
        self.assertEqual(observed["result"]["pixel"], observed["result"]["uv"])
        text = (ROOT / "skills/templates/touchdesigner/reference/python-api.jinja.md").read_text()
        self.assertIn("t.sample(x=0, y=0)", text)
        self.assertIn("t.sample(u=0.5, v=0.5)", text)
        self.assertNotIn("t.sample(x, y, u='pixel')", text)

    def test_layout_is_neither_parameter_nor_supported_flag(self):
        node = SimpleNamespace(path="/owned/n", opType="noiseTOP", par=SimpleNamespace(),
                               nodeX=0, nodeY=0, pars=lambda: [])
        for apply, values, code in (
            (mutate._apply_values, {"nodeX": 100}, "tdmcp.par.unknown"),
            (mutate._apply_flags, {"nodeY": 100}, "tdmcp.flag.unknown"),
        ):
            with self.subTest(code=code):
                self.assertEqual(apply(node, values)["code"], code)
                self.assertEqual((node.nodeX, node.nodeY), (0, 0))
        # This only proves rejected assignments; it is not a batch rollback claim.

    def inspect(self, node, **options):
        with patch.dict(sys.modules, {"td": SimpleNamespace()}), patch.object(
            inspect_bridge, "resolve_op", return_value=node
        ):
            return inspect_bridge.handle_inspect({"paths": [node.path], **options})["nodes"][0]

    def test_default_inspect_requests_wires_messages_not_parameters(self):
        node = fake_node()
        observed = self.inspect(node)
        self.assertTrue(observed["ok"])
        self.assertNotIn("params", observed)
        self.assertEqual(observed["inputs"], [])
        self.assertTrue(observed["observations"]["inputs"]["available"])
        self.assertEqual(observed["errors"], [])
        self.assertEqual(observed["warnings"], [])
        node.pars.assert_not_called()

    def test_parameter_allowlist_does_not_include_wires(self):
        node = fake_node()
        node.pars = lambda: []
        observed = self.inspect(node, include=["params", "errors"])
        self.assertNotIn("inputs", observed)
        self.assertNotIn("children", observed)
        self.assertNotIn("warnings", observed)
        self.assertEqual(observed["params"], [])

    def test_names_only_never_reads_values_or_modes(self):
        class NameOnly:
            name = "Gain"

            def __getattr__(self, name):
                raise AssertionError(f"names-only read {name}")

        node = fake_node()
        node.pars = lambda: [NameOnly()]
        observed = self.inspect(node, include=["params"], paramsMode="names")
        self.assertEqual(observed["params"], [{"name": "Gain"}])
        self.assertNotIn("inputs", observed)

    def test_unavailable_empty_wire_fallback_is_not_clean(self):
        class UnreadableInputs:
            def __init__(self):
                self.__dict__.update(vars(fake_node()))
                del self.__dict__["inputs"]

            @property
            def inputs(self):
                raise RuntimeError("synthetic unavailable wiring")

        observed = self.inspect(UnreadableInputs())
        self.assertEqual(observed["inputs"], [])
        self.assertFalse(observed["observations"]["inputs"]["available"])


if __name__ == "__main__":
    unittest.main()
