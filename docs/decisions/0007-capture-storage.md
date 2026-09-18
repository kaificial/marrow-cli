# ADR 0007: What a captured write stores

Status: accepted, but it changes the PRD's data model (see Consequences)
Date: 2026-09-17

## Context

A Claude Code hook runs `marrow record` after every agent write (PRD §7.2). To
say what happened to each line, that write has to be compared with the previous
version of the same file. Three things constrain how:

- CLAUDE.md: no source code is ever stored, only hashes and metadata.
- There is no commit yet. The file has just been written to the working tree.
- Two writes can arrive in quick succession, even within the same second.

PRD §8 describes four tables: `lines`, `fates`, `sessions`, and `commits`.

## Decisions

### The store keeps the last known state of each file, as hashes

Two tables beyond PRD §8: `snapshots` (one row per file: language, normalizer
version, when it was last seen) and `snapshot_lines` (one row per tracked line:
its line id, line number, kind, role, content token count, fingerprint, and the
token, unigram and bigram hashes).

Comparing a write needs the previous version's line features, and we can't keep
the code, so the features are stored instead. Only the newest version of each
file is kept, because the engine only ever compares consecutive versions. The
history lives in `lines` and `fates`.

### A fate row is written only when something changed

A line that is still there and unchanged gets no row. Its snapshot carries the
time it was last seen, which is what right-censored survival analysis needs.
Writing a row per line per write would multiply the database by the number of
writes and add nothing.

### Fate rows have no unique key

Two writes can land in the same second, and both are real history, so
`(line_id, observed_at)` can't be a key. Rows are ordered by rowid, with an
index on `(line_id, observed_at)`.

### Fate rows record the line number

PRD §8 has no position column, but `marrow explain` (PRD §7.1) has to say where
a line was at each step, so `fates.line_number` holds the line's new position
(null when it died).

### A captured line has no birth commit yet

`lines.birth_commit` stays null at capture time and `birth_ts` carries the time.
The post-commit reconciliation (build plan step 4.2) attaches the commit.

### Smaller choices

- Timestamps are Unix epoch seconds, as integers. `marrow export --audit` can
  format them.
- `line_id` is the SQLite rowid.
- The model comes from `--model`, because Claude Code's hook payload doesn't
  include it. A session keeps the first model it is told.
- Paths are stored only as a hash, per PRD §8's `file_path_hash`. The hash is
  unkeyed for now, so someone holding the database could confirm a guessed path.
  Keying it belongs with the rest of the storage design in phase 5.

### What a hook can and can't see

A hook fires for one file, so `record` runs Layer 0, Layers 2 and 3, and moves
within that file. A move between files needs the whole tree, so it waits for the
post-commit reconciliation. Until then, a function moved to another file during a
session looks like a death and a birth.

### A write is never worth failing over

Data problems warn on stderr and exit 0, so the agent's write stands: an
untracked language, an excluded path, a file that isn't UTF-8 or is over the size
limit, a file the parser gives up on, or a file outside a git repository. A broken
store or an unreadable hook payload exits 1, which Claude Code reports without
blocking. Missing arguments exit 2.

If the stored snapshots were built by a different normalizer version, `record`
says so and stores nothing, because fingerprints from different versions can't be
compared. Rebuilding is `marrow backfill`'s job (phase 5).

## Verification

Ten end-to-end tests drive the built binary against a scratch repo: a first
write, an edit, a deletion, a block moved inside the file, three writes in a row,
two writes at the same time, the hook payload, files that can't be read, a write
outside a repository, and missing arguments. One more test, ignored by default
because it waits for the parse timeout, covers a file the parser gives up on.

On a real git repo driven by a real hook payload, a search of the database's
bytes for words from the source found none.

## Consequences

- **PRD §8 needs updating** if these tables are accepted: two more tables, a
  `line_number` column on `fates`, no unique key on `fates`, and a nullable
  `birth_commit`.
- Phase 5's backfill writes the same tables, and must set `birth_commit`.
- `marrow explain` reads a line's birth from `lines`, its changes from `fates`,
  and its current position from `snapshot_lines`.
- Bumping the normalizer version makes existing snapshots unusable and requires a
  backfill.
