"""Regression tests for source-managed Kitchen dependency protection.

Run with the standard library unittest runner. No network, pip install, or CUDA
compiler is required; the protected pip arguments are inspected before execution.
"""
import contextlib
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src-tauri" / "src"))
import managed_kitchen_pip as guard


class ManagedKitchenPipTests(unittest.TestCase):
    def test_flat_core_requirements_keep_other_dependencies_and_original(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "requirements.txt"
            contents = (
                "torch\ncomfy-kitchen==0.2.40\n"
                "comfy_kitchen[torch]>=0.2.30 ; python_version >= '3.10'\n"
                "some-comfy-kitchen-addon==1\n"
                "requests>=2\n"
            )
            source.write_text(contents)
            with contextlib.ExitStack() as stack:
                args = guard.prepare_install_args(["install", "-r", str(source)], "0.2.40", stack)
                sanitized = Path(args[args.index("-r") + 1])
                text = sanitized.read_text()
                self.assertIn("torch\n", text)
                self.assertIn("requests>=2\n", text)
                self.assertIn("some-comfy-kitchen-addon==1\n", text)
                self.assertNotIn("comfy-kitchen==", text)
                self.assertNotIn("comfy_kitchen[", text)
                constraint = Path(args[-1])
                self.assertEqual(constraint.read_text(), "comfy-kitchen==0.2.40\n")
            self.assertFalse(sanitized.exists())
            self.assertFalse(constraint.exists())
            self.assertEqual(source.read_text(), contents)

    def test_nested_relative_includes_and_constraints(self):
        with tempfile.TemporaryDirectory(prefix="kitchen path with spaces ") as directory:
            root = Path(directory)
            nested = root / "nested"
            nested.mkdir()
            (nested / "extras.txt").write_text(
                "comfy-kitchen @ https://example.test/ck.whl\nother-lib==3\n"
            )
            (nested / "versions.txt").write_text("comfy-kitchen==0.1.0\nother-lib<4\n")
            source = root / "requirements.txt"
            source.write_text(
                "-r 'nested/extras.txt'\n-c nested/versions.txt\nscipy>=1\n"
            )
            with contextlib.ExitStack() as stack:
                args = guard.prepare_install_args(["install", "-r", str(source)], "0.2.40", stack)
                top = Path(args[args.index("-r") + 1]).read_text()
                self.assertNotIn("comfy-kitchen", top)
                self.assertIn("scipy>=1", top)
                include_path = guard._include_path(top.splitlines()[0].split(" ", 1)[1])
                constraints_path = guard._include_path(top.splitlines()[1].split(" ", 1)[1])
                self.assertEqual(Path(include_path).read_text(), "other-lib==3\n")
                self.assertEqual(Path(constraints_path).read_text(), "other-lib<4\n")
            self.assertEqual((nested / "extras.txt").read_text(),
                             "comfy-kitchen @ https://example.test/ck.whl\nother-lib==3\n")

    def test_multiline_kitchen_hash_is_fully_removed(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "requirements.txt"
            source.write_text(
                "comfy-kitchen==0.2.40 \\\n  --hash=sha256:deadbeef\n"
                "requests==2.32.0\n"
            )
            with contextlib.ExitStack() as stack:
                sanitized = guard.rewrite_requirements(source, stack)
                self.assertEqual(sanitized.read_text(), "requests==2.32.0\n")

    def test_pyproject_dependencies_remove_explicit_kitchen_without_network(self):
        with contextlib.ExitStack() as stack:
            args = guard.prepare_install_args(
                ["install", "comfy-kitchen>=0.2", "numpy>=2", "comfy_kitchen @ file:///tmp/ck.whl"],
                "0.2.40", stack
            )
            self.assertIn("numpy>=2", args)
            self.assertFalse(any(guard.is_kitchen_spec(a) for a in args[1:]))

    def test_source_only_pyproject_dependencies_are_noop(self):
        with contextlib.ExitStack() as stack:
            args = guard.prepare_install_args(
                ["install", "comfy-kitchen==0.2.40"], "0.2.40", stack
            )
            self.assertEqual(args, [])

    def test_fail_closed_on_unknown_source_decl_and_upgrade(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "requirements.txt"
            source.write_text("git+https://example.test/comfy-kitchen.git\n")
            with contextlib.ExitStack() as stack:
                with self.assertRaisesRegex(ValueError, "unrecognized"):
                    guard.prepare_install_args(["install", "-r", str(source)], "0.2.40", stack)
            with contextlib.ExitStack() as stack:
                with self.assertRaisesRegex(ValueError, "force-reinstall"):
                    guard.prepare_install_args(["install", "--force-reinstall", "numpy"], "0.2.40", stack)

    def test_cycle_rejected_and_all_temporary_copies_deleted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "requirements.txt"
            source.write_text("-r other.txt\n")
            (root / "other.txt").write_text("-r requirements.txt\n")
            with contextlib.ExitStack() as stack:
                with self.assertRaisesRegex(ValueError, "cyclic"):
                    guard.rewrite_requirements(source, stack)
            self.assertEqual(list(root.glob(".patcher-kitchen-*.txt")), [])


if __name__ == "__main__":
    unittest.main()
