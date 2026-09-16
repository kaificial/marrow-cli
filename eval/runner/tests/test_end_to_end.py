import io
import json
import os
import shutil
import sys
import unittest

import run
from harness import class_metrics, run_engine, score_fixture, tally, validate_trace

RUNNER_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STUB = [sys.executable, os.path.join(RUNNER_DIR, "stub_engine.py")]
ORACLE = [sys.executable, os.path.join(RUNNER_DIR, "tests", "oracle_engine.py")]
RENAME_RUST = os.path.join(os.path.dirname(RUNNER_DIR), "fixtures", "rename-identifier", "rust")


def invoke(*argv):
    out = io.StringIO()
    code = run.main(list(argv), out=out)
    return code, out.getvalue()


class EndToEndTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not os.path.isdir(os.path.join(RENAME_RUST, "repo", ".git")):
            raise AssertionError("fixture repos are missing; run `just fixtures` first")

    def test_gate_passes_a_perfect_engine(self):
        code, output = invoke("--", *ORACLE)
        self.assertEqual(code, run.EXIT_PASS, output)
        self.assertIn("PASS: all 27 fixtures within their thresholds", output)
        self.assertNotIn("FAIL ", output)

    def test_gate_fails_the_stub_with_diagnostics(self):
        code, output = invoke("--show", "0", "--", *STUB)
        self.assertEqual(code, run.EXIT_THRESHOLD, output)
        self.assertIn("WARNING: engine reports name 'stub'", output)
        self.assertIn(
            "p1 (born c0 src/inventory.rs:9) at c1: expected edited @ src/inventory.rs:9, "
            "got dead [score 0.00, layer stub_exact_line_sequence]",
            output,
        )
        self.assertIn("`pub fn parse_item(line: &str) -> Option<Item> {`", output)
        self.assertIn("Confusion matrix", output)
        self.assertRegex(output, r"FAIL: \d+ of 27 scored fixtures exceed their threshold")

    def test_stub_numbers_match_a_hand_derivation(self):
        # Exact-text matching keeps every unchanged line and kills the 7 lines containing a renamed identifier.
        with open(os.path.join(RENAME_RUST, "manifest.json"), encoding="utf-8") as handle:
            manifest = json.load(handle)
        repo = os.path.join(RENAME_RUST, "repo")
        trace = run_engine(STUB, repo, timeout=60, cwd=RUNNER_DIR)
        validate_trace(trace, [commit["sha"] for commit in manifest["commits"]])
        result = score_fixture("rename-identifier/rust", manifest, trace, threshold=0)
        metrics = class_metrics(tally(result.decisions))
        self.assertEqual(result.errors, 7)
        self.assertEqual((metrics["verbatim"].expected, metrics["verbatim"].predicted, metrics["verbatim"].correct), (24, 24, 24))
        self.assertEqual((metrics["edited"].expected, metrics["edited"].predicted, metrics["edited"].correct), (7, 0, 0))
        self.assertEqual((metrics["dead"].expected, metrics["dead"].predicted, metrics["dead"].correct), (0, 7, 0))
        self.assertEqual(result.unmatched_engine_lines, 7)

    def test_crashing_engine_is_a_harness_error(self):
        code, output = invoke(
            "--only", "rename-identifier/rust", "--",
            sys.executable, "-c", "import sys; sys.stderr.write('boom'); sys.exit(3)",
        )
        self.assertEqual(code, run.EXIT_HARNESS, output)
        self.assertIn("engine exited with code 3: boom", output)

    def test_non_json_output_is_a_contract_error(self):
        code, output = invoke("--only", "rename-identifier/rust", "--", sys.executable, "-c", "print('hello')")
        self.assertEqual(code, run.EXIT_HARNESS, output)
        self.assertIn("violates docs/specs/trace-cli.md: stdout is not valid JSON", output)

    def test_engine_that_writes_into_the_repo_is_rejected(self):
        marrow_dir = os.path.join(RENAME_RUST, "repo", ".marrow")
        self.addCleanup(shutil.rmtree, marrow_dir, True)
        writer = (
            "import os, sys; d = os.path.join(sys.argv[3], '.marrow'); os.makedirs(d, exist_ok=True); "
            "open(os.path.join(d, 'db.sqlite'), 'w').close()"
        )
        code, output = invoke("--only", "rename-identifier/rust", "--", sys.executable, "-c", writer)
        self.assertEqual(code, run.EXIT_HARNESS, output)
        self.assertIn("must be read-only", output)


if __name__ == "__main__":
    unittest.main()
