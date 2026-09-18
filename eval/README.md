# Mutation corpus

This folder holds the test data for Marrow's line-tracking engine (`marrow-core`)
and the tool that grades the engine against it.

Each fixture is a small, curated git repo with two commits: a starting version
of some code, then one kind of change to it, such as a function moved to another
file or a file run through a formatter. For every line, the fixture's
`manifest.json` records what really happened to it. We call that the line's
fate:

- `verbatim`: kept, apart from whitespace
- `edited`: the same line, with its content changed
- `moved`: the same content, now in another file or another place in the file
- `dead`: deleted

The engine is graded on how many fates it gets right. Its settings are tuned on
these same fixtures, so a score here is measured on data the engine has already
seen.

## Commands

| Command | What it does |
|---|---|
| `just fixtures` | Rebuild all 27 fixture repos and rewrite their manifests |
| `just fixtures-check` | Rebuild the repos and fail if any committed manifest (including its commit hashes) would change |
| `just eval` | Grade the engine against every fixture. Exits with 0 if all pass, 1 if any fixture has more errors than allowed, and 2 if something went wrong before grading |
| `just eval --only duplicate-heavy --show 0` | Grade only the fixtures whose name contains the given text, and list every wrong line |
| `just eval-selftest` | Test the grading tool itself |

`just eval` builds the engine first, then grades `engine/target/debug/marrow`.
Every fixture currently passes with zero errors. The report prints a warning
whenever the engine it graded isn't Marrow.

## Layout

```
eval/
  generator/
    generate.py              builds every fixture (Python 3.10+, standard library only)
    fixture_lib.py           reads definitions, checks declared fates, makes the commits, writes manifests
    definitions/<class>.py   the answers: one definition per language, every line labeled by hand
  fixtures/<class>/<language>/
    manifest.json            generated from the definition, and committed
    repo/                    the generated git repo the engine runs on (not committed)
  runner/
    run.py                   runs the engine on each fixture and prints the report
    harness.py               checks the engine's output format, scores it, and reads thresholds
    stub_engine.py           a deliberately weak stand-in engine the tests use to check that grading fails
    tests/                   tests for the grading tool, plus a perfect "oracle" engine they use
```

## How grading works

`just eval` runs `<engine> trace --repo <fixture repo> --json` on every fixture.
The output format is defined in
[`docs/specs/trace-cli.md`](../docs/specs/trace-cli.md). The tool first checks
that the output follows that format, then compares it with the manifest.

An answer only counts as correct if the engine gets both the fate and the line's
new position right. Saying a line was kept, but at the wrong line number, is an
error.

The report includes:

- PASS or FAIL for each fixture, with its error count. A failing fixture lists
  each wrong line with the expected answer, the engine's answer, the similarity
  score and engine layer behind that answer, and the line's source text.
- Precision and recall for each fate, per fixture and across all fixtures.
- A confusion matrix of expected fates against the engine's answers, with two
  extra columns: `misplaced` (right fate, wrong position) and `missing` (a line
  the engine never reported).
- How many errors each engine layer made.

If the engine puts two lines in the same position in one commit, each extra
line there also counts as an error.

## Fixtures

Each fixture has a base commit, `c0`, and one change, `c1`. Every fixture must
be graded with zero errors ([ADR 0004](../docs/decisions/0004-eval-accuracy-gate.md)).
`just eval` reads each fixture's allowed error count from the last column of
the second table below, so keep that column in the form `0 errors`.

| Class | What it checks | Main engine layer |
|---|---|---|
| format-only | Reformatting only changes whitespace, so every line survives, even when added or removed blank lines shift the line numbers. | 0, normalization |
| rename-identifier | A line where a name changed is the same line, edited. That includes short lines like `use inventory::parse_record;` where only one word differs. | 3, alignment |
| rename-file | Lines in a renamed file are kept at the new path, not counted as moves. An unrelated file deleted and another added in the same commit must not be mistaken for a rename ([ADR 0006](../docs/decisions/0006-rename-file-class.md)). | 1, file matching |
| move-within-file | A function moved to another part of its file is detected as a move, while the code around it stays verbatim. | 4, move detection |
| move-across-files | A function moved into a new file is detected as a move, and identical lines left behind in the old file aren't claimed by it. | 4, move detection |
| delete-function | Deleted lines die, even when a similar function below them shifts up into their old line numbers. | 2 and 3, diff and alignment |
| reindent | Wrapping code in an `if` only adds indentation, so the wrapped lines survive. The new `if` line and its closing `}` are new. | 0, normalization |
| full-rewrite | When a file is rewritten from scratch, every old line dies, even lines like `}` that happen to reappear ([ADR 0003](../docs/decisions/0003-full-rewrite-coincident-lines.md)). | 3, rewrite size limit |
| duplicate-heavy | With many identical lines, the engine has to match them by their order in the file, not just by their text or line number. | 3, alignment |

| Fixture | Change from c0 to c1 | Threshold |
|---|---|---|
| format-only/rust | Run through rustfmt: spacing fixed, indentation changed from 2 to 4 spaces, a double blank line reduced to one | 0 errors |
| format-only/python | Run through black: spacing fixed, indentation changed from 2 to 4 spaces, blank lines added between definitions, a triple blank line reduced to two | 0 errors |
| format-only/typescript | Run through prettier: spacing fixed, indentation changed from 4 to 2 spaces, a triple blank line reduced to one | 0 errors |
| rename-identifier/rust | `parse_item` renamed to `parse_record` in two files, and `parts` to `fields`; 7 lines edited | 0 errors |
| rename-identifier/python | `parse_item` renamed to `parse_record` in two files, and `parts` to `fields`; 7 lines edited | 0 errors |
| rename-identifier/typescript | `parseItem` renamed to `parseRecord` in two files, and `parts` to `fields`; 7 lines edited | 0 errors |
| rename-file/rust | `src/inventory.rs` renamed to `src/stock.rs`, with its doc comment edited and `main.rs` updated. `src/bin/legacy_import.rs` is deleted and an unrelated `src/bin/report.rs` added; they share only `fn main() {` and `}` | 0 errors |
| rename-file/python | `inventory.py` renamed to `stock.py`, with its header comment edited and the import in `main.py` updated. `scripts/legacy_import.py` is deleted and an unrelated `scripts/report.py` added; they share only `import sys` | 0 errors |
| rename-file/typescript | `src/inventory.ts` moved and renamed to `src/domain/stock.ts`, with its header comment edited and the import in `main.ts` updated. `scripts/legacyImport.ts` is deleted and an unrelated `scripts/report.ts` added; they share no lines | 0 errors |
| move-within-file/rust | A 5-line helper function moved from the top of the file to the bottom, past 17 unchanged lines | 0 errors |
| move-within-file/python | A 4-line helper function moved from the top of the file to the middle, past 9 unchanged lines | 0 errors |
| move-within-file/typescript | A 5-line helper function moved from the bottom of the file to the top, past 18 unchanged lines | 0 errors |
| move-across-files/rust | `total_value` moved into a new `src/pricing.rs`; the `use` line in `main.rs` edited and a `mod pricing;` line added | 0 errors |
| move-across-files/python | `total_value` moved into a new `pricing.py`; the import in `main.py` edited | 0 errors |
| move-across-files/typescript | `totalValue` moved into a new `src/pricing.ts`; the import in `main.ts` edited | 0 errors |
| delete-function/rust | `total_value` and its doc comment deleted (7 lines); the similar `most_valuable` below moves up into their line numbers | 0 errors |
| delete-function/python | `total_value` and its comment deleted (5 lines); the similar `most_valuable` below moves up into their line numbers | 0 errors |
| delete-function/typescript | `totalValue` and its comment deleted (8 lines); `totalQuantity` below shares 5 identical lines with it and moves up | 0 errors |
| reindent/rust | A loop and the line after it wrapped in an `if`: 2 new lines, and 4 kept lines now indented one level deeper | 0 errors |
| reindent/python | Two blocks each wrapped in an `if`, which needs no closing line in Python: 2 new lines, and 5 kept lines indented deeper | 0 errors |
| reindent/typescript | A loop and the line after it wrapped in an `if`: 2 new lines, and 4 kept lines now indented one level deeper | 0 errors |
| full-rewrite/rust | `src/inventory.rs` rewritten from scratch: 18 lines deleted and 28 added, including an identical import and several `}` lines; `money.rs` unchanged | 0 errors |
| full-rewrite/python | `inventory.py` rewritten from scratch: 15 lines deleted and 21 added, including an identical `typing` import; `money.py` unchanged | 0 errors |
| full-rewrite/typescript | `src/inventory.ts` rewritten from scratch: 18 lines deleted and 25 added, including several identical `}` lines; `money.ts` unchanged | 0 errors |
| duplicate-heavy/rust | Six `if` checks all end in the same `return Err(...)` line. The third check gets a new logging line, identical to ones in other checks, and a different error message, which shifts every line below it | 0 errors |
| duplicate-heavy/python | The same change, with `raise ValueError("invalid item")` | 0 errors |
| duplicate-heavy/typescript | The same change, with `throw new Error("invalid item");` | 0 errors |

Each manifest's `summary` field describes its fixture in more detail, and
`just fixtures` prints how many lines of each fate every fixture has.

## Manifest format (version 1)

The reasoning behind this format is in
[ADR 0001](../docs/decisions/0001-mutation-corpus-design.md).

```json
{
  "schema_version": 1,
  "fixture": "rename-identifier/rust",
  "mutation_class": "rename-identifier",
  "language": "rust",
  "summary": "…",
  "commits": [{"id": "c0", "sha": "…", "timestamp": "2026-01-01T00:00:00Z", "message": "…"},
              {"id": "c1", …, "renames": [{"from": "src/inventory.rs", "to": "src/stock.rs"}]}],
  "lines": [
    {"line_id": "p1", "kind": "code", "text": "pub fn parse_item(line: &str) -> Option<Item> {",
     "birth": {"commit": "c0", "path": "src/inventory.rs", "line": 9},
     "fates": [{"commit": "c1", "state": "edited", "path": "src/inventory.rs", "line": 9}]}
  ]
}
```

- A line is identified by where it first appeared (`birth`). `line_id` is a
  short label that only exists to make error messages readable. The engine's own
  line IDs don't need to match it.
- `fates` has one entry for each later commit while the line exists, and ends at
  `dead`.
- A renamed file keeps its identity, so its lines can be `verbatim` or `edited`
  at the new path. `moved` means a line went to a different file some other way,
  or changed position relative to the lines around it
  ([ADR 0006](../docs/decisions/0006-rename-file-class.md)).
- `renames` only appears on commits that rename a file, and lists each rename.
- Lines added in the last commit have an empty `fates` list. They're included so
  the grading tool can tell when the engine wrongly matches an old line to a new
  one.
- `kind` is `code` or `comment`. Blank lines aren't tracked.

## Rules

- Don't edit a `manifest.json` by hand. To change an answer, edit the fixture's
  definition in `eval/generator/definitions/`, run `just fixtures`, and review
  the change to the manifest.
- Fates are written by hand, and the generator never works them out itself.
  Each line in a definition looks like `label op | code`, where the op is `+` for
  a new line, `=` for verbatim, `~` for edited, or `>` for moved. Deleted lines
  go in `dead="..."`, and renamed files in `renames={"old path": "new path"}`.
  The generator refuses to build a fixture if the declared fates don't match the
  code, the line order, or the declared renames.
- The answers are protected. A Claude Code hook stops the agent from editing
  anything in `eval/fixtures/` or `eval/generator/definitions/`, so only the
  project owner changes them.
- Never lower a threshold or weaken a fixture to make `just eval` pass. Fix the
  engine, or if the fixture itself is wrong, raise it with the project owner.
- A new fixture should test one kind of change, look like real code, and have a
  clear right answer for every line. If the right answer for a line is
  debatable, change the fixture instead.

## Known gaps

- Grading checks each line's fate and its new position, not which engine layer
  decided it. Two different paths through the engine can both pass: with the
  block limits raised, the move fixtures still pass because their moved lines are
  unique and move individually instead.

- Formatter changes other than whitespace aren't tested yet: splitting or
  joining lines, changing quotes or punctuation, and sorting imports. The engine
  spec has to decide how to handle them first
  ([ADR 0002](../docs/decisions/0002-format-only-scope.md)).
- These are out of scope for v1 (build plan step 2.1): lines that are edited and
  moved in the same commit, merge commits, and generated or vendored files.
