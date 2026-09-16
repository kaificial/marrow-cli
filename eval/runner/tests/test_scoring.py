import unittest

from harness import class_metrics, score_fixture, tally

C0, C1 = "0" * 40, "1" * 40
MANIFEST = {
    "commits": [{"id": "c0", "sha": C0}, {"id": "c1", "sha": C1}],
    "lines": [
        {"line_id": "right", "text": "a", "birth": {"commit": "c0", "path": "f", "line": 1},
         "fates": [{"commit": "c1", "state": "verbatim", "path": "f", "line": 1}]},
        {"line_id": "shifted", "text": "b", "birth": {"commit": "c0", "path": "f", "line": 2},
         "fates": [{"commit": "c1", "state": "verbatim", "path": "f", "line": 2}]},
        {"line_id": "renamed", "text": "c", "birth": {"commit": "c0", "path": "f", "line": 3},
         "fates": [{"commit": "c1", "state": "edited", "path": "f", "line": 4}]},
        {"line_id": "extracted", "text": "d", "birth": {"commit": "c0", "path": "f", "line": 4},
         "fates": [{"commit": "c1", "state": "moved", "path": "g", "line": 1}]},
        {"line_id": "deleted", "text": "e", "birth": {"commit": "c0", "path": "f", "line": 5},
         "fates": [{"commit": "c1", "state": "dead"}]},
        {"line_id": "also_deleted", "text": "f", "birth": {"commit": "c0", "path": "f", "line": 6},
         "fates": [{"commit": "c1", "state": "dead"}]},
    ],
}


def fate(state, path=None, line=None, score=0.5):
    result = {"commit": C1, "state": state, "similarity_score": score, "deciding_layer": "test"}
    if path is not None:
        result.update(path=path, line=line)
    return result


def engine_line(line, *fates):
    return {"birth": {"commit": C0, "path": "f", "line": line}, "fates": list(fates)}


class ScoringTest(unittest.TestCase):
    def score(self, engine_lines):
        trace = {"engine": {"name": "test", "version": "0"}, "commits": [C0, C1], "lines": engine_lines}
        return score_fixture("synthetic", MANIFEST, trace, threshold=0)

    def test_perfect_trace_scores_every_present_class_at_one(self):
        result = self.score([
            engine_line(1, fate("verbatim", "f", 1)),
            engine_line(2, fate("verbatim", "f", 2)),
            engine_line(3, fate("edited", "f", 4)),
            engine_line(4, fate("moved", "g", 1)),
            engine_line(5, fate("dead")),
            engine_line(6, fate("dead")),
        ])
        self.assertEqual(result.errors, 0)
        self.assertTrue(result.passed)
        for state, metrics in class_metrics(tally(result.decisions)).items():
            self.assertEqual((metrics.precision, metrics.recall), (1.0, 1.0), state)

    def test_hand_computed_metrics_for_each_kind_of_error(self):
        result = self.score([
            engine_line(1, fate("verbatim", "f", 1)),
            engine_line(2, fate("verbatim", "f", 3)),
            engine_line(3, fate("dead")),
            engine_line(5, fate("verbatim", "f", 2)),
            engine_line(6, fate("dead")),
        ])
        outcomes = {decision.line_id: decision.outcome for decision in result.decisions}
        self.assertEqual(outcomes, {
            "right": "correct",
            "shifted": "misplaced",
            "renamed": "wrong_state",
            "extracted": "missing",
            "deleted": "wrong_state",
            "also_deleted": "correct",
        })
        matrix = tally(result.decisions)
        self.assertEqual(matrix["verbatim"], {"verbatim": 1, "edited": 0, "moved": 0, "dead": 0, "misplaced": 1, "missing": 0})
        self.assertEqual(matrix["edited"]["dead"], 1)
        self.assertEqual(matrix["moved"]["missing"], 1)
        self.assertEqual(matrix["dead"]["verbatim"], 1)
        self.assertEqual(matrix["dead"]["dead"], 1)

        metrics = class_metrics(matrix)
        self.assertEqual((metrics["verbatim"].expected, metrics["verbatim"].predicted, metrics["verbatim"].correct), (2, 3, 1))
        self.assertAlmostEqual(metrics["verbatim"].precision, 1 / 3)
        self.assertEqual(metrics["verbatim"].recall, 0.5)
        self.assertIsNone(metrics["edited"].precision)
        self.assertEqual(metrics["edited"].recall, 0.0)
        self.assertIsNone(metrics["moved"].precision)
        self.assertEqual(metrics["moved"].recall, 0.0)
        self.assertEqual((metrics["dead"].expected, metrics["dead"].predicted, metrics["dead"].correct), (2, 2, 1))
        self.assertEqual((metrics["dead"].precision, metrics["dead"].recall), (0.5, 0.5))
        self.assertEqual(result.errors, 4)
        self.assertFalse(result.passed)

    def test_missing_reason_distinguishes_untracked_from_already_dead(self):
        result = self.score([engine_line(1, fate("verbatim", "f", 1))])
        reasons = {decision.line_id: decision.missing_reason for decision in result.decisions if decision.outcome == "missing"}
        self.assertEqual(reasons["shifted"], "engine reported no line born here")

    def test_two_lines_claiming_one_destination_count_as_an_error(self):
        result = self.score([
            engine_line(1, fate("verbatim", "f", 1)),
            engine_line(2, fate("verbatim", "f", 2)),
            engine_line(3, fate("edited", "f", 4)),
            engine_line(4, fate("moved", "g", 1)),
            engine_line(5, fate("dead")),
            engine_line(6, fate("dead")),
            {"birth": {"commit": C1, "path": "f", "line": 1}, "fates": []},
        ])
        self.assertEqual(sum(decision.outcome != "correct" for decision in result.decisions), 0)
        self.assertEqual(len(result.conflicts), 1)
        self.assertEqual(result.conflicts[0][0], (C1, "f", 1))
        self.assertEqual(result.errors, 1)
        self.assertEqual(result.unmatched_engine_lines, 1)

    def test_threshold_allows_errors_up_to_its_value(self):
        trace = {"engine": {"name": "test", "version": "0"}, "commits": [C0, C1], "lines": []}
        self.assertEqual(score_fixture("synthetic", MANIFEST, trace, threshold=6).passed, True)
        self.assertEqual(score_fixture("synthetic", MANIFEST, trace, threshold=5).passed, False)


if __name__ == "__main__":
    unittest.main()
