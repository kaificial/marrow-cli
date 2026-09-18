# Line genealogy spec


This spec describes how Marrow decides what happened to every line of code
between two commits: whether it was kept, edited, moved, or deleted. Every
survival number Marrow reports depends on these decisions, so the rules are
written out in full here, and every tunable number is listed in one table
(see [Tunable constants](#tunable-constants)).

Four choices in this spec were made by the project owner before it was
written:

- Lines that contain only a comment are tracked. A comment at the end of a code
  line is ignored when matching that line.
- A full rewrite is decided by low similarity, not by hunk size. Hunk size only
  limits how much work alignment does.
- Three formatter behaviors from ADR 0002 get special handling: single lines
  reordered within a file, lines split or joined without changing their
  tokens, and quote, trailing comma, and semicolon changes.
- Files in languages without a grammar are not tracked.

## Terms

- **Commit pair**: a commit and its first parent. Marrow walks the first-parent
  history and runs this pipeline once per commit pair.
- **Old side / new side**: the parent commit and the child commit.
- **Token**: a piece of source text taken from the tree-sitter syntax tree, such
  as an identifier, keyword, operator, or bracket. Layer 0 defines exactly how
  tokens are made.
- **Content token**: a token that contains at least one letter, digit, or
  underscore. `fn`, `parse_item`, and `0` are content tokens. `{`, `;`, and `::`
  are not. Content tokens measure how much evidence a line carries: `}` has
  none, while `use std::io::Read;` has four.
- **Tracked line**: a line with at least one token after normalization. Blank
  lines are not tracked.
- **Fingerprint**: a hash of a line's canonical token sequence. Two lines with
  the same fingerprint are treated as identical.
- **File pair**: an old file and a new file that Layer 1 decided are the same
  file, either at the same path or across a rename.
- **Anchor**: a run of consecutive lines that Layer 2 found unchanged.
- **Hunk**: the old lines and new lines between two anchors.
- **Match**: an old line paired with a new line, with a state of `verbatim`,
  `edited`, or `moved`.
- **Residue**: old and new lines that no layer has matched yet.
- **Fate**: the final decision for an old line in a commit pair: `verbatim`,
  `edited`, or `moved` (with its new position), or `dead`. New lines that nothing
  matched are **born**.

## How the pipeline runs

For each commit pair:

1. **Pick the files.** Compare the two commit trees. Skip files that aren't
   tracked: excluded paths, languages without a grammar, binary or non-UTF-8
   files, and files larger than `MAX_TRACKED_FILE_BYTES`.
2. **Layer 0, normalization**: turn every changed file version into tracked
   lines with tokens, fingerprints, and similarity features.
3. **Layer 1, file correspondence**: decide which old files and new files are the
   same file.
4. **Layer 2, histogram diff**: for each file pair, find the unchanged regions and
   the hunks between them.
5. **Layer 3, within-hunk alignment**: inside each hunk, detect reflowed lines,
   match edited lines, and decide whether the hunk or the whole file was
   rewritten.
6. **Layer 4, move detection**: over everything still unmatched, find lines that
   moved within their file, then lines that moved to another file.
7. **Finalize**: old lines still unmatched are dead. New lines still unmatched
   are born.

Each layer only works on lines the earlier layers left unmatched. A match is
never undone by a later layer, with one exception: when Layer 3 decides a region
was rewritten, it discards the weak matches inside that region (see
[Rewrites](#rewrites)).

## Requirements for every layer

- **Deterministic.** The same two commits always produce the same decisions,
  byte for byte. Every choice between equal options uses a fixed tie-break rule,
  and running work in parallel must never change a result.
- **Evidence on every decision.** Each fate records a similarity score between 0
  and 1, the name of the rule that decided it (see
  [Deciding layer names](#deciding-layer-names)), and, when there was one, the
  position of the best candidate it was compared against. `marrow explain` and
  the `trace` output are built from these records.
- **No source text is stored.** Layer 0 reads source code, but everything it hands
  to the other layers is hashes and counts. Later layers never need the original
  text, so earlier file versions (including snapshots taken by the capture hooks)
  can be compared without storing any code.

## Layer 0: Normalization

Layer 0 turns a file's text into tracked lines that the other layers can compare
without caring about whitespace, formatting style, or trailing comments.

**Input:** the file's bytes and its path.

**Output:** the file's tracked lines, in order. Each one has:

- its line number (counting from 1)
- its canonical tokens, stored as hashes
- a fingerprint: a hash of the canonical token sequence
- its unigram set and bigram multiset, as hashes (used by Layers 1 and 3)
- its content token count
- its kind: `code`, or `comment` if every token on the line comes from a comment
- its syntactic role: the kind of the largest syntax node that starts on the
  line. This is only used for reports, never for matching.

### Which files are read

The language comes from the file extension: `.rs` is Rust, `.py` is Python, and
`.ts`, `.mts`, and `.cts` are TypeScript. `.tsx` uses the TSX grammar from the
same TypeScript package. Every other extension is untracked. The file must be
valid UTF-8, and lines are split on `\n` with any trailing `\r` removed, so CRLF
and LF files produce the same lines. A byte-order mark at the start of the file
is ignored.

### Making tokens

1. Parse the file with the tree-sitter grammar for its language. The grammar
   versions are pinned.
2. Walk the leaves of the syntax tree in order. Skip zero-width leaves: those are
   the tokens tree-sitter inserts when it recovers from a syntax error, and
   Python's indent and dedent markers. Whitespace is never a token, so
   indentation never affects a line.
3. Most leaves become a single token with their exact text.
4. Comment text and string contents are split into word tokens: runs of letters,
   digits, and underscores, with every other non-space character as its own
   token. The comment markers (`//`, `#`, `/*`) and string quotes stay as their
   own tokens. Without this step, a whole comment or string would be one token,
   and changing a single word in it would look like a completely different line.
5. A token belongs to the line it's on. A comment or string that spans several
   lines contributes its words to each of those lines.

Files with syntax errors are still tokenized from whatever tree tree-sitter
builds. There is no switch to a different tokenizer for a broken file, because a
different tokenizer would give different fingerprints and make unchanged lines
look edited.

A few malformed inputs make the TypeScript and TSX grammars loop instead of
finishing, using more and more memory. For example, this 16-byte file never
finishes parsing: `$\'''*[}=r"f"f")`. Parsing a file stops after
`PARSE_TIMEOUT_MICROS`. When that happens, `marrow trace` stops with an error
that names the file and commit, rather than guessing at the file's lines (see
[Open questions](#open-questions-for-review)).

### Canonical tokens

Before hashing, three formatter-only differences are removed, so they don't make
a line look edited:

- **Quote style.** In Python and TypeScript, single-quoted and double-quoted
  strings are treated the same, and so are `'''` and `"""` in Python. Escaped
  quotes inside a string (`\'`, `\"`) are compared as the plain character, and
  Python string prefixes are compared in lowercase (`R"x"` matches `r"x"`).
  TypeScript template literals are left alone. Rust is left alone, because `'a'`
  and `"a"` mean different things there.
- **Trailing commas.** A comma directly before a closing `)`, `]`, or `}` is
  dropped, unless the syntax needs it. The comma in a one-element tuple, `(x,)`
  in Python or Rust, is kept, and so is a comma that follows another comma or an
  opening bracket.
- **TypeScript semicolons.** A semicolon that ends a statement or a class field
  is dropped, and so are the `;` or `,` separators between members of an
  interface or object type. Semicolons inside a `for (...)` header are kept.
  Rust and Python semicolons are never dropped.

Canonical tokens are only used for matching. Content token counts are unaffected,
since quotes, commas, and semicolons are never content tokens.

### Comments

- A line whose tokens all come from comments is a `comment` line. Its fingerprint
  and similarity features are built from its comment tokens, so an edited comment
  is `edited` and a deleted comment is `dead`.
- On a line that has both code and a comment, only the code tokens are used for
  its fingerprint and similarity features. Changing a trailing comment leaves the
  line `verbatim`.
- Comment lines are only ever matched with other comment lines, and code lines
  with code lines.

### Similarity features

- **Unigram set**: the distinct canonical tokens on the line.
- **Bigram multiset**: every pair of neighboring canonical tokens, with a
  start-of-line marker before the first token and an end-of-line marker after
  the last one. The markers give every line at least two bigrams, so short lines
  like `}` can still be compared.

### Normalizer version

The fingerprint of a line depends on these rules and on the grammar versions.
Any change to either bumps the normalizer version. Fingerprints from different
versions are never compared, and stored history built with an older version has
to be rebuilt.

### Known failure modes

- **Half-written code.** Tree-sitter recovers from syntax errors differently
  depending on the surrounding code, so a file with errors can split tokens
  differently near the error. Agent writes that leave a file temporarily broken
  may show a few extra edits.
- **Language changes.** Renaming `x.ts` to `x.tsx` switches grammars, which can
  change tokens on some lines. If the content is identical, every line still
  stays `verbatim` (Layer 2 pairs the lines by line number). If the content also
  changed, a line the two grammars tokenize differently can look edited or die.
- **Untracked files.** Non-UTF-8 files, oversized files, and files in unsupported
  languages are ignored completely. Code moved into or out of them looks like
  births and deaths.
- **Hash collisions.** Two different lines could share a fingerprint. With 64-bit
  hashes compared within a single commit pair, this is negligible.
- **Quote changes that alter meaning.** Treating `'` and `"` as equal in Python
  and TypeScript is safe, but a change between a plain string and a template
  literal in TypeScript still counts as an edit.

## Layer 1: File correspondence

Layer 1 decides which old file and which new file are the same file, so that a
renamed file's lines are compared with their old versions instead of all dying
and being reborn (ADR 0006).

**Input:** the tracked files in both commits (path and content hash), with their
Layer 0 lines.

**Output:** a list of file pairs, each recorded with how it was paired
(`same_path`, `exact_rename`, or `similar_rename`) and its similarity. Old files
with no pair are deleted files, and new files with no pair are added files.

### Decision rule

1. **Same path.** A path that exists in both commits is always paired with
   itself, however much its content changed. A file rewritten in place is
   handled by the rewrite rule in Layer 3.
2. **Exact renames.** A deleted file and an added file with identical content
   are paired, with similarity 1.0.
3. **Similar renames.** For every remaining deleted file and added file, compute
   the Jaccard similarity of their fingerprint sets: the number of distinct
   fingerprints they share, divided by the number of distinct fingerprints in
   either. Only lines with at least one content token go into the sets. Lines
   like `}` appear in nearly every file and would make unrelated small files look
   alike.
4. **Pick pairs.** Go through the candidate pairs from most to least similar.
   Accept a pair if its similarity is at least `FILE_RENAME_MIN_JACCARD` and
   neither file is already paired. Ties go to a pair with the same file name,
   then the same parent directory, then the alphabetically first old path, then
   the alphabetically first new path.

Every line of a deleted file and every line of an added file goes straight to
the residue, where Layer 4 can still find moves.

In the rename-file fixtures, the renamed files score about 0.8 and the unrelated
deleted and added files score at most 0.125. `FILE_RENAME_MIN_JACCARD` has to
fall between those two values.

### Known failure modes

- **Small files.** With only a few distinct fingerprints, one edited line swings
  the similarity a lot. A three-line file that is renamed and edited may not be
  paired.
- **Look-alike files.** When one of several similar files (such as test files
  built from the same template) is deleted and another added, the wrong pair can
  win.
- **Swaps.** If a file is renamed and a new file is created at the old path in
  the same commit, the same-path rule pairs the new file with the old one. The
  renamed content can then only be recovered by Layer 4 as moved blocks, and
  single lines from it die.
- **Copies.** Copying a file leaves the original at its path, so the copy's lines
  are born. A copy is new code as far as survival goes.

## Layer 2: Histogram diff

Layer 2 finds the lines that didn't change in each file pair. It is fast, and it
handles most lines, leaving only the hunks for the more expensive layers.

**Input:** a file pair's old and new tracked lines, compared by fingerprint.

**Output:** anchors (unchanged lines, each with its old and new position) and the
hunks between them, in file order.

### Decision rule

1. **Unchanged files.** If both versions of a file pair have identical content,
   every line is `verbatim` at the same line number in the new file, with score
   1.0.
2. **Histogram diff.** Otherwise, run a histogram diff over the two fingerprint
   sequences, the same algorithm as `git diff --histogram`. Within a region it
   picks the common line that occurs the fewest times as an anchor, extends it to
   the longest matching run, and repeats on the parts before and after that run.
   Lines in anchors are `verbatim` with score 1.0.
3. **No Myers fallback.** Reference implementations switch to the Myers algorithm
   when no common line in a region occurs `HISTOGRAM_MAX_OCCURRENCES` times or
   fewer. Marrow doesn't. That region stays a single hunk and goes to Layer 3.
4. **Slide changed blocks.** Two diffs can be equally short but disagree about
   which of two identical lines changed. When a function is deleted from between
   two others, the `}` that closes it looks the same as the `}` that closes the
   function above it, and the diff can report either one as deleted. Marrow
   settles this the way git does. Each block of inserted or deleted lines is
   shifted as far down as identical lines allow, unless shifting it back up lines
   it up with a changed block on the other side. Git also has an indent heuristic
   that looks at blank lines and indentation. Marrow doesn't use it, because
   normalized lines no longer carry indentation.
5. **Weak anchors.** An anchor run is weak if its lines have
   `WEAK_ANCHOR_MAX_CONTENT_TOKENS` content tokens or fewer in total, and it has
   a hunk on both sides. A weak anchor's lines are put back, and the two hunks and
   the anchor are merged into one hunk. Repeat until no weak anchors remain.

Weak anchors are why a `}` shared by chance doesn't split a rewritten function in
two. After merging, Layer 3 sees the whole rewritten region at once. If the region
wasn't rewritten, Layer 3 matches the `}` again anyway, since identical lines
score 1.0.

### Known failure modes

- **Chance matches between unique lines.** A distinctive line that appears in both
  versions by coincidence, like a shared import in a rewritten file, is a strong
  anchor. Only the file-level rewrite check in Layer 3 can overrule it.
- **Which side moved.** When a large block moves past a small one, the diff may
  keep the large block in place and report the small one as moved, even if the
  author actually moved the large one. The fixtures avoid symmetric cases, but
  real history has them.
- **Repeated blocks.** Several identical blocks (like similar test cases) give the
  diff no low-count lines to anchor on, so reordering them can show up as edits
  or moves in the wrong place.
- **Repeated first lines.** Shifting a block down picks the wrong copy when a
  deleted block starts with the same line as the block below it, like the same
  decorator on two functions in a row. The deleted function's decorator is kept,
  and the decorator of the function below it dies. The number of surviving lines
  is still right, but one of them gets the wrong birth.

## Layer 3: Within-hunk alignment

Layer 3 works out what happened inside each hunk: which lines were only reflowed,
which were edited, and whether the hunk, or the whole file, was rewritten.

**Input:** one hunk at a time (old lines and new lines, with their Layer 0
features), then the file pair as a whole.

**Output:** matched lines (`verbatim` or `edited`, with scores), lines marked as
part of a rewrite, and the remaining unmatched lines, which go to the residue.

### Step 1: Empty sides

If a hunk has no old lines, its new lines go to the residue. If it has no new
lines, its old lines go to the residue. Nothing else happens to that hunk.

### Step 2: Scoring candidate pairs

For each old line and new line of the same kind (code with code, comment with
comment):

- Identical fingerprints score 1.0.
- If the Jaccard similarity of their unigram sets is below
  `PREFILTER_MIN_UNIGRAM_JACCARD`, they are not candidates, and their Dice score is
  never computed. This only changes which lines get matched if a rejected pair would
  have scored `EDIT_MIN_DICE` or better, and calibration has to confirm that never
  happens on a fixture. It does affect the evidence stored for a line that dies:
  see [Finalizing fates](#finalizing-fates).
- Otherwise, their score is the Dice coefficient of their bigram multisets: twice
  the number of shared bigrams, divided by the total number of bigrams on both
  lines.
- A pair scoring at least `EDIT_MIN_DICE` is a candidate.

For example, `mod inventory;` and `mod stock;` share 2 of their 4 bigrams each
(start-of-line + `mod`, and `;` + end-of-line), so they score exactly 0.5.

### Step 3: Reflowed lines

A reflow group is a run of consecutive old lines and a run of consecutive new
lines whose canonical tokens, joined end to end, are identical, but whose line
breaks differ. This is what a formatter does when it splits a long call over
several lines, or joins a short one back together.

- A group is found by reading tokens from an old line and a new line side by
  side, moving on to the next line on whichever side runs out first. The group
  ends the first time both sides reach the end of a line at the same token. If
  the tokens differ before that, there is no group.
- A group needs more than one line on at least one side. Two single lines with
  the same tokens are simply identical.
- A group may not have more than `REFLOW_MAX_GROUP_LINES` lines on either side,
  and all its lines must be the same kind.

Ending at the first shared line end keeps a group from swallowing an unchanged
line. When `foo(a,` / `b)` / `bar()` becomes `foo(a, b)` / `bar()`, the group
is the first two old lines and the first new line, and `bar()` stays a separate,
identical line.

Groups are not picked before alignment. Each one is another candidate in Step 4,
worth 1.0, the same as a pair of identical lines, so a group only wins when no
better line-by-line matches compete with it. Picked first, a group that sits far
from where its lines belong could leave a stretch of edited lines with nothing to
match.

When a group is part of the chosen alignment:

- The first old line in the group is `verbatim` at the first new line, with score
  1.0. Only line breaks changed.
- Every other old line in the group is dead, recorded as `reflow`. Its score is
  its similarity to the first new line of the group.
- Every other new line in the group is born.

A split or join that also changes a token is not a reflow group. Those lines go
through normal alignment, and usually end up dead and born.

### Step 4: Alignment

Choose the non-crossing set of candidates with the highest total score: both old
and new positions strictly increase, so matched lines never cross. The candidates
are the pairs from Step 2 and the groups from Step 3. This is Needleman-Wunsch
alignment with no gap penalty, restricted to candidates. Every score in a hunk is
scaled to a whole number, by the least common multiple of the scores'
denominators, so alignments with equal totals really tie instead of differing in
the last bit of a sum. For a hunk whose denominators are too large to scale that
way, scores are rounded down instead, and two totals can then differ by the
rounding.

- If several alignments have the same total, prefer the one whose pairs sit
  closest to the hunk's diagonal. For old line i of m and new line j of n
  (counting from 0), a pair's distance is |(2i + 1)·n − (2j + 1)·m|, which
  compares the midpoints of the two lines within the hunk. Midpoints treat an
  insertion at the top of a hunk the same as one at the bottom. A group's
  distance is that of its first old line and first new line.
- If that still ties, earlier lines win. Working back from the end of the hunk,
  alignment leaves an old line unmatched before a new line, and matches a pair
  only when skipping it would lower the total or move matches off the diagonal.
  This keeps identical lines like repeated `return Err(...)` lines matched to
  their neighbors rather than to a copy further down, and agrees with Layer 2,
  which slides changed blocks down.
- If the hunk has more than `HUNK_FULL_ALIGNMENT_MAX_CELLS` old-and-new line
  combinations, only candidates within `HUNK_ALIGNMENT_BAND_LINES` lines of the
  diagonal are considered, counted in lines of the hunk's longer side. When one
  side is more than `HUNK_ALIGNMENT_BAND_LINES` times longer than the other, the
  band widens to the longer side's length, because a narrower one would leave no
  path across the hunk at all. Large hunks are never treated as rewrites just for
  being large, so a variable renamed across 1,000 lines still comes out as 1,000
  edited lines.

A matched pair with identical fingerprints is `verbatim`. Any other matched pair
is `edited`. Both keep their score.

### Rewrites

After alignment, Layer 3 checks each hunk, and then each file pair as a whole,
for a rewrite. A rewrite means the old content was replaced rather than edited,
so lines that happen to match (a `}`, a common import) shouldn't count as
survivors (ADR 0003).

**Retention** measures how much of one side the matches explain. For the old
side, add up each matched line's content token count multiplied by its match
score, then divide by the total content tokens of all old lines in the region.
Retention for the new side is computed the same way. Every line of a chosen
reflow group counts as explained, with score 1.0. At the file level, anchors
from Layer 2 count as matches with score 1.0.

A region is a rewrite when both sides have content tokens and both retentions are
below `REWRITE_MAX_RETENTION`. Requiring both sides keeps a large deletion or a
large addition from counting as a rewrite: deleting most of a file leaves the new
side fully explained.

Inside a rewrite:

- A run of consecutive identical matches survives as `verbatim`, but only if it
  has at least `MIN_MOVE_BLOCK_LINES` lines and `MIN_MOVE_BLOCK_CONTENT_TOKENS`
  content tokens. That's the same bar a block has to clear to count as moved, so
  kept code survives and chance matches don't. A surviving match keeps the layer
  that made it.
- Every other match in the region is discarded.
- Every line in the region outside a surviving run, old or new, matched or not,
  goes to the residue marked as a rewrite line. Marking the whole region, not
  just the discarded matches, keeps Layer 4 from pulling a stray line out of a
  rewrite as a single-line move.
- Layer 4 can still claim rewrite lines as part of a moved block, so a function
  moved out of a rewritten file is found. They can't be claimed as single-line
  moves.
- Rewrite lines that nothing claims are dead, recorded as `hunk_rewrite` or
  `file_rewrite`. Their score is the higher of their discarded match's score and
  their best candidate score.

The hunk check runs first, for each hunk. The file check runs once all hunks in
the file pair are done, and it can overrule anchors, which is how a rewritten file
that keeps one identical import still has every old line die.

Estimated by hand, retention in the full-rewrite fixtures is around 0.15 to 0.25. In the
rename-file Rust fixture, the hunk holding `mod stock;` and
`use stock::parse_item;` keeps about 0.6. `REWRITE_MAX_RETENTION` has to fall
between those values.

### Known failure modes

- **Short lines.** A one-token change in a three-token line scores 0.5, the same
  as two unrelated lines with the same shape (`mod legacy;` and `mod report;`).
  Short lines are the most likely to be matched wrongly or missed.
- **Many weak matches.** Alignment maximizes the total score, so two mediocre
  matches can win over one excellent match that would cross them.
- **Edit plus reflow.** A line that is edited and split or joined at the same time
  is not a reflow group, and is usually recorded as dead and born.
- **Reflow against edits.** A reflow group counts as one match worth 1.0, so it
  loses to two or more edited matches that cross it, even when the reflow is real.
- **Duplicates at the end of a hunk.** The diagonal tie-break can send a line to
  a later identical copy instead of the neighbouring one. With two identical
  guard clauses and a third copy appended at the end of the hunk, the second
  guard's lines match the appended copy and their real neighbours are born. The
  number of surviving lines is right, but two of them get the wrong position.
- **Heavy edits look like rewrites.** A region where most lines were heavily
  edited, not just renamed, can fall below `REWRITE_MAX_RETENTION`, and all of it
  dies.
- **Band limit.** In a very large hunk, a line that moved further than
  `HUNK_ALIGNMENT_BAND_LINES` from the diagonal can't be matched here. Layer 4
  can still find it if it's part of a large enough block.

## Layer 4: Move detection

Layer 4 looks through everything still unmatched in the commit pair for code that
was moved without being changed.

**Input:** the residue from every file: old lines from file pairs and deleted
files, and new lines from file pairs and added files. Rewrite lines are marked.

**Output:** `moved` matches, each with score 1.0. Everything left over goes to
finalization.

A moved line must have exactly the same fingerprint in both places. Lines that
were moved and edited at the same time are out of scope for v1 (see
[Out of scope](#out-of-scope-for-v1)).

The other old lines of a chosen reflow group are already dead, and the other new
lines of a group are already born, so they aren't residue and Layer 4 never
claims them.

### What counts as a block

A block is a run of old residue lines that are consecutive among the old file's
tracked lines, matching a run of new residue lines that are consecutive among the
new file's tracked lines, with the same fingerprints in the same order. Blank
lines between them don't break a block, but a line that was already matched does.

A block is only accepted if it has at least `MIN_MOVE_BLOCK_LINES` lines and
`MIN_MOVE_BLOCK_CONTENT_TOKENS` content tokens. Short runs of common lines, like
`}` followed by `}`, match between unrelated places all the time, and treating
those as moves would make code look like it survived when it didn't.

### Finding and accepting blocks

Steps 1 and 2 find blocks the same way, and differ only in which files they
compare.

1. Index the new residue lines by fingerprint.
2. Take each old residue line as a seed. Skip seeds whose fingerprint appears on
   more than `MOVE_SEED_MAX_CANDIDATES` new residue lines the step may use; they
   are too common to be useful seeds. Blocks can still extend through such lines.
3. For each new line with the same fingerprint, extend the match backwards and
   forwards while both sides stay consecutive, unmatched, and identical.
4. Accept blocks one at a time, best first:
   - more lines;
   - then more content tokens;
   - then the smallest gap between the block's old and new starting positions,
     counted in tracked lines;
   - then the earliest old file by path, then the earliest start in that file;
   - then the earliest new file by path, then the earliest start in that file.

   Once a block is accepted, its lines can't be part of another block. The rest of
   a block that overlapped it is still a candidate if it still meets the size
   limits.

### Step 1: Blocks within a file

Find blocks between the old residue and the new residue of the same file pair.
Order does not have to be preserved, which is what makes it a move.

### Step 2: Blocks across files

Find blocks between old residue lines and new residue lines of different files,
anywhere in the commit. Different files means not the same file pair: an old file
can send a block to any new file except its own new version, and a deleted file
can send one to any new file.

### Step 3: Single reordered lines within a file

An old residue line is `moved` to a new residue line in the same file pair when
all of these are true:

- neither line is a rewrite line
- its fingerprint appears exactly once among all tracked lines of the old file,
  and exactly once among all tracked lines of the new file
- it has at least `SINGLE_LINE_MOVE_MIN_CONTENT_TOKENS` content tokens

This catches imports sorted by a formatter, and single statements that were moved
on their own. Both files contain the line exactly once, so it is the same line.

Single lines come last, after both block steps, because a block is the stronger
evidence. Claiming a line on its own first could split a block leaving the file
into pieces too small to count.

### Known failure modes

- **Moved and edited.** A block with one edited line splits into smaller blocks
  around that line. The parts still large enough are moved, the edited line dies,
  and its new version is born.
- **Duplicated code.** If a block was copied rather than moved, the original stays
  matched where it is and the copy is born. If two identical blocks were deleted
  and one was added elsewhere, the tie-breaks pick which one moved.
- **Small moves.** A moved group of lines smaller than the block limits, other than
  a single unique line, is recorded as dead and born.
- **Blocks of nothing but common lines.** The seed limit is a cliff. When every
  line of a block appears on more than `MOVE_SEED_MAX_CANDIDATES` new residue
  lines, no seed survives and the whole move is lost, even though the block itself
  is long enough. With 64 identical four-line blocks every line moves; with 65 none
  of them do. The `boilerplate` fixture should pin this before the sweep.

## Finalizing fates

After Layer 4:

- Old lines that are still unmatched are `dead`. A rewrite line gets
  `hunk_rewrite` or `file_rewrite` as its deciding layer. Any other unmatched line
  gets `no_match`.
- A dead line's score is the best score it reached against any line it was actually
  scored against in Layer 3, including scores below `EDIT_MIN_DICE`, or 0 if it was
  never scored against anything. A line the prefilter rejected was never scored, so
  a dead line can be recorded with 0 and no candidate even though some barely
  similar line existed. That is the price of the prefilter: computing the score
  anyway, just to report it, is the work the prefilter exists to avoid.
  The position of that best candidate is recorded too. A rewrite line uses the
  higher of that score and its discarded match's score. The other old lines of a
  reflow group use their similarity to the group's first new line.
- New lines that are still unmatched are born.

Every old line gets exactly one fate, and every new line is either matched by
exactly one old line or born.

### Deciding layer names

These are the values of `deciding_layer` in stored fates and in `marrow trace`
output.

| Name | Layer | Used for |
|---|---|---|
| `unchanged_file` | 2 | `verbatim` lines in a file pair whose content didn't change |
| `histogram_diff` | 2 | `verbatim` lines in an anchor |
| `reflow` | 3 | the `verbatim` first line of a reflow group, and its other old lines, which are `dead` |
| `within_hunk_alignment` | 3 | `verbatim` or `edited` lines matched inside a hunk |
| `hunk_rewrite` | 3 | `dead` lines in a hunk that was rewritten |
| `file_rewrite` | 3 | `dead` lines in a file that was rewritten |
| `intra_file_move` | 4 | `moved` lines within the same file pair, both blocks and single lines |
| `cross_file_move` | 4 | `moved` lines that went to another file |
| `no_match` | none | `dead` lines that no layer could match |

Layer 1 doesn't decide any single line's fate. Its decisions (which files were
paired, how, and with what similarity) are recorded per file pair, so
`marrow explain` can show that a line survived through a rename.

## Tunable constants

Every tunable number in this spec is listed here. The defaults are starting points
for the calibration sweep in build plan phase 3, not final choices. For each
constant, the sweep reports fixture results across a range of values, and the
project owner picks the value (build plan standing rules).

"Calibrated by" says which fixtures limit the value, and in which direction. Where
no fixture limits a value yet, the missing fixture is named in
[Fixtures this spec needs](#fixtures-this-spec-needs).

| Name | Layer | Default | Calibrated by | Why this default |
|---|---|---|---|---|
| `MAX_TRACKED_FILE_BYTES` | all | 1,048,576 (1 MiB) | No fixture. Set by timing on real repos. | Hand-written source files are almost never this big. Larger files are usually generated, and parsing and aligning them is slow. |
| `PARSE_TIMEOUT_MICROS` | 0 | 10,000,000 (10 seconds) | No fixture. It isn't a research choice, just a guard against grammar bugs. | The timeout only covers parsing. A 1 MiB TypeScript file (the largest tracked size) parsed in 0.55 seconds in a debug build and 0.18 seconds in release, so 10 seconds leaves room for a slow or busy machine. |
| `GENERATED_PATH_PATTERNS` | all | The list in [Out of scope](#out-of-scope-for-v1) | No fixture. | Common locations for vendored and generated files in Rust, Python, and TypeScript projects. |
| `FILE_RENAME_MIN_JACCARD` | 1 | 0.5 | rename-file: the renamed files (about 0.8) must be paired, and the unrelated deleted and added files (0.125 at most) must not. | It's git's default for rename detection (50% similar), and it sits in the middle of the range the fixtures allow. |
| `HISTOGRAM_MAX_OCCURRENCES` | 2 | 64 | No fixture. Only affects performance on very repetitive regions. | The limit JGit's histogram diff uses. |
| `WEAK_ANCHOR_MAX_CONTENT_TOKENS` | 2 | 2 | full-rewrite needs at least 0, so lone braces are merged. No fixture sets an upper limit yet. | Lines like `}`, `} else {`, and `return None` are too common to prove two hunks are separate. Merging costs nothing when the hunks weren't rewritten. |
| `REFLOW_MAX_GROUP_LINES` | 3 | 20 | No fixture yet (needs the reflow fixture). | A formatted statement rarely spans more than 20 lines, and the limit keeps the search small. Set it to 1 to turn reflow off; 0 is not a valid value. |
| `PREFILTER_MIN_UNIGRAM_JACCARD` | 3 | 0.2 | Must not reject any fixture's edited pair. The lowest is 0.5, `mod inventory;` to `mod stock;`. Calibration checks that results are identical with the prefilter turned off. | Lines that share less than a fifth of their distinct tokens practically never score 0.5, so skipping them only saves time. |
| `EDIT_MIN_DICE` | 3 | 0.5 | rename-identifier, rename-file, and duplicate-heavy set the upper limit: `mod inventory;` to `mod stock;` scores exactly 0.5 and must match. No fixture sets a lower limit yet (needs the replaced-line fixture). | "At least half of the token pairs survive" is easy to reason about. It sits exactly on the upper limit, so the sweep may settle a little lower. |
| `HUNK_FULL_ALIGNMENT_MAX_CELLS` | 3 | 250,000 | No fixture (every fixture hunk is far smaller). Set by timing on real repos. | Roughly a 500 by 500 line hunk, which full alignment handles in well under a second. |
| `HUNK_ALIGNMENT_BAND_LINES` | 3 | 100 | No fixture. Set by timing and by spot checks of large refactors. | Mechanical edits keep lines near the diagonal. 100 lines leaves room for insertions and deletions inside a big hunk. |
| `REWRITE_MAX_RETENTION` | 3 | 0.4 | full-rewrite (about 0.15 to 0.25, estimated by hand) must fall below it. The rename-file Rust `main.rs` hunk (about 0.6) and every other edited hunk must stay above it. | The middle of the range the fixtures allow: a region counts as rewritten only if matches explain less than 40% of its content on both sides. |
| `MIN_MOVE_BLOCK_LINES` | 3, 4 | 3 | rename-file and full-rewrite share single lines that must not move, so at least 2. No fixture sets an upper limit: the move fixtures still pass with the limit raised to 6, because every line of their moved blocks is unique in both versions of the file and Step 3 moves it on its own. A fixture whose moved block repeats lines would pin it (see [Fixtures this spec needs](#fixtures-this-spec-needs)). | One above the chance matches and one below the smallest real move in the fixtures. |
| `MIN_MOVE_BLOCK_CONTENT_TOKENS` | 3, 4 | 8 | The smallest moved block in the fixtures has 15 content tokens, so at most 15. No fixture sets a lower limit yet (needs the boilerplate fixture). | Keeps runs of braces and one-word lines (`}`, `else:`, `break;`) from counting as moves. Any real function clears it easily. |
| `SINGLE_LINE_MOVE_MIN_CONTENT_TOKENS` | 4 | 2 | No fixture sets the value yet (needs the sorted-imports fixture). full-rewrite checks that rewrite lines are never moved this way. | Appearing exactly once in both files is already strong evidence. 2 lets `import sys` count and leaves out one-word lines like `pass`. |
| `MOVE_SEED_MAX_CANDIDATES` | 4 | 64 | No fixture. Only affects performance. | A line that appears more than 64 times among unmatched new lines is boilerplate and a poor starting point. Blocks can still extend through it. |

## Out of scope for v1

### Lines that are moved and edited in the same commit

**What happens:** Layer 4 only matches identical lines, so a line that moved and
also changed is recorded as dead where it was and born where it is now.

**Why:** Matching changed lines across every file in a commit means comparing each
unmatched old line with each unmatched new line, which is slow on large commits.
It also gives chance matches a much bigger space to appear in, and it would need
its own similarity threshold with no fixtures to set it. For v1 it's safer to
undercount survival here than to overcount it with false matches.

**Consequence:** Refactors that move code and tweak it at the same time look like
deaths. If agents do this more often than people, it biases the comparison, so
it's worth measuring during dogfooding.

### Merge commits

**What happens:** Marrow only follows first parents. A merge commit is compared
with its first parent like any other commit, so everything the merged branch
added appears to be born at the merge, with the merge's timestamp.

**Why:** Deciding which parent each line came from takes a three-way comparison,
and the solo, mostly linear histories Marrow targets rarely need it. For code
written by Claude, the capture hooks (phase 4) record when each line was actually
written, independent of merges.

**Consequence:** Lines written on a branch and merged later look younger than they
are. Rebased and squashed histories aren't affected.

### Generated and vendored files

**What happens:** Files whose repo-relative path matches `GENERATED_PATH_PATTERNS`
are never tracked. The default list:

- `**/vendor/**`
- `**/third_party/**`
- `**/node_modules/**`
- `**/target/**`
- `**/dist/**`
- `**/build/**`
- `**/__generated__/**`
- `**/*.generated.*`
- `**/*.min.js`
- `**/*_pb2.py` and `**/*_pb2_grpc.py`
- `.marrow/**`

**Why:** Nobody wrote this code by hand, and no agent wrote it either. Its churn
measures the tool that generated it, and large generated diffs would dominate both
the statistics and the running time.

**Consequence:** Generated files outside these paths are still tracked. The list
can be extended per repo. Detecting generated files from their content, such as
an `@generated` header, is left for later.

### Other exclusions

- Files in languages without a grammar, binary files, non-UTF-8 files, and files
  larger than `MAX_TRACKED_FILE_BYTES` (see Layer 0).
- Symlinks and git submodules.
- A line that is edited and reflowed at the same time (see Layer 3).

## Fixtures this spec needs

### Which layers each existing fixture needs

| Fixture class | Layers needed to pass |
|---|---|
| format-only | 0, 2 |
| reindent | 0, 2 |
| delete-function | 0, 2 |
| rename-identifier | 0, 2, 3 |
| duplicate-heavy | 0, 2, 3 |
| full-rewrite | 0, 2, 3 |
| rename-file | 0, 1, 2, 3 |
| move-within-file | 0, 2, 4 |
| move-across-files | 0, 1, 2, 3, 4 |

This doesn't fully match the build plan. Step 3.1 expects move-across-files to
pass after Layers 0 to 2, but the moved function needs Layer 4 and the edited
import needs Layer 3. Step 3.3 lists delete-function as passing after Layer 4, but
it should already pass after Layer 2.

### New fixtures to add before calibration

Several rules and limits in this spec aren't tested by any fixture yet. These
fixtures would cover them, each in Rust, Python, and TypeScript:

| Fixture | What it checks | Calibrates |
|---|---|---|
| sorted-imports | Imports reordered by a formatter are `moved`, not dead and born | `SINGLE_LINE_MOVE_MIN_CONTENT_TOKENS` |
| reflow | Lines split, joined, and re-wrapped without token changes keep one survivor per group. A line that is split and edited at the same time dies. | `REFLOW_MAX_GROUP_LINES` and the reflow rule |
| formatter-tokens | Quote style, trailing comma, and TypeScript semicolon changes are `verbatim`. Rust `'a'` to `"a"` and Python `(x,)` to `(x)` are `edited`. | Canonical token rules |
| replaced-line | A line replaced in place by an unrelated line with a similar shape is dead and born, not edited | Lower limit of `EDIT_MIN_DICE` |
| boilerplate | Unrelated files that share a short run of braces and one-word lines don't produce a move. Also a block long enough to move whose every line appears on more than `MOVE_SEED_MAX_CANDIDATES` new residue lines, which no seed survives today | Lower limit of `MIN_MOVE_BLOCK_CONTENT_TOKENS`, and the `MOVE_SEED_MAX_CANDIDATES` cliff |
| duplicate-block-move | A block moved within a file whose lines each appear more than once in the file, so single-line moves can't rescue them and only the block rules can | Upper limits of `MIN_MOVE_BLOCK_LINES` and `MIN_MOVE_BLOCK_CONTENT_TOKENS` |
| partial-rewrite | One function rewritten inside a kept file: its chance matches die, the rest of the file stays `verbatim`, and a function moved out of it to another file is `moved` | `WEAK_ANCHOR_MAX_CONTENT_TOKENS` and the hunk-level rewrite check |

## Handoffs to later phases

- **"Effectively rewritten" deaths.** PRD §6 defines a second kind of death: a line
  whose total change from its original version crosses a threshold. Genealogy
  scores each change against the previous version, not the original. The survival
  statistics in phase 6 need a line's birth version to compute that, so the
  hashed features from Layer 0 have to be kept for each line's birth version.
- **Storage.** Layer 1 file pairs, and the best candidate position for dead lines,
  need somewhere to live for `marrow explain`. The four tables in PRD §8 don't
  have a place for file pairs yet.
- **Keyed hashes.** This spec only requires that stored data is hashes. Short lines
  like `return None` are easy to guess from their hash. Keying the hash with a
  per-repo secret would stop someone who has the database from confirming a
  guessed line. That belongs in the storage design in phase 5.

## Open questions for review

These are smaller calls made while writing this draft. Each one is easy to change
before implementation.

1. **Reflow survivors.** The first line of a reflow group is `verbatim`, since only
   line breaks changed. It could be recorded as `edited` instead.
2. **Retention weights.** Retention weighs each matched line by its content tokens
   times its score. Counting lines would be simpler, but then a matched `}` would
   carry as much weight as a full statement.
3. **Alignment objective.** Alignment maximizes the total score, which can prefer
   two mediocre matches over one strong match that crosses them. Scoring each
   match by how far it clears `EDIT_MIN_DICE` would favor strong matches instead.
4. **Layer 1 similarity sets.** Lines with no content tokens are left out, so
   braces don't make unrelated small files look alike.
5. **Dead lines with no match.** They're labeled `no_match` rather than blamed on
   the last layer that looked at them, since that layer usually isn't the one at
   fault.
6. **`EDIT_MIN_DICE` default.** At 0.5 it sits exactly on the limit set by
   `mod inventory;` to `mod stock;`. A slightly lower default would give the
   fixtures some margin, at a higher risk of false edits.
7. **Build plan fixture expectations.** Steps 3.1 and 3.3 don't match the table in
   [Which layers each existing fixture needs](#which-layers-each-existing-fixture-needs).
8. **Files the parser gives up on.** `marrow trace` stops with an error. That's safe
   while only the grading tool runs it, but once `marrow backfill` exists (phase 5),
   one such file anywhere in a repo's history would block the whole repo. The
   alternatives are to skip that file version, so its lines die and are reborn, or
   to treat the file as unchanged until a version parses. `marrow reconcile` already
   takes the second option: it leaves the file's stored lines exactly as they were
   and says so on stderr, because calling them dead would invent deaths that never
   happened ([ADR 0008](../decisions/0008-post-commit-reconciliation.md)). Backfill
   should probably do the same; trace still can't, because a file missing from the
   middle of a history looks like every line in it dying.
