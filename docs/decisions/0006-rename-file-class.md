# ADR 0006: Fixtures for renamed files, and why their lines aren't moves

Status: accepted
Date: 2026-09-16

## Context

No fixture tested renamed files, which is what Layer 1 of the engine handles by
matching old file paths to new ones. PRD §7.5 lists a "rename" fixture, but the
build plan read that as renaming a variable or function. The project owner
approved adding a class of fixtures for renamed files.

That raised a question. ADR 0001 defined `moved` as the same content in a
different file or a different order. By that definition, every line in a renamed
file would count as moved.

## Decision

### A renamed file is still the same file

Lines in a renamed file are `verbatim` (or `edited`) at the new path, not `moved`.
This follows the order of the engine's layers in the PRD: Layer 1 pairs up renamed
files before any diffing happens, and Layer 4 only looks for moves among the lines
left over after that. It also keeps `moved` meaning what people expect, which is a
piece of code relocated, like a function extracted into another file. The
move-across-files fixtures still use `moved`.

### Renames are written into the fixture definition

A commit in a fixture definition can list its renames as
`renames={"old path": "new path"}`. Only then can a line marked `=` or `~` change
its path, and only to the renamed path. The generator rejects a rename whose old
file didn't exist, or whose new path already existed. It also rejects lines in a
renamed file that are marked as moved when their order hasn't changed.

The manifest records each rename on its commit, as
`"renames": [{"from": ..., "to": ...}]`. The field only appears on commits that
rename a file, so none of the 24 existing manifests changed.

### Each fixture also deletes and adds unrelated files

In the same commit, one unrelated file is deleted and a different unrelated file
is added. Their lines have to be marked dead and new, not paired up as a rename.
Without this, an engine that paired any deleted file with any added file would
pass. The two files share at most one common line (`fn main() {`, `}`, or
`import sys`), never several lines in a row. That keeps the fixture from deciding
the smallest block of lines that counts as a move, which hasn't been tuned yet.

### Each language varies the rename

The Rust and Python fixtures rename a file where it is, from `inventory` to
`stock`. The TypeScript fixture also moves it into a new folder, from
`src/inventory.ts` to `src/domain/stock.ts`, so an engine can't get by on matching
folder names. Each renamed file also has one edited line, so the old and new
versions are very similar but not identical.

## How this was checked

- The generator rejects each kind of bad rename listed above, and accepts a
  correct one.
- Every file in every fixture parses.
- A separate script that reads only the git repos and manifests found no problems
  in any of the 27 fixtures.
- On the TypeScript fixture, git's own rename detection (`git show -M`) pairs the
  renamed file at 88% similarity and treats the unrelated delete and add as
  separate files, which matches the manifest.

## Consequences

- There are now nine classes and 27 fixtures, all required to have zero errors.
- The engine spec (build plan step 2.1) has to set how similar two files must be
  to count as a rename. These fixtures don't test that limit closely, because the
  renamed files are nearly identical and the unrelated files share almost nothing.
  A harder fixture, such as a rename combined with heavy edits, should wait until
  that threshold is chosen.
