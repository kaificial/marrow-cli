"""Tests for block_fixture_edits.py: python -m unittest discover -s .claude/hooks"""

import os
import shutil
import tempfile
import unittest

from block_fixture_edits import refusal

DEFINITION = "eval/generator/definitions/format_only.py"


class BlockFixtureEditsTest(unittest.TestCase):
    def test_generated_fixtures_are_never_edited(self):
        for tool in ("Edit", "Write"):
            self.assertIsNotNone(
                refusal(tool, {"file_path": "C:\\repo\\eval\\fixtures\\x\\manifest.json"})
            )

    def test_adding_to_a_definition_is_allowed(self):
        edit = {
            "file_path": DEFINITION,
            "old_string": 'TYPESCRIPT = Fixture(\n    language="typescript",\n)',
            "new_string": 'TYPESCRIPT = Fixture(\n    language="typescript",\n)\n\nJAVASCRIPT = Fixture()',
        }
        self.assertIsNone(refusal("Edit", edit))

    def test_changing_or_removing_an_answer_is_refused(self):
        changed = {
            "file_path": DEFINITION,
            "old_string": "p1  = | pub fn parse_item(line: &str) -> Option<Item> {",
            "new_string": "p1  ~ | pub fn parse_item(line: &str) -> Option<Item> {",
        }
        self.assertIsNotNone(refusal("Edit", changed))
        removed = {"file_path": DEFINITION, "old_string": "s1  + | x\ns2  + | y", "new_string": "s1  + | x"}
        self.assertIsNotNone(refusal("Edit", removed))
        everywhere = {"file_path": DEFINITION, "old_string": "a", "new_string": "a b", "replace_all": True}
        self.assertIsNotNone(refusal("Edit", everywhere))

    def test_a_rewrite_must_keep_the_whole_old_file(self):
        directory = tempfile.mkdtemp()
        try:
            path = os.path.join(directory, "eval", "generator", "definitions", "reindent.py")
            os.makedirs(os.path.dirname(path))
            with open(path, "w", encoding="utf-8") as file:
                file.write("RUST = 1\nPYTHON = 2\n")
            self.assertIsNone(refusal("Write", {"file_path": path, "content": "RUST = 1\nPYTHON = 2\nJAVASCRIPT = 3\n"}))
            self.assertIsNotNone(refusal("Write", {"file_path": path, "content": "RUST = 1\nJAVASCRIPT = 3\n"}))
            new = os.path.join(os.path.dirname(path), "brand_new.py")
            self.assertIsNone(refusal("Write", {"file_path": new, "content": "X = 1\n"}))
        finally:
            shutil.rmtree(directory, ignore_errors=True)

    def test_everything_else_is_left_alone(self):
        self.assertIsNone(refusal("Edit", {"file_path": "engine/src/lib.rs", "old_string": "a", "new_string": "b"}))


if __name__ == "__main__":
    unittest.main()
