# ADR 0001: How the mutation corpus is built

Status: accepted
Date: 2026-09-16

## Context

Marrow's line-tracking engine is graded against small synthetic git repos
called fixtures (PRD §7.5). Each fixture comes with a manifest that says what
really happened to every line. If a manifest is wrong or vague, the grades mean
nothing, so we had to decide two things up front: what the manifest looks like,
and how its answers get written.

## Decision

### The generator is written in Python

A script builds every fixture from scratch. It uses Python's standard library
and the `git` command line tool, and nothing else.

The main reason is to keep the answers separate from the engine. If the
generator were Rust code in the same workspace, it would be easy to reuse the
engine's own parsing or diff code. A bug in that code would then show up in both
the engine and the answers, and the tests would never catch it. Python also
keeps the fixture definitions easy to read, which matters because they get
checked by hand.

The cost is a second language in the repo, and no shared types between the
generator and the engine. The manifest format is the only thing they share.

### One fixture per type of change per language

There are nine types of change and three languages (Rust, Python, and
TypeScript), so there are 27 fixtures, stored in
`eval/fixtures/<class>/<language>/`. The corpus started with eight types and 24
fixtures, and ADR 0006 added the ninth. Keeping the languages separate means a
failure tells you which language broke, and a function moved between files
always stays within one language.

### The manifest lists every line and what happened to it

The format follows the `lines` and `fates` tables in PRD §8:

- A line is identified by where it first appeared: the commit, the file path,
  and the line number, counting from 1. Each line also gets a short label, like
  `p1`, which only exists to make error messages readable.
- For each later commit, the line has one fate: `verbatim` (unchanged apart from
  whitespace), `edited` (the same line with different content), `moved` (the
  same content somewhere else), or `dead` (removed). Nothing comes after `dead`.
- Each line is marked as `code` or `comment`, so the engine spec can decide later
  whether comments count, without rebuilding any fixtures.
- The manifest stores each line's original text, so error messages can show it.

An answer is only correct if the fate and the line's new position both match.

### The answers are written by hand, not computed

In a fixture definition, every line has a label and a symbol for its fate: `+`
for new, `=` for verbatim, `~` for edited, and `>` for moved. Deleted lines are
listed by label. The generator only works out mechanical details, like line
numbers and commit hashes. It never runs a diff to decide what happened to a
line.

The generator does check the hand-written answers for mistakes. It refuses to
build a fixture when:

- a line marked verbatim actually changed
- a line marked edited only changed whitespace
- a line marked moved stayed in the same order, or its text changed
- lines marked verbatim or edited ended up out of order
- a line disappeared without being marked dead

### No way to skip a line

We considered letting a fixture mark a line as "don't grade this", for cases
where the right answer is debatable. We decided against it, because it would make
it too easy to hide failures. Instead, each fixture is designed so every answer
is clear. For example, a moved block is always small compared to the code around
it, and no fixture has two identical `}` lines side by side where you couldn't
tell which one is new.

### Blank lines aren't tracked

A blank line has no content, so there's nothing meaningful to say about whether
it was kept or moved. Blank lines still matter, though: adding or removing one
shifts the line numbers of everything below it, and those positions are graded.
The first proposal recorded blank lines without grading them, but that turned
out to add nothing.

### Every fixture builds the same way every time

The generated repos go in a `repo/` folder next to each manifest and aren't
committed. Only the manifests are. To make the commits identical on every
machine, the generator fixes the author, the dates (one day apart, starting
2026-01-01), the line endings, and the hash format, and it ignores the user's own
git settings. `just fixtures-check` rebuilds everything and fails if any
manifest, including its commit hashes, would change.

## Consequences

- To change an answer, edit the fixture definition and review the resulting
  change to the manifest. Never edit a manifest by hand.
- PRD §7.5 lists a "rename" fixture, and the build plan read that as renaming a
  variable or function. That left nothing testing a renamed file, which is what
  Layer 1 of the engine handles. ADR 0006 adds a `rename-file` class to cover it.
