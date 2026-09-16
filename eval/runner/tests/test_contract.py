import copy
import unittest

from harness import ContractError, validate_trace

C0, C1, C2 = "0" * 40, "1" * 40, "2" * 40
VALID = {
    "contract_version": 1,
    "engine": {"name": "marrow", "version": "0.1.0"},
    "commits": [C0, C1, C2],
    "lines": [
        {
            "birth": {"commit": C0, "path": "src/a.rs", "line": 1},
            "fates": [
                {"commit": C1, "state": "edited", "path": "src/a.rs", "line": 2, "similarity_score": 0.8, "deciding_layer": "alignment"},
                {"commit": C2, "state": "dead", "similarity_score": 0.1, "deciding_layer": "alignment"},
            ],
        },
        {
            "birth": {"commit": C1, "path": "src/a.rs", "line": 1},
            "fates": [{"commit": C2, "state": "verbatim", "path": "src/a.rs", "line": 1, "similarity_score": 1, "deciding_layer": "histogram"}],
        },
        {"birth": {"commit": C2, "path": "src/b.rs", "line": 1}, "fates": []},
    ],
}


class ContractTest(unittest.TestCase):
    def assertViolation(self, mutate, message):
        trace = copy.deepcopy(VALID)
        mutate(trace)
        with self.assertRaises(ContractError) as caught:
            validate_trace(trace, [C0, C1, C2])
        self.assertIn(message, str(caught.exception))

    def test_valid_trace_passes(self):
        validate_trace(copy.deepcopy(VALID), [C0, C1, C2])

    def test_fate_without_similarity_score(self):
        self.assertViolation(lambda t: t["lines"][0]["fates"][0].pop("similarity_score"), "similarity_score")

    def test_dead_fate_without_similarity_score(self):
        self.assertViolation(lambda t: t["lines"][0]["fates"][1].pop("similarity_score"), "similarity_score")

    def test_similarity_score_out_of_range(self):
        self.assertViolation(lambda t: t["lines"][0]["fates"][0].update(similarity_score=1.5), "similarity_score")

    def test_boolean_is_not_a_score(self):
        self.assertViolation(lambda t: t["lines"][0]["fates"][0].update(similarity_score=True), "similarity_score")

    def test_fate_without_deciding_layer(self):
        self.assertViolation(lambda t: t["lines"][1]["fates"][0].update(deciding_layer=""), "deciding_layer")

    def test_unknown_state(self):
        self.assertViolation(lambda t: t["lines"][1]["fates"][0].update(state="renamed"), "state must be one of")

    def test_commits_must_match_first_parent_history(self):
        self.assertViolation(lambda t: t.update(commits=[C0, C2, C1]), "first-parent history")

    def test_alive_line_missing_a_later_fate(self):
        self.assertViolation(lambda t: t["lines"][1].update(fates=[]), "no fate for commit")

    def test_skipped_commit(self):
        self.assertViolation(lambda t: t["lines"][0]["fates"].pop(0), "expected the fate for commit")

    def test_fate_after_death(self):
        def mutate(trace):
            trace["lines"][0]["fates"][0] = {"commit": C1, "state": "dead", "similarity_score": 0, "deciding_layer": "x"}
        self.assertViolation(mutate, "nothing may follow a dead fate")

    def test_dead_fate_with_location(self):
        self.assertViolation(lambda t: t["lines"][0]["fates"][1].update(path="src/a.rs", line=3), "must not have path or line")

    def test_survivor_without_location(self):
        self.assertViolation(lambda t: t["lines"][1]["fates"][0].pop("line"), "line must be a 1-indexed integer")

    def test_zero_indexed_line(self):
        self.assertViolation(lambda t: t["lines"][2]["birth"].update(line=0), "1-indexed")

    def test_windows_path(self):
        self.assertViolation(lambda t: t["lines"][2]["birth"].update(path="src\\b.rs"), "forward slashes")

    def test_duplicate_birth(self):
        self.assertViolation(lambda t: t["lines"].append(copy.deepcopy(t["lines"][2])), "already born")

    def test_wrong_contract_version(self):
        self.assertViolation(lambda t: t.update(contract_version=2), "contract_version")


if __name__ == "__main__":
    unittest.main()
