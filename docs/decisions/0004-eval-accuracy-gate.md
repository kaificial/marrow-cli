# ADR 0004: Every fixture must be graded with zero errors

Status: accepted (the project owner asked for this decision to be made for them)
Date: 2026-09-16

## Context

`eval/README.md` records how many errors each fixture is allowed. The PRD (§9.1)
asks for at least 95% precision and recall on each fate. There were three
options: apply 95% to each fixture, apply 95% to all fixtures combined, or allow
no errors at all.

## Decision

No errors. Every fixture has to get every line exactly right, which means 100%
precision and recall. The grading tool still reports precision and recall for
each fate, per fixture and overall, because the PRD asks for those numbers to be
published.

## Reasons

1. There's no randomness to allow for. A fixture gives the same result every
   time, and each one was designed so the right answer for every line is clear.
   So an error always points to a real bug in the engine or a gap in its spec,
   never to bad luck. Allowing a few errors would mean accepting failures we
   already know about.
2. Percentages don't work with this few lines. A fixture tracks between 16 and 49
   lines, and many fates appear only a handful of times: `duplicate-heavy` has
   exactly one edited line, and the move fixtures have 4 to 6 moved lines. With
   one line, 95% really means 100%. With five moved lines, a single miss drops the
   score to 80%. A 95% bar would mean something different in every fixture.
3. An average across all fixtures could hide the fixture that matters most.
   `duplicate-heavy` could fail while the overall score still passed, and the
   build plan (step 3.2) says not to move on if that fixture fails.
4. It still meets the PRD's requirement, since 100% is at least 95%.

## Limitation

The engine's settings are tuned against these same fixtures (PRD §7.3). A perfect
score shows the engine handles these cases, but it doesn't tell you how accurate
the engine is on real code. That comes from the roughly 200 real line histories
checked by hand (PRD §9.2), which is where a percentage target and a confidence
interval make sense. Don't quote corpus scores as real-world accuracy.

## Consequences

- If the engine can't get one fixture right without breaking another, that's a
  conflict in the spec to raise with the project owner. It isn't a reason to
  allow errors, and CLAUDE.md forbids lowering a threshold to make a test pass.
- CI can't require zero errors while the engine is still being built. ADR 0005
  covers what CI checks in the meantime.
