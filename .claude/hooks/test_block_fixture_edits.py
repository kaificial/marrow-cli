"""Tests for block_fixture_edits.py: python -m unittest discover -s .claude/hooks"""

import os
import shutil
import tempfile
import unittest

from block_fixture_edits import refusal

COMMITTED = "RUST = 1\np1  = | keep this answer\nPYTHON = 2\n"


class BlockFixtureEditsTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.mkdtemp()
        folder = os.path.join(self.directory, "eval", "generator", "definitions")
        os.makedirs(folder)
        self.path = os.path.join(folder, "format_only.py")

    def tearDown(self):
        shutil.rmtree(self.directory, ignore_errors=True)

    def write(self, text):
        with open(self.path, "w", encoding="utf-8") as file:
            file.write(text)

    def ask(self, tool, **fields):
        return refusal(tool, {"file_path": self.path, **fields}, lambda _: COMMITTED)

    def test_generated_fixtures_are_never_edited(self):
        for tool in ("Edit", "Write"):
            self.assertIsNotNone(refusal(tool, {"file_path": "C:\\repo\\eval\\fixtures\\x\\manifest.json"}))

    def test_adding_to_a_definition_is_allowed(self):
        self.write(COMMITTED)
        self.assertIsNone(self.ask("Edit", old_string="PYTHON = 2\n", new_string="PYTHON = 2\nJAVASCRIPT = 3\n"))

    def test_changing_or_removing_a_committed_answer_is_refused(self):
        self.write(COMMITTED)
        self.assertIsNotNone(self.ask("Edit", old_string="p1  = |", new_string="p1  ~ |"))
        self.assertIsNotNone(self.ask("Edit", old_string="PYTHON = 2\n", new_string=""))
        self.assertIsNotNone(self.ask("Write", content="RUST = 1\nPYTHON = 2\n"))

    def test_lines_added_since_the_commit_can_still_be_corrected(self):
        self.write(COMMITTED + "c1 + | a label that clashes\n")
        self.assertIsNone(self.ask("Edit", old_string="c1 + | a label", new_string="k1 + | a label"))

    def test_a_file_not_yet_committed_is_free(self):
        self.assertIsNone(refusal("Write", {"file_path": self.path, "content": "X = 1\n"}, lambda _: None))

    def test_everything_else_is_left_alone(self):
        self.assertIsNone(refusal("Edit", {"file_path": "engine/src/lib.rs", "old_string": "a", "new_string": "b"}))


if __name__ == "__main__":
    unittest.main()
