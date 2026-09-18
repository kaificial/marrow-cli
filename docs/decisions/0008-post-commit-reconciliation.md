# ADR 0008: What a commit reconciles

Status: accepted, and it adds to the PRD's data model (see Consequences)
Date: 2026-09-17

## Context

The write-time hook ([ADR 0007](0007-capture-storage.md)) only sees one file at a
time, and it only fires when an agent writes. That leaves four gaps:

- Code a person writes by hand is never captured.
- A block moved from one file to another looks like a death and a birth, because a
  single file is all the hook can see.
- A file renamed or deleted with git is never noticed.
- Nothing ties any of it to a commit, so there is no clock to measure survival
  against.

Build plan step 4.2 closes all four with a `post-commit` hook. It also has to work
when marrow is installed partway through a repository's life, with a history it has
never seen.

## Decisions

### Reconciliation compares whole trees, not files

`marrow reconcile` puts everything the store knows on one side and the tree HEAD
actually has on the other, then runs the full pipeline: Layer 1 pairs files, Layers
2 and 3 align them, Layer 4 looks for moves across the whole tree. That is what
catches a rename (Layer 1 pairs files by content, so a renamed file is paired with
its old self) and a cross-file move (Layer 4 sees both files at once).

The per-file `compare_file` the hook uses stays as it is. The two entry points now
share one function, `pipeline::compare_snapshots`.

### HEAD's tree is the new state, and the parents decide what the commit did

The committed tree is what a line's fate is read from, because a commit is the only
moment with a timestamp everyone agrees on.

But HEAD alone can't say what *happened*. A file missing from HEAD might have been
deleted, or might belong to a branch we just switched away from, or might be work
that was never committed. So every finding is checked against the commit's parents:

- A stored file that HEAD doesn't have is recorded as deleted only if the commit is
  what removed it, meaning some parent had that path. A root commit deleted nothing,
  and a file no parent had was never this commit's business. Taking *some* parent
  rather than all of them means a merge that resolves a file to "deleted" still
  counts as deleting it.
- A file that is in neither the commit nor the working tree, and that no parent had,
  leaves no evidence at all of what became of its lines. They keep the checkpoint
  they already had, which censors them where they were last seen rather than calling
  them alive or dead.
- A file whose contents differ from our picture, where every parent has exactly
  what HEAD has, was changed by something that isn't a commit: a checkout, a reset,
  a rebase, or work thrown away. The committed version is taken as a new baseline,
  line identities are carried across by the same comparison, and no fates are
  recorded, because nobody performed them.

Without this, ordinary branch work produces nonsense. Writing a file on a branch,
switching away, and committing something else made every line of that file die;
merging the branch back then recorded them all as newly born. Seven real lines were
stored as ten, with three deaths that never happened. Deaths are the measurement,
so a phantom death is the worst error the tool can make.

### When our picture is ahead of HEAD

A file whose stored state matches what is on disk, but not what is in HEAD, is left
exactly as it is: not compared, not replaced, and nothing born from it. The commit
left those changes out — the file wasn't staged, or only part of it was — and the
working tree is what the next write will be compared against.

Its lines are still marked alive, with the time but without the commit. They are
alive because they are on disk; the commit isn't evidence of where they are, so it
isn't recorded as the place they were last seen.

The first version of this compared byte-level content ids, which was wrong in a way
that only shows up on Windows. Where git rewrites line endings, a working copy's
bytes never match its own blob, so every file marrow had captured would look ahead
of HEAD forever and never be reconciled again. The comparison is made on normalized
lines instead — the same token fingerprints the engine matches with — which is blind
to line endings by construction. Content ids are kept only as a shortcut for "this
file hasn't changed at all", where being wrong costs one comparison and nothing else.

### Content ids decide what needs looking at

Each stored snapshot now records the git blob id of the contents it was built from
(`snapshots.content_id`). A file whose committed blob id matches its snapshot is
known to be unchanged without parsing it at all: every line in it is alive, and the
comparison skips it. On a normal commit that is nearly every file, which is what
keeps a per-commit hook cheap.

It also settles an awkward case. A file HEAD still has but tree-sitter can no longer
parse is left exactly as it was, and reported on stderr. Calling its lines dead
would invent deaths that never happened. This is the answer to open question 8 in
[the genealogy spec](../specs/genealogy.md#open-questions-for-review) for
reconciliation.

Two unknown ids must never count as the same contents, so every comparison that can
conclude "unchanged" asks whether the id is a real one first, and an object id marrow
can't represent is an error rather than a placeholder. Without that, a repository
whose ids aren't sha-1 would make every file look like every other file, and every
deletion look like a rename, with nothing reported anywhere.

### The checkpoint lives on the line, not in a fate row

Step 4.2 asks for "a commit-anchored checkpoint in the fates table". Instead,
`lines` gained `last_seen_at` and `last_seen_commit`, updated in place.

A row per line per commit would be the literal reading, but it multiplies the
database by commits × lines and stores the same fact over and over: this line was
still there, unchanged. Survival analysis needs exactly one thing from it, the last
moment a line was known alive, which is what right-censoring is. Keeping it on the
line gives that in one column and one update.

`fates` gained `previous_seen_at` for the other half of the problem. A change found
by reconciliation happened somewhere between the last time we saw the line and this
commit, not at a known instant. Recording both ends makes it interval-censored data
rather than a guess.

### Anything not accounted for is human-written

A line in the commit that no stored line matches is recorded as written by a person,
at that commit. This covers hand-written code, and it also covers an agent write the
hook missed (the hook wasn't installed, or it failed). Attributing an unobserved
write to a person is the conservative direction: it can only understate how much
code agents wrote, never overstate it, which matters because overstating is the
result the tool would be accused of.

An agent-written line a person then edits becomes `agent_then_human_revised`, the
third origin in PRD §6. A line that only *moved* is not a revision: nothing about
what it says has changed.

A dead line also records the line it was most nearly like, resolved to that line's
id once every line in the commit has one, so the evidence the spec asks for is
attached to whole-tree decisions and not only to per-file ones.

### Marrow installed mid-history starts from the tree it finds

With no stored snapshots, every line in HEAD is new to marrow, so every line is
born at that commit, human-written. The history before it is not reconstructed;
that's `marrow backfill`'s job in phase 5. Nothing is recorded as having died,
because nothing was ever seen alive.

### Paths are hashed, so every side has to be hashed too

The store keeps only a hash of each path (PRD §8), so a stored file can't be looked
up by name anywhere. Reconciliation hashes the paths it can see instead — the
commit's, each parent's, and the working tree's — and looks the stored hash up in
those. The working-tree side means walking the tree and hashing what is there;
without it, every scratch or ignored file an agent wrote would be reported as
deleted at the next commit.

### Smaller choices

- Timestamps come from the commit's author date, so reconciliation of an old commit
  dates its findings correctly. A checkpoint never moves backwards, and an interval
  never ends before it starts, in case a commit is dated earlier than a write we
  already captured.
- A line born from a write and killed before any commit keeps a null `birth_commit`.
  It never made it into a commit, and saying otherwise would be untrue.
- Merge commits are reconciled like any other, against HEAD's tree, and recorded
  with `is_merge` so later analysis can exclude them.
- `marrow reconcile --install` writes `.git/hooks/post-commit`, and refuses to touch
  a hook that already exists, printing the line to add instead. Git ignores a
  post-commit hook's exit code, so reconciliation can never fail a commit.
- The store's schema version is now 2, and opening a store built by another version
  fails with a clear message rather than half-upgrading it. Migrations belong with
  backfill in phase 5.
- Content ids are computed from the bytes as they are, so on a repository where git
  rewrites line endings (`core.autocrlf`) a file's stored id won't match its blob id
  and the unchanged shortcut won't fire. Every such file is compared in full instead,
  which costs time and reaches the same answer. Matching git's own filters is a
  phase 5 problem.
- A store that has marrow's tables but no readable schema version is treated as
  version 0 and refused, rather than having the current schema created around it.

## Verification

Fifteen end-to-end tests drive the built binary against real git repositories with
real commits: a commit made by hand, marrow installed mid-history, an agent's lines
keeping their origin and gaining a birth commit, a hand edit to an agent line
becoming the third origin, a file deleted by hand, a file renamed with `git mv`, a
file written but never committed, a branch switch and the merge that follows it, an
edit left out of a commit, reconciling the same commit twice, installing the hook
(twice, and over someone else's), a commit that reconciles itself through the
installed hook, and a repository with no commits yet.

Three of those are regression tests for bugs the tests as first written did not
catch: the branch case, work that vanished before any commit (which was being
re-confirmed alive at every commit afterwards), and a repository that rewrites line
endings (where every captured file was being left unreconciled). The first came out
of running a scratch repository by hand; the other two out of a review of this
change.

Run by hand on a scratch repository with the hook installed: an agent write then a
commit (4 agent lines, anchored to the commit), a file written by hand then
committed (4 human lines), and a hand edit of one agent line (that line became
`agent_then_human_revised`, with one `edited` fate at 0.75 from
`within_hunk_alignment`). A search of the database's bytes for words from the source
found none.

The 27-fixture corpus still passes with 0 errors, so making `compare_snapshots`
public and changing how content identity is represented didn't disturb the engine.

## What this still gets wrong

Worth knowing before the numbers are believed. All three err the same way: they
understate deaths rather than inventing them.

- **Switching between branches repeatedly duplicates lines.** The store keeps one
  last-known state per path, so taking branch A's version as a new baseline drops
  branch B's line identities without recording a fate, and going back to B births
  them again. Fixing it properly means keeping state per branch, which phase 5
  should decide on.
- **A hand revert of an uncommitted agent edit is censored, not recorded dead.**
  From the outside it is indistinguishable from a branch switch: the file differs
  from our picture, and no commit touched it. Code written and thrown away before
  any commit is left censored at its last sighting.
- **A move out of a re-baselined file keeps its identity but records no `moved`
  fate.** Suppressing that file's fates is what makes branch switches safe, and the
  move couldn't be attributed to this commit anyway.

## Consequences

- **PRD §8 needs updating**, on top of the changes ADR 0007 already lists:
  `lines.last_seen_at`, `lines.last_seen_commit`, `fates.previous_seen_at`, and
  `snapshots.content_id`.
- Phase 6's survival curves read `birth_ts`/`birth_commit` for entry, `fates` for
  events with their intervals, and `last_seen_at` for right-censoring.
- `marrow backfill` (phase 5) walks history with the same comparison and must fill
  the same columns, including the checkpoints for commits it replays.
- Reconciliation is idempotent: running it again on the same commit records nothing
  new, because the snapshots it wrote already match the tree.
