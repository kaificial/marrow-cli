import io
import json
import os
import shutil
import tempfile
import unittest

import run
from harness import Decision, FixtureResult


def result(engine_name, outcomes):
    decisions = [
        Decision(line_id=str(i), text="", birth={}, commit="", expected={"state": "verbatim"}, actual=None, outcome=o)
        for i, o in enumerate(outcomes)
    ]
    return FixtureResult(
        name="fixture",
        threshold=0,
        engine={"name": engine_name, "version": "9.9.9"},
        decisions=decisions,
        conflicts=[],
        unmatched_engine_lines=0,
    )


class RecordGradeTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.mkdtemp()
        self.binary = os.path.join(self.directory, "marrow")
        with open(self.binary, "wb") as handle:
            handle.write(b"not really an engine")
        self.addCleanup(shutil.rmtree, self.directory)

    def test_a_full_run_of_marrow_is_recorded_beside_the_binary(self):
        results = [result("marrow", ["correct", "correct"]), result("marrow", ["correct", "missing"])]
        run.record_grade([self.binary], results, ["broken"], 3, io.StringIO())
        with open(self.binary + ".eval.json", encoding="utf-8") as handle:
            record = json.load(handle)
        status = os.stat(self.binary)
        self.assertEqual(record["engine_version"], "9.9.9")
        self.assertEqual(record["binary_size"], status.st_size)
        self.assertEqual(record["binary_mtime_ns"], status.st_mtime_ns)
        self.assertEqual(record["fixtures"], 3)
        self.assertEqual(record["fixtures_passed"], 1)
        self.assertEqual(record["fixtures_unscored"], 1)
        self.assertEqual(record["fate_decisions"], 4)
        self.assertEqual(record["fate_decisions_correct"], 3)

    def test_another_engine_is_not_recorded(self):
        run.record_grade([self.binary], [result("stub", ["correct"])], [], 1, io.StringIO())
        self.assertFalse(os.path.exists(self.binary + ".eval.json"))

    def test_an_engine_run_through_an_interpreter_is_not_recorded(self):
        run.record_grade([self.binary, "script.py"], [result("marrow", ["correct"])], [], 1, io.StringIO())
        self.assertFalse(os.path.exists(self.binary + ".eval.json"))


if __name__ == "__main__":
    unittest.main()
