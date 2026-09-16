import os
import tempfile
import unittest

from harness import HarnessError, discover_fixtures, load_thresholds

EVAL_DIR = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


class ThresholdTest(unittest.TestCase):
    def write_readme(self, text):
        handle = tempfile.NamedTemporaryFile("w", suffix=".md", delete=False, encoding="utf-8")
        handle.write(text)
        handle.close()
        self.addCleanup(os.remove, handle.name)
        return handle.name

    def test_real_readme_records_zero_errors_for_every_fixture(self):
        names = [name for name, _, _ in discover_fixtures(os.path.join(EVAL_DIR, "fixtures"))]
        self.assertEqual(len(names), 27)
        thresholds = load_thresholds(os.path.join(EVAL_DIR, "README.md"), names)
        self.assertEqual(thresholds, {name: 0 for name in names})

    def test_parses_error_counts(self):
        path = self.write_readme("| Fixture | Mutation | Threshold |\n|---|---|---|\n| a-b/rust | x | y | 0 errors |\n| a-b/python | z | 2 errors |\n")
        self.assertEqual(load_thresholds(path, ["a-b/rust", "a-b/python"]), {"a-b/rust": 0, "a-b/python": 2})

    def test_fixture_without_threshold_row_is_an_error(self):
        path = self.write_readme("| a-b/rust | x | 0 errors |\n")
        with self.assertRaisesRegex(HarnessError, "no threshold recorded for a-b/python"):
            load_thresholds(path, ["a-b/rust", "a-b/python"])

    def test_threshold_row_for_unknown_fixture_is_an_error(self):
        path = self.write_readme("| a-b/rust | x | 0 errors |\n| gone/rust | x | 0 errors |\n")
        with self.assertRaisesRegex(HarnessError, "do not exist: gone/rust"):
            load_thresholds(path, ["a-b/rust"])

    def test_duplicate_threshold_rows_are_an_error(self):
        path = self.write_readme("| a-b/rust | x | 0 errors |\n| a-b/rust | x | 3 errors |\n")
        with self.assertRaisesRegex(HarnessError, "more than one threshold row"):
            load_thresholds(path, ["a-b/rust"])


if __name__ == "__main__":
    unittest.main()
