# ADR 0005: How the grading tool works, and what CI checks before the engine exists

Status: accepted
Date: 2026-09-16

## Context

Build plan step 1.2 asks for three things:

- a tool that runs `marrow trace --repo <path> --json` on each fixture and grades
  the output
- a fake engine, or stub, so the tool can be tested before the real engine exists
- a `just eval` command that runs it

ADR 0004 left one question open: what CI should check while there's no real
engine to grade.

## Decisions

### The grading tool is written in Python

Like the fixture generator, the grading tool is written in Python so that it
stays independent of the engine. It checks the engine's JSON output against its
own copy of the rules in `docs/specs/trace-cli.md`, instead of sharing code with
the engine. If the engine's output ever drifts from the spec, grading fails
instead of the mismatch slipping through.

### The stub is a separate Python script

The stub lives in `eval/runner/stub_engine.py`, rather than as a placeholder
command in the Rust CLI, so no fake logic ends up in the product code. It reads
the fixture's git history and nothing else, and never looks at the manifest.
It keeps a line only when Python's difflib finds identical text in the same
order, and marks every other line as dead. That means it can't handle whitespace
changes, edits, or moves, so it fails most fixtures.

The stub reports its name as `stub`, and the report prints a warning whenever the
engine being graded isn't `marrow`. `just eval` finds the stub through the
`eval_engine` variable in the justfile.

### Exit codes tell a wrong engine apart from a broken setup

- 0: every fixture passed.
- 1: at least one fixture had more errors than allowed.
- 2: something went wrong before grading. The engine crashed, timed out, printed
  invalid JSON, broke one of the output rules, or changed the fixture repo, or the
  thresholds in the README don't match the fixtures.

A fixture that couldn't be graded is reported as an error, never turned into an
accuracy number.

### How answers are graded

- The engine's lines are matched to the manifest's lines by where each line first
  appeared.
- An answer is correct only if both the fate and the new position are right.
- A wrong answer is one of four kinds: the right fate at the wrong position
  (`misplaced`), the wrong fate, a line the engine never reported (`missing`), or
  a line the engine had already marked as dead in an earlier commit.
- For precision, a misplaced answer counts as a prediction of that fate, but not
  a correct one.
- If two of the engine's lines claim the same position in a commit, each extra
  claim counts as an error. The manifest never does this, but the engine could,
  even while getting every tracked line right.
- When precision or recall can't be calculated, for example because the engine
  never predicted that fate, the report shows `-` instead of 0 or 1.

### Thresholds come from the README

The grading tool reads each fixture's allowed error count from the fixture table
in `eval/README.md`. It stops with exit code 2 if a fixture has no row or two
rows, or if a row names a fixture that doesn't exist. That way the README can't
fall out of step with the fixtures without anyone noticing.

### The grading tool has its own tests

`just eval-selftest` runs 33 tests covering:

- the scoring math, checked against a confusion matrix and precision and recall
  worked out by hand, with one case for each kind of error
- every rule in the output spec
- reading thresholds from the README
- full runs, where a perfect "oracle" engine that replays the manifest's answers
  must pass, and the stub must fail with the expected messages
- the stub's numbers on `rename-identifier/rust`, which must match a count done
  by hand: 7 errors, all 24 verbatim lines right, and none of the 7 edited lines
  right
- engines that crash, print something other than JSON, or write into the fixture
  repo, which must all stop with exit code 2

To make sure these tests can actually fail, we broke the grading tool on purpose
in three ways: ignoring line positions, passing every fixture, and turning off
the output checks. Each time, tests failed.

### CI tests the grading tool, not the stub

CI's eval job runs `just eval-selftest`, which also rebuilds the fixtures and
checks their manifests. It doesn't run `just eval`. Grading the stub in CI would
fail on every push until the engine exists, which says nothing about the product
and teaches everyone to ignore a red check. Running `just eval` locally still
grades the stub and fails, as the build plan asks.

## Consequences

When the engine gets built (build plan step 3.1):

1. Implement `marrow trace` in `marrow-cli` following `docs/specs/trace-cli.md`,
   and point `eval_engine` at the built binary, making sure the recipe builds it
   first.
2. Change CI to grade the real engine. The engine is built one layer at a time,
   and some fixtures will keep failing until the last layer is done (build plan
   steps 3.1 and 3.2), so CI shouldn't require zero errors yet. Instead, record
   each fixture's error count the first time the real engine runs, and fail CI if
   any count goes up. Zero errors stays the goal for finishing Phase 3 (build plan
   step 3.3). Build that check then, when there are real numbers to record.
