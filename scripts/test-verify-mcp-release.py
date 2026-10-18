#!/usr/bin/env python3
"""Regression checks for false-positive release proof and interrupted commands."""

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("release", Path(__file__).with_name("verify-mcp-release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseGateTests(unittest.TestCase):
    def execute(self, code):
        with tempfile.TemporaryDirectory() as directory:
            return release.execute(release.check("fixture", [sys.executable, "-c", code]), Path(directory), 5)

    def test_successful_process_with_skipped_proof_is_not_passed(self):
        result = self.execute("print('SKIP: required tool absent')")
        self.assertEqual(result["status"], "blocked")
        self.assertEqual(result["exit_code"], 0)

    def test_failed_process_stays_failed_even_if_it_prints_a_pass(self):
        result = self.execute("print('PASS fixture'); raise SystemExit(1)")
        self.assertEqual(result["status"], "failed")

    def test_missing_binary_is_blocked_instead_of_passed(self):
        with tempfile.TemporaryDirectory() as directory:
            result = release.execute(release.check("fixture", ["commoncal-nonexistent-proof-tool"]), Path(directory), 5)
        self.assertEqual(result["status"], "blocked")

    def test_manual_release_proof_cannot_be_satisfied_by_automated_checks(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "proof"
            with patch.object(release, "checks", return_value=[]), patch.object(sys, "argv", [
                "verify-mcp-release.py", "--with-image", "--live-discovery", "--output-dir", str(output),
            ]):
                self.assertEqual(release.main(), 2)
            report = json.loads((output / "report.json").read_text())
            self.assertEqual(report["status"], "blocked")
            self.assertEqual(report["counts"], {"blocked": 4})
            self.assertFalse(report["deployment_performed"])


if __name__ == "__main__":
    unittest.main()
