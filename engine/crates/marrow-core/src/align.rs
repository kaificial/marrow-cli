
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::ops::Range;

use crate::constants::{
    EDIT_MIN_DICE, HUNK_ALIGNMENT_BAND_LINES, HUNK_FULL_ALIGNMENT_MAX_CELLS,
    MIN_MOVE_BLOCK_CONTENT_TOKENS, MIN_MOVE_BLOCK_LINES, PREFILTER_MIN_UNIGRAM_JACCARD,
    REFLOW_MAX_GROUP_LINES, REWRITE_MAX_RETENTION,
};
use crate::correspondence::jaccard;
use crate::histogram::Segment;
use crate::normalize::Line;
use crate::pipeline::{DecidingLayer, State};

/// Above this, scaling every score in a hunk to a whole number would overflow, so scores are
/// rounded down instead and two totals can differ by the rounding.
const MAX_EXACT_SCALE: u128 = 1_000_000_000_000_000_000_000_000_000_000;

/// Fixed-point scale for comparing a single score against EDIT_MIN_DICE.
const CANDIDACY_SCALE: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rewrite {
    Hunk,
    File,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OldLine {
    Matched {
        new: usize,
        state: State,
        similarity: f64,
        layer: DecidingLayer,
    },
    ReflowDead {
        similarity: f64,
        new: usize,
    },
    Residue {
        similarity: f64,
        best_candidate: Option<usize>,
        rewrite: Option<Rewrite>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum NewLine {
    Matched,
    ReflowBorn,
    Residue { rewrite: Option<Rewrite> },
}

/// What Layers 2 and 3 decided for every tracked line of one file pair, by line index.
#[derive(Debug)]
pub struct FileAlignment {
    pub old: Vec<OldLine>,
    pub new: Vec<NewLine>,
    best: Vec<Option<(f64, usize)>>,
}

impl FileAlignment {
    fn new(old_len: usize, new_len: usize) -> FileAlignment {
        FileAlignment {
            old: vec![
                OldLine::Residue {
                    similarity: 0.0,
                    best_candidate: None,
                    rewrite: None,
                };
                old_len
            ],
            new: vec![NewLine::Residue { rewrite: None }; new_len],
            best: vec![None; old_len],
        }
    }
}

/// Identical bytes keep every line verbatim at the same line number. Lines are paired by number
/// because a grammar change, `.ts` to `.tsx`, can track a slightly different set of lines.
pub fn unchanged_file(old: &[Line], new: &[Line]) -> FileAlignment {
    let mut alignment = FileAlignment::new(old.len(), new.len());
    let mut new_lines = new.iter().enumerate().peekable();
    for (i, old_line) in old.iter().enumerate() {
        while new_lines
            .next_if(|(_, line)| line.number < old_line.number)
            .is_some()
        {}
        if let Some((j, _)) = new_lines.next_if(|(_, line)| line.number == old_line.number) {
            alignment.old[i] = OldLine::Matched {
                new: j,
                state: State::Verbatim,
                similarity: 1.0,
                layer: DecidingLayer::UnchangedFile,
            };
            alignment.new[j] = NewLine::Matched;
        }
    }
    alignment
}

pub fn align_file(old: &[Line], new: &[Line], segments: &[Segment]) -> FileAlignment {
    let mut alignment = FileAlignment::new(old.len(), new.len());
    for segment in segments {
        match segment {
            Segment::Anchor { old: o, new: n } => {
                for (i, j) in o.clone().zip(n.clone()) {
                    alignment.old[i] = OldLine::Matched {
                        new: j,
                        state: State::Verbatim,
                        similarity: 1.0,
                        layer: DecidingLayer::HistogramDiff,
                    };
                    alignment.new[j] = NewLine::Matched;
                }
            }
            Segment::Hunk { old: o, new: n } => {
                if !o.is_empty() && !n.is_empty() {
                    align_hunk(&mut alignment, old, new, o.clone(), n.clone());
                    if is_rewrite(&alignment, old, new, o.clone(), n.clone()) {
                        apply_rewrite(
                            &mut alignment,
                            old,
                            new,
                            o.clone(),
                            n.clone(),
                            Rewrite::Hunk,
                        );
                    }
                }
            }
        }
    }
    if is_rewrite(&alignment, old, new, 0..old.len(), 0..new.len()) {
        apply_rewrite(
            &mut alignment,
            old,
            new,
            0..old.len(),
            0..new.len(),
            Rewrite::File,
        );
    }
    alignment
}

/// Both sides have content, and matches explain less than REWRITE_MAX_RETENTION of each.
fn is_rewrite(
    alignment: &FileAlignment,
    old: &[Line],
    new: &[Line],
    old_range: Range<usize>,
    new_range: Range<usize>,
) -> bool {
    let mut new_scores = vec![0.0; new_range.len()];
    let old_retention = retention(old_range.map(|i| {
        let score = match &alignment.old[i] {
            OldLine::Matched {
                new: j, similarity, ..
            } => {
                new_scores[j - new_range.start] = *similarity;
                *similarity
            }
            OldLine::ReflowDead { .. } => 1.0,
            OldLine::Residue { .. } => 0.0,
        };
        (old[i].content_tokens, score)
    }));
    let new_retention = retention(new_range.clone().map(|j| {
        let score = match alignment.new[j] {
            NewLine::ReflowBorn => 1.0,
            _ => new_scores[j - new_range.start],
        };
        (new[j].content_tokens, score)
    }));
    matches!(
        (old_retention, new_retention),
        (Some(old), Some(new)) if old < REWRITE_MAX_RETENTION && new < REWRITE_MAX_RETENTION
    )
}

/// Content-weighted share of a region explained by matches, or None if it has no content.
fn retention(lines: impl Iterator<Item = (u32, f64)>) -> Option<f64> {
    let (mut explained, mut total) = (0.0, 0u64);
    for (content_tokens, score) in lines {
        explained += f64::from(content_tokens) * score;
        total += u64::from(content_tokens);
    }
    (total > 0).then(|| explained / total as f64)
}

fn apply_rewrite(
    alignment: &mut FileAlignment,
    old: &[Line],
    new: &[Line],
    old_range: Range<usize>,
    new_range: Range<usize>,
    kind: Rewrite,
) {
    let kept_old = surviving_runs(alignment, old, new, old_range.clone());
    let mut kept_new = vec![false; new_range.len()];
    for i in old_range.clone() {
        if kept_old[i - old_range.start] {
            if let OldLine::Matched { new: j, .. } = alignment.old[i] {
                kept_new[j - new_range.start] = true;
            }
            continue;
        }
        let (mut similarity, mut best_candidate) = alignment.best[i]
            .map_or((0.0, None), |(similarity, candidate)| {
                (similarity, Some(candidate))
            });
        let (earlier, earlier_candidate) = match alignment.old[i] {
            OldLine::Matched {
                new, similarity, ..
            }
            | OldLine::ReflowDead { similarity, new } => (similarity, Some(new)),
            OldLine::Residue {
                similarity,
                best_candidate,
                ..
            } => (similarity, best_candidate),
        };
        if earlier > similarity {
            (similarity, best_candidate) = (earlier, earlier_candidate);
        }
        alignment.old[i] = OldLine::Residue {
            similarity,
            best_candidate,
            rewrite: Some(kind),
        };
    }
    for j in new_range.clone() {
        if !kept_new[j - new_range.start] {
            alignment.new[j] = NewLine::Residue {
                rewrite: Some(kind),
            };
        }
    }
}

/// Marks runs of consecutive identical matches that are big enough to survive a rewrite.
fn surviving_runs(
    alignment: &FileAlignment,
    old: &[Line],
    new: &[Line],
    old_range: Range<usize>,
) -> Vec<bool> {
    let identical = |i: usize| match alignment.old[i] {
        OldLine::Matched {
            new: j,
            state: State::Verbatim,
            ..
        } if old[i].fingerprint == new[j].fingerprint => Some(j),
        _ => None,
    };
    let mut kept = vec![false; old_range.len()];
    let mut i = old_range.start;
    while i < old_range.end {
        let Some(mut j) = identical(i) else {
            i += 1;
            continue;
        };
        let start = i;
        let mut content_tokens = 0;
        loop {
            content_tokens += old[i].content_tokens;
            i += 1;
            match (i < old_range.end).then(|| identical(i)).flatten() {
                Some(next) if next == j + 1 => j = next,
                _ => break,
            }
        }
        if i - start >= MIN_MOVE_BLOCK_LINES && content_tokens >= MIN_MOVE_BLOCK_CONTENT_TOKENS {
            kept[start - old_range.start..i - old_range.start].fill(true);
        }
    }
    kept
}

struct Candidate {
    new: usize,
    shared: u32,
    total: u32,
    similarity: f64,
}

impl Candidate {
    fn units(&self, scale: u128) -> u128 {
        2 * u128::from(self.shared) * scale / u128::from(self.total)
    }
}

/// The least common multiple of a hunk's score denominators, so its totals add up exactly. Two
/// and every `total` divide it, so every score becomes a whole number.
fn score_scale(candidates: &[Vec<Candidate>]) -> u128 {
    let mut scale: u128 = 2;
    for candidate in candidates.iter().flatten() {
        let total = u128::from(candidate.total);
        let Some(next) = (scale / gcd(scale, total)).checked_mul(total) else {
            return MAX_EXACT_SCALE;
        };
        if next > MAX_EXACT_SCALE {
            return MAX_EXACT_SCALE;
        }
        scale = next;
    }
    scale
}

fn gcd(a: u128, b: u128) -> u128 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn align_hunk(
    alignment: &mut FileAlignment,
    old: &[Line],
    new: &[Line],
    old_range: Range<usize>,
    new_range: Range<usize>,
) {
    let (m, n) = (old_range.len(), new_range.len());
    let grid = Grid::new(m, n);
    let threshold = (EDIT_MIN_DICE * CANDIDACY_SCALE as f64).round() as u128;
    let mut candidates: Vec<Vec<Candidate>> = Vec::with_capacity(m);
    for i in 0..m {
        let old_line = &old[old_range.start + i];
        let mut best: Option<(f64, usize)> = None;
        let mut row = Vec::new();
        for j in grid.pair_columns(i) {
            let Some((shared, total)) = score_pair(old_line, &new[new_range.start + j]) else {
                continue;
            };
            let similarity = 2.0 * f64::from(shared) / f64::from(total);
            if best.is_none_or(|(score, _)| similarity > score) {
                best = Some((similarity, new_range.start + j));
            }
            if 2 * u128::from(shared) * CANDIDACY_SCALE >= threshold * u128::from(total) {
                row.push(Candidate {
                    new: j,
                    shared,
                    total,
                    similarity,
                });
            }
        }
        if let Some((similarity, best_candidate)) = best {
            alignment.old[old_range.start + i] = OldLine::Residue {
                similarity,
                best_candidate: Some(best_candidate),
                rewrite: None,
            };
        }
        alignment.best[old_range.start + i] = best;
        candidates.push(row);
    }
    let groups = reflow_groups(&grid, &old[old_range.clone()], &new[new_range.clone()]);
    let scale = score_scale(&candidates);
    for chosen in best_alignment(&grid, m, n, &candidates, &groups, scale) {
        match chosen {
            Chosen::Pair(i, candidate) => {
                let (old_index, new_index) = (old_range.start + i, new_range.start + candidate.new);
                let state = if old[old_index].fingerprint == new[new_index].fingerprint {
                    State::Verbatim
                } else {
                    State::Edited
                };
                alignment.old[old_index] = OldLine::Matched {
                    new: new_index,
                    state,
                    similarity: candidate.similarity,
                    layer: DecidingLayer::WithinHunkAlignment,
                };
                alignment.new[new_index] = NewLine::Matched;
            }
            Chosen::Group(group) => {
                let first_old = old_range.start + group.old;
                let first_new = new_range.start + group.new;
                alignment.old[first_old] = OldLine::Matched {
                    new: first_new,
                    state: State::Verbatim,
                    similarity: 1.0,
                    layer: DecidingLayer::Reflow,
                };
                alignment.new[first_new] = NewLine::Matched;
                let rest_old = first_old + 1..first_old + group.old_len;
                for (outcome, line) in alignment.old[rest_old.clone()]
                    .iter_mut()
                    .zip(&old[rest_old])
                {
                    *outcome = OldLine::ReflowDead {
                        similarity: dice(line, &new[first_new]),
                        new: first_new,
                    };
                }
                alignment.new[first_new + 1..first_new + group.new_len].fill(NewLine::ReflowBorn);
            }
        }
    }
}

/// The pair's score as shared token pairs out of the total on both lines (so the score is
/// 2 × shared / total), or None when the pair isn't compared at all.
fn score_pair(old: &Line, new: &Line) -> Option<(u32, u32)> {
    if old.kind != new.kind {
        return None;
    }
    if old.fingerprint == new.fingerprint {
        return Some((1, 2));
    }
    if unigram_jaccard(old, new) < PREFILTER_MIN_UNIGRAM_JACCARD {
        return None;
    }
    let shared = shared_bigrams(old, new) as u32;
    let total = (old.bigrams.len() + new.bigrams.len()) as u32;
    Some((shared, total))
}

#[derive(Clone, Copy)]
struct Value {
    score: u128,
    distance: u64,
}

impl Value {
    fn beats(self, other: Value) -> bool {
        self.score > other.score || (self.score == other.score && self.distance < other.distance)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Unreachable,
    Start,
    SkipOld,
    SkipNew,
    Match,
    Group,
}

/// A reflow group: old lines old..old+old_len hold the same tokens as new lines new..new+new_len.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Group {
    old: usize,
    new: usize,
    old_len: usize,
    new_len: usize,
}

enum Chosen<'c> {
    Pair(usize, &'c Candidate),
    Group(Group),
}

/// Every reflow group whose start and end cells are in the grid, keyed by the cell where it ends.
fn reflow_groups(grid: &Grid, old: &[Line], new: &[Line]) -> HashMap<(usize, usize), Vec<Group>> {
    let mut groups: HashMap<(usize, usize), Vec<Group>> = HashMap::new();
    for a in 0..old.len() {
        for b in grid.lo[a]..=grid.hi[a].min(new.len() - 1) {
            if old[a].tokens[0] != new[b].tokens[0] {
                continue;
            }
            let Some((old_len, new_len)) = reflow_group(old, new, a, b) else {
                continue;
            };
            let end = (a + old_len, b + new_len);
            if grid.contains(end.0, end.1) {
                groups.entry(end).or_default().push(Group {
                    old: a,
                    new: b,
                    old_len,
                    new_len,
                });
            }
        }
    }
    groups
}

/// Walks the token counts of old line a and new line b side by side, moving on whichever side has
/// fewer tokens so far. The group ends the first time both sides end a line together, which is the
/// only place the token streams can match, so tokens are compared once at the end.
fn reflow_group(old: &[Line], new: &[Line], a: usize, b: usize) -> Option<(usize, usize)> {
    let kind = old[a].kind;
    if new[b].kind != kind {
        return None;
    }
    let (mut i, mut j) = (a, b);
    let (mut old_tokens, mut new_tokens) = (old[a].tokens.len(), new[b].tokens.len());
    while old_tokens != new_tokens {
        if old_tokens < new_tokens {
            i += 1;
            if i == old.len() || i - a >= REFLOW_MAX_GROUP_LINES || old[i].kind != kind {
                return None;
            }
            old_tokens += old[i].tokens.len();
        } else {
            j += 1;
            if j == new.len() || j - b >= REFLOW_MAX_GROUP_LINES || new[j].kind != kind {
                return None;
            }
            new_tokens += new[j].tokens.len();
        }
    }
    let (old_len, new_len) = (i + 1 - a, j + 1 - b);
    if old_len == 1 && new_len == 1 {
        return None;
    }
    let old_stream = old[a..=i].iter().flat_map(|line| &line.tokens);
    let new_stream = new[b..=j].iter().flat_map(|line| &line.tokens);
    old_stream.eq(new_stream).then_some((old_len, new_len))
}

/// The cells (old prefix i, new prefix j) alignment may use: all of them, or for a hunk with more
/// than HUNK_FULL_ALIGNMENT_MAX_CELLS line pairs, a band around the diagonal.
struct Grid {
    lo: Vec<usize>,
    hi: Vec<usize>,
}

impl Grid {
    fn new(m: usize, n: usize) -> Grid {
        if m * n <= HUNK_FULL_ALIGNMENT_MAX_CELLS {
            return Grid {
                lo: vec![0; m + 1],
                hi: vec![n; m + 1],
            };
        }
        // Never narrower than the longer side, or neighboring rows stop overlapping and no path exists.
        let width = (HUNK_ALIGNMENT_BAND_LINES * m.min(n)).max(m.max(n)) as u64;
        let (m, n) = (m as u64, n as u64);
        let (lo, hi) = (0..=m)
            .map(|i| {
                let center = i * n;
                (
                    center.saturating_sub(width).div_ceil(m) as usize,
                    ((center + width) / m).min(n) as usize,
                )
            })
            .unzip();
        Grid { lo, hi }
    }

    fn contains(&self, i: usize, j: usize) -> bool {
        self.lo[i] <= j && j <= self.hi[i]
    }

    /// New lines that old line i can pair with: the cells before and after the pair are in the grid.
    fn pair_columns(&self, i: usize) -> Range<usize> {
        self.lo[i].max(self.lo[i + 1].saturating_sub(1))..(self.hi[i] + 1).min(self.hi[i + 1])
    }
}

/// Needleman-Wunsch over candidate pairs and reflow groups: highest total score, then closest to
/// the diagonal, then earlier lines (ties keep the first transition considered, so later lines
/// stay unmatched).
fn best_alignment<'c>(
    grid: &Grid,
    m: usize,
    n: usize,
    candidates: &'c [Vec<Candidate>],
    groups: &HashMap<(usize, usize), Vec<Group>>,
    scale: u128,
) -> Vec<Chosen<'c>> {
    let mut steps: Vec<Vec<Step>> = Vec::with_capacity(m + 1);
    let mut group_steps: HashMap<(usize, usize), Group> = HashMap::new();
    let mut recent: VecDeque<Vec<Option<Value>>> = VecDeque::new();
    let earlier = |recent: &VecDeque<Vec<Option<Value>>>, i: usize, row: usize, column: usize| {
        let back = i - row;
        if back > recent.len() || !grid.contains(row, column) {
            return None;
        }
        recent[recent.len() - back][column - grid.lo[row]]
    };
    for i in 0..=m {
        let (lo, hi) = (grid.lo[i], grid.hi[i]);
        let mut row: Vec<Option<Value>> = vec![None; hi + 1 - lo];
        let mut row_steps = vec![Step::Unreachable; hi + 1 - lo];
        let mut next = 0;
        for j in lo..=hi {
            let mut best: Option<Value> = None;
            let mut step = Step::Unreachable;
            let mut cell_group = None;
            let mut consider = |value: Value, via: Step| {
                let wins = best.is_none_or(|current| value.beats(current));
                if wins {
                    best = Some(value);
                    step = via;
                }
                wins
            };
            if i == 0 && j == 0 {
                consider(
                    Value {
                        score: 0,
                        distance: 0,
                    },
                    Step::Start,
                );
            }
            if i > 0 {
                if let Some(value) = earlier(&recent, i, i - 1, j) {
                    consider(value, Step::SkipOld);
                }
            }
            if j > lo {
                if let Some(value) = row[j - 1 - lo] {
                    consider(value, Step::SkipNew);
                }
            }
            if i > 0 && j > 0 {
                let list = &candidates[i - 1];
                while next < list.len() && list[next].new < j - 1 {
                    next += 1;
                }
                if let (Some(candidate), Some(value)) = (
                    list.get(next).filter(|c| c.new == j - 1),
                    earlier(&recent, i, i - 1, j - 1),
                ) {
                    consider(
                        Value {
                            score: value.score + candidate.units(scale),
                            distance: value.distance + diagonal_distance(i - 1, j - 1, m, n),
                        },
                        Step::Match,
                    );
                }
            }
            for group in groups.get(&(i, j)).into_iter().flatten() {
                if let Some(value) = earlier(&recent, i, group.old, group.new) {
                    let value = Value {
                        score: value.score + scale,
                        distance: value.distance + diagonal_distance(group.old, group.new, m, n),
                    };
                    if consider(value, Step::Group) {
                        cell_group = Some(*group);
                    }
                }
            }
            row[j - lo] = best;
            row_steps[j - lo] = step;
            if let (Step::Group, Some(group)) = (step, cell_group) {
                group_steps.insert((i, j), group);
            }
        }
        recent.push_back(row);
        if recent.len() > REFLOW_MAX_GROUP_LINES.max(1) {
            recent.pop_front();
        }
        steps.push(row_steps);
    }

    let mut chosen = Vec::new();
    let (mut i, mut j) = (m, n);
    loop {
        match steps[i][j - grid.lo[i]] {
            Step::Start | Step::Unreachable => break,
            Step::SkipOld => i -= 1,
            Step::SkipNew => j -= 1,
            Step::Match => {
                let list = &candidates[i - 1];
                let index = list
                    .binary_search_by_key(&(j - 1), |candidate| candidate.new)
                    .expect("a matched cell has a candidate");
                chosen.push(Chosen::Pair(i - 1, &list[index]));
                i -= 1;
                j -= 1;
            }
            Step::Group => {
                let group = group_steps[&(i, j)];
                chosen.push(Chosen::Group(group));
                (i, j) = (group.old, group.new);
            }
        }
    }
    chosen.reverse();
    chosen
}

/// How far a pair sits from the hunk's diagonal, comparing the midpoints of the 2 lines.
fn diagonal_distance(i: usize, j: usize, m: usize, n: usize) -> u64 {
    ((2 * i + 1) as u64 * n as u64).abs_diff((2 * j + 1) as u64 * m as u64)
}

/// Jaccard similarity of the two lines distinct tokens.
pub fn unigram_jaccard(old: &Line, new: &Line) -> f64 {
    jaccard(&old.unigrams, &new.unigrams)
}

/// Dice coefficient of the two lines token-pair multisets.
pub fn dice(old: &Line, new: &Line) -> f64 {
    2.0 * shared_bigrams(old, new) as f64 / (old.bigrams.len() + new.bigrams.len()) as f64
}

fn shared_bigrams(old: &Line, new: &Line) -> usize {
    let (a, b) = (&old.bigrams, &new.bigrams);
    let (mut i, mut j, mut shared) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
    }
    shared
}

#[cfg(test)]
mod tests {
    use super::{
        align_file, dice, unigram_jaccard, FileAlignment, Grid, NewLine, OldLine, Rewrite,
    };
    use crate::constants::{HUNK_ALIGNMENT_BAND_LINES, HUNK_FULL_ALIGNMENT_MAX_CELLS};
    use crate::histogram::Segment;
    use crate::language::Language;
    use crate::normalize::{normalize, Line};
    use crate::pipeline::{DecidingLayer, State};

    fn lines(source: &str, language: Language) -> Vec<Line> {
        normalize(source, language).expect("parses")
    }

    fn line(source: &str, language: Language) -> Line {
        lines(source, language)
            .into_iter()
            .next()
            .expect("one tracked line")
    }

    fn one_hunk(old: &[Line], new: &[Line]) -> FileAlignment {
        align_file(
            old,
            new,
            &[Segment::Hunk {
                old: 0..old.len(),
                new: 0..new.len(),
            }],
        )
    }

    /// (old line number, new line number or 0 when unmatched, state)
    fn outcomes(
        old: &[Line],
        new: &[Line],
        alignment: &FileAlignment,
    ) -> Vec<(u32, u32, Option<State>)> {
        alignment
            .old
            .iter()
            .zip(old)
            .map(|(outcome, line)| match outcome {
                OldLine::Matched { new: j, state, .. } => {
                    (line.number, new[*j].number, Some(*state))
                }
                _ => (line.number, 0, None),
            })
            .collect()
    }

    #[test]
    fn a_one_word_change_in_a_short_line_scores_exactly_one_half() {
        let old = line("mod inventory;\n", Language::Rust);
        let new = line("mod stock;\n", Language::Rust);
        assert_eq!(dice(&old, &new), 0.5);
        assert_eq!(unigram_jaccard(&old, &new), 0.5);
        let alignment = one_hunk(std::slice::from_ref(&old), std::slice::from_ref(&new));
        assert!(matches!(
            alignment.old[0],
            OldLine::Matched {
                state: State::Edited,
                layer: DecidingLayer::WithinHunkAlignment,
                ..
            }
        ));
    }

    #[test]
    fn identical_lines_score_one_and_unrelated_lines_score_low() {
        let old = line(
            "let quantity = parts.next()?.trim().parse().ok()?;\n",
            Language::Rust,
        );
        assert_eq!(dice(&old, &old), 1.0);
        let unrelated = line("println!(\"done\");\n", Language::Rust);
        assert!(dice(&old, &unrelated) < 0.2);
    }

    #[test]
    fn repeated_token_pairs_are_counted_as_a_multiset() {
        let old = line("f()()\n", Language::Python);
        let new = line("f()\n", Language::Python);
        assert_eq!(old.bigrams.len(), 6);
        assert_eq!(new.bigrams.len(), 4);
        assert_eq!(dice(&old, &new), 2.0 * 4.0 / 10.0);
    }

    const VALIDATE_OLD: &str = r#"use crate::inventory::Item;

fn log_rejection(item: &Item) {
    eprintln!("rejected {}", item.name);
}

pub fn validate(item: &Item) -> Result<(), String> {
    if item.name.is_empty() {
        return Err(String::from("invalid item"));
    }
    if item.name.len() > 64 {
        log_rejection(item);
        return Err(String::from("invalid item"));
    }
    if item.quantity == 0 {
        return Err(String::from("invalid item"));
    }
    if item.quantity > 10_000 {
        return Err(String::from("invalid item"));
    }
    if item.price_cents == 0 {
        log_rejection(item);
        return Err(String::from("invalid item"));
    }
    if item.price_cents > 1_000_000 {
        return Err(String::from("invalid item"));
    }
    Ok(())
}
"#;

    #[test]
    fn duplicate_heavy_as_one_hunk_is_aligned_by_position_not_by_text() {
        let new_source = VALIDATE_OLD.replace(
            "    if item.quantity == 0 {\n        return Err(String::from(\"invalid item\"));",
            "    if item.quantity == 0 {\n        log_rejection(item);\n        return Err(String::from(\"quantity must be positive\"));",
        );
        let old = lines(VALIDATE_OLD, Language::Rust);
        let new = lines(&new_source, Language::Rust);
        let alignment = one_hunk(&old, &new);
        for (old_number, new_number, state) in outcomes(&old, &new, &alignment) {
            match old_number {
                16 => assert_eq!((new_number, state), (17, Some(State::Edited))),
                n if n < 16 => {
                    assert_eq!((new_number, state), (n, Some(State::Verbatim)), "line {n}")
                }
                n => assert_eq!(
                    (new_number, state),
                    (n + 1, Some(State::Verbatim)),
                    "line {n}"
                ),
            }
        }
    }

    #[test]
    fn ties_keep_the_earlier_copy_and_stay_near_the_diagonal() {
        let old = lines("x = 1\nreturn None\nx = 1\n", Language::Python);
        let new = lines("x = 1\n", Language::Python);
        assert_eq!(
            outcomes(&old, &new, &one_hunk(&old, &new)),
            [(1, 1, Some(State::Verbatim)), (2, 0, None), (3, 0, None)]
        );
        let old = lines("y = 2\n", Language::Python);
        let new = lines("y = 2\nprint(y)\ny = 2\n", Language::Python);
        assert_eq!(
            outcomes(&old, &new, &one_hunk(&old, &new)),
            [(1, 1, Some(State::Verbatim))]
        );
    }

    #[test]
    fn dead_lines_keep_their_best_score_even_below_the_edit_threshold() {
        let old = lines("total = price * quantity\n", Language::Python);
        let new = lines("total = price + tax + shipping\n", Language::Python);
        let alignment = one_hunk(&old, &new);
        let OldLine::Residue {
            similarity,
            best_candidate,
            ..
        } = alignment.old[0]
        else {
            panic!("expected an unmatched line, got {:?}", alignment.old[0]);
        };
        assert!(similarity > 0.0 && similarity < 0.5, "{similarity}");
        assert_eq!(best_candidate, Some(0));
    }

    const KEPT: &str = "totals = compute_totals(orders, discounts)\nnames = normalize_names(customers)\nreport = merge_report(totals, names)\n";

    fn rewrite_of(old: &OldLine) -> Option<Rewrite> {
        match old {
            OldLine::Residue { rewrite, .. } => *rewrite,
            _ => None,
        }
    }

    #[test]
    fn a_rewritten_hunk_loses_its_chance_matches_but_the_rest_of_the_file_survives() {
        let old_source = format!(
            "{KEPT}{KEPT}def summarize(rows):\n    for row in rows:\n        emit(row.name, row.total)\n    return None\n"
        );
        let new_source = format!(
            "{KEPT}{KEPT}def summarize(rows):\n    grouped = bucket_by_region(rows)\n    publish_chart(grouped, style=\"bar\")\n    return None\n"
        );
        let old = lines(&old_source, Language::Python);
        let new = lines(&new_source, Language::Python);
        let alignment = align_file(
            &old,
            &new,
            &[
                Segment::Anchor {
                    old: 0..7,
                    new: 0..7,
                },
                Segment::Hunk {
                    old: 7..10,
                    new: 7..10,
                },
            ],
        );
        assert!((0..7).all(|i| matches!(alignment.old[i], OldLine::Matched { .. })));
        assert!(
            (7..10).all(|i| rewrite_of(&alignment.old[i]) == Some(Rewrite::Hunk)),
            "{:?}",
            alignment.old
        );
        assert!((7..10).all(|j| alignment.new[j]
            == NewLine::Residue {
                rewrite: Some(Rewrite::Hunk)
            }));
    }

    #[test]
    fn identical_blocks_survive_a_rewrite_but_single_matching_lines_do_not() {
        let old_source = format!(
            "import os\nlegacy_setup = configure_legacy(settings)\n{KEPT}legacy = parse_legacy_rows(raw_rows, delimiter)\nfiltered = drop_expired(legacy, cutoff_date)\nordered = sort_by_region(filtered, region_names)\nemit_legacy_report(ordered, printer_target)\narchive_legacy(ordered, archive_bucket)\ncleanup_legacy(archive_bucket)\n"
        );
        let new_source = format!(
            "import os\nfresh_setup = configure_stream(settings)\n{KEPT}stream = open_event_stream(source_url, batch_size)\nchecked = validate_schema(stream, schema_version)\nstored = persist_batch(checked, warehouse_table)\nnotify_listeners(stored, webhook_urls)\nrecord_metrics(stored, metrics_client)\nclose_stream(stream)\n"
        );
        let old = lines(&old_source, Language::Python);
        let new = lines(&new_source, Language::Python);
        let alignment = one_hunk(&old, &new);
        assert_eq!(rewrite_of(&alignment.old[0]), Some(Rewrite::File));
        assert_eq!(rewrite_of(&alignment.old[1]), Some(Rewrite::File));
        assert!((2..5).all(|i| matches!(
            alignment.old[i],
            OldLine::Matched {
                state: State::Verbatim,
                ..
            }
        )));
        assert!((5..11).all(|i| rewrite_of(&alignment.old[i]) == Some(Rewrite::File)));
        let OldLine::Residue {
            similarity,
            best_candidate,
            ..
        } = alignment.old[0]
        else {
            unreachable!();
        };
        assert_eq!((similarity, best_candidate), (1.0, Some(0)));
    }

    #[test]
    fn a_large_deletion_is_not_a_rewrite() {
        let old = lines(
            "first = load_rows(path)\nsecond = clean_rows(first)\nthird = rank_rows(second)\nfourth = write_rows(third)\n",
            Language::Python,
        );
        let new = lines("third = rank_rows(second)\n", Language::Python);
        let alignment = one_hunk(&old, &new);
        assert!(matches!(alignment.old[2], OldLine::Matched { new: 0, .. }));
        assert!([0, 1, 3]
            .iter()
            .all(|&i| rewrite_of(&alignment.old[i]).is_none()));
    }

    #[test]
    fn a_joined_call_keeps_its_first_line_and_does_not_swallow_the_next_line() {
        let old = lines("foo(a,\n    b)\nbar()\n", Language::Python);
        let new = lines("foo(a, b)\nbar()\n", Language::Python);
        let alignment = one_hunk(&old, &new);
        assert_eq!(
            alignment.old[0],
            OldLine::Matched {
                new: 0,
                state: State::Verbatim,
                similarity: 1.0,
                layer: DecidingLayer::Reflow
            }
        );
        assert!(matches!(alignment.old[1], OldLine::ReflowDead { .. }));
        assert!(matches!(
            alignment.old[2],
            OldLine::Matched {
                new: 1,
                state: State::Verbatim,
                layer: DecidingLayer::WithinHunkAlignment,
                ..
            }
        ));
    }

    #[test]
    fn a_split_call_keeps_its_line_and_the_extra_new_lines_are_born() {
        let old = lines("total = compute(first, second, third)\n", Language::Python);
        let new = lines(
            "total = compute(\n    first,\n    second,\n    third,\n)\n",
            Language::Python,
        );
        let alignment = one_hunk(&old, &new);
        assert!(matches!(
            alignment.old[0],
            OldLine::Matched {
                new: 0,
                layer: DecidingLayer::Reflow,
                ..
            }
        ));
        assert_eq!(alignment.new[0], NewLine::Matched);
        assert!((1..5).all(|j| alignment.new[j] == NewLine::ReflowBorn));
    }

    #[test]
    fn a_split_that_also_changes_a_token_is_not_a_reflow() {
        let old = lines("total = compute(first, second)\n", Language::Python);
        let new = lines(
            "total = compute(\n    first,\n    extra)\n",
            Language::Python,
        );
        let alignment = one_hunk(&old, &new);
        assert!(!alignment.old.iter().any(|outcome| matches!(
            outcome,
            OldLine::Matched {
                layer: DecidingLayer::Reflow,
                ..
            } | OldLine::ReflowDead { .. }
        )));
        assert!(!alignment.new.contains(&NewLine::ReflowBorn));
    }

    #[test]
    fn a_reflow_group_loses_to_better_line_matches_that_cross_it() {
        let old = lines(
            "alpha = load_alpha(source)\nbeta = load_beta(source)\ngamma = load_gamma(source)\nfoo(a,\n    b)\n",
            Language::Python,
        );
        let new = lines(
            "foo(a, b)\nalpha = load_alpha(target)\nbeta = load_beta(target)\ngamma = load_gamma(target)\n",
            Language::Python,
        );
        let alignment = one_hunk(&old, &new);
        assert!((0..3).all(|i| matches!(
            alignment.old[i],
            OldLine::Matched { new, state: State::Edited, .. } if new == i + 1
        )));
        assert!(!alignment.new.contains(&NewLine::ReflowBorn));
        assert!((3..5).all(|i| matches!(alignment.old[i], OldLine::Residue { .. })));
    }

    #[test]
    fn the_band_counts_lines_of_the_longer_side_and_its_rows_always_overlap() {
        for (m, n) in [
            (600, 600),
            (100, 3000),
            (3000, 100),
            (3, 90_000),
            (90_000, 3),
        ] {
            let grid = Grid::new(m, n);
            assert_eq!((grid.lo[0], grid.hi[m]), (0, n), "{m}x{n}");
            for i in 0..m {
                assert!(grid.lo[i] <= grid.hi[i], "{m}x{n} row {i} is empty");
                assert!(
                    grid.hi[i] >= grid.lo[i + 1],
                    "{m}x{n} rows {i} and {} do not overlap",
                    i + 1
                );
            }
            let widest = (0..=m).map(|i| grid.hi[i] - grid.lo[i]).max().unwrap();
            if HUNK_ALIGNMENT_BAND_LINES * m.min(n) >= m.max(n) {
                let expected = 2 * HUNK_ALIGNMENT_BAND_LINES * m.min(n) / m + 2;
                assert!(
                    widest <= expected,
                    "{m}x{n}: rows up to {widest} wide, expected {expected}"
                );
            }
        }
        let full = Grid::new(500, 500);
        assert_eq!((full.lo[250], full.hi[250]), (0, 500));
    }

    #[test]
    fn equal_totals_tie_so_the_diagonal_decides() {
        let old = lines(
            "import alpha_module\nimport beta_module\np1 = q1 + r1\np2 = q2 + r2\np3 = q3 + r3\nprint(first_marker)\nprint(second_marker)\n",
            Language::Python,
        );
        let new = lines(
            "import alpha_module\nimport beta_module\nprint(first_marker)\nprint(second_marker)\np1 = q1 + s1\np2 = q2 + s2\np3 = q3 + s3\n# done\n",
            Language::Python,
        );
        assert_eq!(dice(&old[2], &new[4]), 2.0 / 3.0);
        let alignment = align_file(
            &old,
            &new,
            &[
                Segment::Anchor {
                    old: 0..2,
                    new: 0..2,
                },
                Segment::Hunk {
                    old: 2..7,
                    new: 2..8,
                },
            ],
        );
        assert!(
            (2..5).all(|i| matches!(
                alignment.old[i],
                OldLine::Matched { new, state: State::Edited, .. } if new == i + 2
            )),
            "three edits worth 2/3 each tie with two identical lines, and the diagonal picks the edits: {:?}",
            alignment.old
        );
        assert!((5..7).all(|i| matches!(alignment.old[i], OldLine::Residue { .. })));
    }

    #[test]
    fn a_file_rewrite_keeps_the_score_a_hunk_rewrite_already_found() {
        let old = lines(
            "total = compute(alpha_one,\n    beta_two)\nlegacy_one = parse_legacy(raw_input_rows)\nlegacy_two = filter_legacy(legacy_one, cutoff_value)\nlegacy_three = sort_legacy(legacy_two, sort_key_name)\nlegacy_four = emit_legacy(legacy_three, printer_target)\n",
            Language::Python,
        );
        let new = lines(
            "total = compute(alpha_one, beta_two)\nfresh_one = open_stream(source_endpoint_url)\nfresh_two = check_schema(fresh_one, schema_version_id)\nfresh_three = store_batch(fresh_two, warehouse_table_name)\nfresh_four = notify_hooks(fresh_three, webhook_target_list)\n",
            Language::Python,
        );
        let alignment = one_hunk(&old, &new);
        assert_eq!(
            alignment.old[0],
            OldLine::Residue {
                similarity: 1.0,
                best_candidate: Some(0),
                rewrite: Some(Rewrite::File)
            },
            "the discarded reflow match scored 1.0, and the file check must not lower it"
        );
        let OldLine::Residue {
            similarity,
            best_candidate,
            ..
        } = alignment.old[1]
        else {
            unreachable!();
        };
        assert_eq!(
            (similarity, best_candidate),
            (dice(&old[1], &new[0]), Some(0))
        );
    }

    #[test]
    fn huge_hunks_only_compare_pairs_near_the_diagonal() {
        let mut old_source = String::from("# marker\n");
        let mut new_source = String::new();
        for i in 0..599 {
            old_source.push_str(&format!("value_{i} = compute_{i}(input_{i})\n"));
            new_source.push_str(&format!("result_{i} = compute_{i}(input_{i})\n"));
        }
        new_source.push_str("# marker\n");
        let old = lines(&old_source, Language::Python);
        let new = lines(&new_source, Language::Python);
        assert!(old.len() * new.len() > HUNK_FULL_ALIGNMENT_MAX_CELLS);
        let alignment = one_hunk(&old, &new);
        assert!(
            matches!(
                alignment.old[0],
                OldLine::Residue {
                    best_candidate: None,
                    ..
                }
            ),
            "the marker lines are far apart, so they are never compared: {:?}",
            alignment.old[0]
        );
        assert!((1..old.len()).all(|i| matches!(
            alignment.old[i],
            OldLine::Matched { new, state: State::Edited, .. } if new == i - 1
        )));
    }

    #[test]
    fn a_rewritten_file_overrules_a_matching_import() {
        let old_source = "use std::collections::HashMap;\npub fn total(values: &[u32]) -> u32 {\n    let sum = values.iter().sum();\n    sum\n}\n";
        let new_source = "use std::collections::HashMap;\npub struct Cache {\n    entries: HashMap<String, u64>,\n}\nimpl Cache {\n    pub fn get(&self, key: &str) -> Option<u64> {\n        self.entries.get(key).copied()\n    }\n}\n";
        let old = lines(old_source, Language::Rust);
        let new = lines(new_source, Language::Rust);
        let alignment = align_file(
            &old,
            &new,
            &[
                Segment::Anchor {
                    old: 0..1,
                    new: 0..1,
                },
                Segment::Hunk {
                    old: 1..old.len(),
                    new: 1..new.len(),
                },
            ],
        );
        assert!(
            alignment
                .old
                .iter()
                .all(|outcome| rewrite_of(outcome) == Some(Rewrite::File)),
            "{:?}",
            alignment.old
        );
        assert!(alignment.new.iter().all(|outcome| *outcome
            == NewLine::Residue {
                rewrite: Some(Rewrite::File)
            }));
    }
}
