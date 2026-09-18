# ADR 0003: In a full rewrite, matching lines still count as deleted

Status: accepted
Date: 2026-09-16

## Context

When a file is rewritten from scratch, the new version usually still has a few
lines identical to the old one, like closing braces or a common import such as
`use std::collections::HashMap;`. We had to decide whether those lines count as
having survived.

## Decision

They don't. When a file is replaced wholesale, every old line dies and every line
in the new version is new, even where the text matches exactly. The full-rewrite
fixtures include a few matching lines on purpose, to test this.

## Reasons

- The PRD (§7.3) says a change larger than a set size should be treated as a full
  rewrite instead of being matched line by line. The build plan (step 3.3) says a
  rewrite means "everything dies, not one big move".
- Marrow measures whether the code someone wrote lasted. A `}` that got retyped as
  part of a new design didn't last. Counting it as a survivor would make
  thrown-away code look longer-lived than it was. Overstating survival is the
  mistake this project most wants to avoid, and it's the first thing the reviewer
  agent is told to check for.

## Consequences

- Diff tools latch onto unique lines that two versions share, and split a rewrite
  into smaller pieces around them. You can see this in `git log -p` on the
  `full-rewrite/python` fixture, where git shows the shared `typing` import as
  unchanged. The engine's rewrite rule has to look past those shared lines, rather
  than judging each small piece on its own.
- These fixtures are all clear-cut rewrites. How big a change has to be before it
  counts as a rewrite depends on a size limit that gets tuned later (build plan
  phases 2 and 3). No fixture tests that borderline case yet.
