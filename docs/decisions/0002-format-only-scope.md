# ADR 0002: The format-only fixtures only change whitespace

Status: accepted
Date: 2026-09-16

## Context

The build plan describes the format-only fixtures as "lines survive verbatim".
That sounds simple, but code formatters do more than adjust spacing. To see what
they actually do, we ran rustfmt 1.8.0, black, and prettier 3 on sample code
written to trigger each of their behaviors:

| What the formatter did | Formatters | Effect on lines | Effect on content |
|---|---|---|---|
| Changed spacing, indentation, or blank lines | all three | each line stays one line | unchanged |
| Split a long line into several | all three | one line becomes several | unchanged |
| Joined several short lines into one | all three | several lines become one | unchanged |
| Moved the line breaks in a multi-line expression | prettier | lines regrouped into a different number of lines | unchanged |
| Switched quote style, such as `'a'` to `"a"` | black, prettier | each line stays one line | changed |
| Added or removed trailing commas and semicolons | all three | varies | changed |
| Sorted imports | rustfmt | lines reordered | unchanged |

## Decision

The format-only fixtures only use the first row: spacing, indentation, and blank
lines. The second commit in each one is real output from rustfmt, black, or
prettier, and we checked that it only changed whitespace.

We decided not to include split and joined lines with a simple rule like "the
first of the new lines keeps the old line's identity". There were three reasons:

1. The engine compares code one line at a time. Nothing in its design can say
   that one line became three, so the fixture would test something the engine was
   never built to do.
2. The rule would change Marrow's results. If a line split into five counted as
   one survivor plus four new lines, every formatter run would inflate the line
   counts. That's a decision about how Marrow measures survival, not a detail of
   how a test is written.
3. A fixture mixing splits, quote changes, sorting, and whitespace could fail
   without telling you which of them caused it.

## Consequences

This isn't a rare case. Formatters make these changes constantly, and this repo
itself runs `cargo fmt` after every edit Claude makes to a Rust file. The engine
spec (build plan step 2.1) needs to decide how to handle split and joined lines,
quote and punctuation changes, and sorted imports. Each of those should then get
its own fixture before any results from real projects are trusted.
