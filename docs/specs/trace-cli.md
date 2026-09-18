# `marrow trace` output format (version 1)

`marrow trace` is the command the grading tool in `eval/runner` uses to see what
the engine decided about each line The grading tool checks every rule on this page. 
If the output breaks one, it stops with exit code 2 instead of producing a score.

## Command

```
marrow trace --repo <path> --json
```

- `<path>` is the absolute path to a git repository on the local machine. There's
  no way to pass a URL (PRD §3a).
- The engine reads the history of `HEAD` from the first commit onward, following
  only the first parent of each merge, and works out what happened to every line
  between each pair of consecutive commits.
- The command must not change the repository in any way: no commits, no
  checkouts, and no `.marrow/` folder or other new files. The grading tool compares
  the repository's state before and after the run.
- On success it exits with 0 and prints exactly one JSON document to stdout,
  encoded as UTF-8. Log messages go to stderr.
- On failure it exits with a non-zero code and explains why on stderr.

## Output

```json
{
  "contract_version": 1,
  "engine": {"name": "marrow", "version": "0.1.0"},
  "commits": ["<hash of the first commit>", "...", "<hash of HEAD>"],
  "lines": [
    {
      "birth": {"commit": "<hash>", "path": "src/inventory.rs", "line": 9},
      "fates": [
        {"commit": "<hash>", "state": "edited", "path": "src/inventory.rs", "line": 9,
         "similarity_score": 0.87, "deciding_layer": "within_hunk_alignment"}
      ]
    }
  ]
}
```

### Top-level fields

| Field | Rules |
|---|---|
| `contract_version` | Must be `1`. |
| `engine` | An object with string fields `name` and `version`. The real engine uses the name `marrow`, and the grading tool prints a warning for any other name. |
| `commits` | The full 40-character hashes of the commits that were read, oldest first. They must match the repository's actual history. |
| `lines` | One entry per line the engine tracked. It can include lines the fixtures don't track, such as blank lines. Those aren't graded, but the report says how many there were. |

### Fields for each line

| Field | Rules |
|---|---|
| `birth` | Where the line first appeared: `commit` (a hash from `commits`), `path` (relative to the repository root, with forward slashes), and `line` (numbered from 1). No two lines can have the same birth. |
| `fates` | What happened to the line in each later commit: exactly one entry per commit, in order, until the line dies. A line still alive at `HEAD` has an entry for every commit after its birth. Nothing can come after a `dead` entry. |

### Fields for each fate

| Field | Rules |
|---|---|
| `commit` | The commit this entry describes. |
| `state` | `verbatim`, `edited`, `moved`, or `dead` (PRD §8). |
| `path`, `line` | Where the line is in this commit. Required unless the state is `dead`, and not allowed when it is. |
| `similarity_score` | A number from 0 to 1, required on every entry, including `dead` ones. For a dead line, use the score of the closest match that was rejected, or 0 if there was nothing to compare against. |
| `deciding_layer` | The name of the engine layer that made the decision, required on every entry. The allowed names are listed in [the genealogy spec](genealogy.md#deciding-layer-names). |

Requiring a score and a layer on every decision matches the rule CLAUDE.md sets
for the `fates` table: no decision is stored without the evidence behind it.

## How the output is graded

- The engine's lines are matched to the fixture's manifest by where each line was
  born (commit, path, and line number). The engine's own line IDs are never
  compared.
- An answer is correct only if the state is right, and for a line that still
  exists, the path and line number are right too (ADR 0001).
- In each commit, only one line can occupy a given position: either a line born
  there, or one existing line that stayed or moved there. Each extra line claiming
  the same position counts as one error. This doesn't break the format, so grading
  still goes ahead.

## Exit codes of `eval/runner/run.py`

| Code | Meaning |
|---|---|
| 0 | Every fixture is within its allowed error count in `eval/README.md`. |
| 1 | At least one fixture has more errors than allowed. |
| 2 | Grading couldn't run properly. The engine crashed, timed out, printed invalid JSON, broke a rule on this page, or changed the repository, or the thresholds in the README don't match the fixtures. |
