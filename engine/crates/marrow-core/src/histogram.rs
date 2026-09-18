use std::collections::HashMap;
use std::ops::Range;

use crate::constants::{HISTOGRAM_MAX_OCCURRENCES, WEAK_ANCHOR_MAX_CONTENT_TOKENS};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Anchor {
        old: Range<usize>,
        new: Range<usize>,
    },
    Hunk {
        old: Range<usize>,
        new: Range<usize>,
    },
}

enum Task {
    Region(Range<usize>, Range<usize>),
    Emit(Segment),
}

/// Histogram diff over line fingerprints. Returns anchors and hunks in file order.
pub fn diff(old: &[u64], new: &[u64]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut tasks = vec![Task::Region(0..old.len(), 0..new.len())];
    while let Some(task) = tasks.pop() {
        match task {
            Task::Emit(segment) => push_segment(&mut segments, segment),
            Task::Region(old_range, new_range) => {
                split_region(old, new, old_range, new_range, &mut tasks)
            }
        }
    }
    segments
}

/// Common leading and trailing lines are deliberately not trimmed first, unlike git: a moved
/// block ending in `}` would otherwise have that brace matched to the file's final `}`.
fn split_region(old: &[u64], new: &[u64], o: Range<usize>, n: Range<usize>, tasks: &mut Vec<Task>) {
    if o.is_empty() && n.is_empty() {
        return;
    }
    match rarest_longest_run(old, new, &o, &n) {
        Some((a, b, length)) => {
            tasks.push(Task::Region(a + length..o.end, b + length..n.end));
            tasks.push(Task::Emit(Segment::Anchor {
                old: a..a + length,
                new: b..b + length,
            }));
            tasks.push(Task::Region(o.start..a, n.start..b));
        }
        None => tasks.push(Task::Emit(Segment::Hunk { old: o, new: n })),
    }
}

/// The matching run whose rarest line occurs the fewest times in the old region, longest first.
/// Lines occurring more than HISTOGRAM_MAX_OCCURRENCES times can't start a run, and there is
/// deliberately no Myers fallback when nothing qualifies: the region stays one hunk.
fn rarest_longest_run(
    old: &[u64],
    new: &[u64],
    o: &Range<usize>,
    n: &Range<usize>,
) -> Option<(usize, usize, usize)> {
    if o.is_empty() || n.is_empty() {
        return None;
    }
    let mut occurrences: HashMap<u64, Vec<usize>> = HashMap::new();
    for index in o.clone() {
        occurrences.entry(old[index]).or_default().push(index);
    }
    let mut best: Option<(usize, usize, usize, usize)> = None;
    let mut j = n.start;
    while j < n.end {
        let mut next = j + 1;
        if let Some(positions) = occurrences
            .get(&new[j])
            .filter(|positions| positions.len() <= HISTOGRAM_MAX_OCCURRENCES)
        {
            for &i in positions {
                let (mut a, mut b) = (i, j);
                while a > o.start && b > n.start && old[a - 1] == new[b - 1] {
                    a -= 1;
                    b -= 1;
                }
                let (mut a_end, mut b_end) = (i + 1, j + 1);
                while a_end < o.end && b_end < n.end && old[a_end] == new[b_end] {
                    a_end += 1;
                    b_end += 1;
                }
                let length = a_end - a;
                let rarity = (a..a_end)
                    .map(|k| occurrences[&old[k]].len())
                    .min()
                    .unwrap_or(usize::MAX);
                let better = best.is_none_or(|(best_rarity, best_length, _, _)| {
                    rarity < best_rarity || (rarity == best_rarity && length > best_length)
                });
                if better {
                    best = Some((rarity, length, a, b));
                }
                next = next.max(b_end);
            }
        }
        j = next;
    }
    best.map(|(_, length, a, b)| (a, b, length))
}

fn push_segment(segments: &mut Vec<Segment>, segment: Segment) {
    if let Some(last) = segments.last_mut() {
        match (last, &segment) {
            (
                Segment::Anchor { old, new },
                Segment::Anchor {
                    old: next_old,
                    new: next_new,
                },
            )
            | (
                Segment::Hunk { old, new },
                Segment::Hunk {
                    old: next_old,
                    new: next_new,
                },
            ) if old.end == next_old.start && new.end == next_new.start => {
                old.end = next_old.end;
                new.end = next_new.end;
                return;
            }
            _ => {}
        }
    }
    segments.push(segment);
}

/// Merges each weak anchor, and the hunks on both sides of it, into one hunk.
pub fn merge_weak_anchors(segments: Vec<Segment>, old_content_tokens: &[u32]) -> Vec<Segment> {
    let mut segments = segments;
    loop {
        let mut merged = Vec::with_capacity(segments.len());
        let mut changed = false;
        let mut index = 0;
        while index < segments.len() {
            if let (
                Some(Segment::Hunk {
                    old: first_old,
                    new: first_new,
                }),
                Some(Segment::Anchor {
                    old: anchor_old, ..
                }),
                Some(Segment::Hunk {
                    old: last_old,
                    new: last_new,
                }),
            ) = (
                segments.get(index),
                segments.get(index + 1),
                segments.get(index + 2),
            ) {
                let weight: u32 = old_content_tokens[anchor_old.clone()].iter().sum();
                if weight <= WEAK_ANCHOR_MAX_CONTENT_TOKENS {
                    merged.push(Segment::Hunk {
                        old: first_old.start..last_old.end,
                        new: first_new.start..last_new.end,
                    });
                    index += 3;
                    changed = true;
                    continue;
                }
            }
            merged.push(segments[index].clone());
            index += 1;
        }
        segments = merged;
        if !changed {
            return segments;
        }
    }
}

#[derive(Clone, Copy)]
struct Group {
    start: usize,
    end: usize,
}

impl Group {
    fn first(changed: &[bool]) -> Group {
        let mut group = Group { start: 0, end: 0 };
        while group.end < changed.len() && changed[group.end] {
            group.end += 1;
        }
        group
    }

    fn is_empty(self) -> bool {
        self.start == self.end
    }

    fn next(&mut self, changed: &[bool]) -> bool {
        if self.end == changed.len() {
            return false;
        }
        self.start = self.end + 1;
        self.end = self.start;
        while self.end < changed.len() && changed[self.end] {
            self.end += 1;
        }
        true
    }

    fn previous(&mut self, changed: &[bool]) -> bool {
        if self.start == 0 {
            return false;
        }
        self.end = self.start - 1;
        self.start = self.end;
        while self.start > 0 && changed[self.start - 1] {
            self.start -= 1;
        }
        true
    }

    fn slide_down(&mut self, lines: &[u64], changed: &mut [bool]) -> bool {
        if self.end == lines.len() || lines[self.start] != lines[self.end] {
            return false;
        }
        changed[self.start] = false;
        changed[self.end] = true;
        self.start += 1;
        self.end += 1;
        while self.end < changed.len() && changed[self.end] {
            self.end += 1;
        }
        true
    }

    fn slide_up(&mut self, lines: &[u64], changed: &mut [bool]) -> bool {
        if self.start == 0 || lines[self.start - 1] != lines[self.end - 1] {
            return false;
        }
        self.start -= 1;
        self.end -= 1;
        changed[self.start] = true;
        changed[self.end] = false;
        while self.start > 0 && changed[self.start - 1] {
            self.start -= 1;
        }
        true
    }
}

/// Slides each block of changed lines the way git does without its indent heuristic: as far
/// down as it can go, or back up to the lowest position where it lines up with a change in the
/// other file. Equal lines at a block's edge otherwise decide which copy counts as unchanged.
pub fn slide_changes(segments: Vec<Segment>, old: &[u64], new: &[u64]) -> Vec<Segment> {
    let mut old_changed = vec![false; old.len()];
    let mut new_changed = vec![false; new.len()];
    for segment in &segments {
        if let Segment::Hunk { old, new } = segment {
            old_changed[old.clone()].fill(true);
            new_changed[new.clone()].fill(true);
        }
    }
    slide_file(old, &mut old_changed, &new_changed);
    slide_file(new, &mut new_changed, &old_changed);
    segments_from_changes(&old_changed, &new_changed)
}

fn slide_file(lines: &[u64], changed: &mut [bool], other_changed: &[bool]) {
    let mut group = Group::first(changed);
    let mut other = Group::first(other_changed);
    loop {
        if !group.is_empty() {
            let mut lines_up_with_other;
            loop {
                let size = group.end - group.start;
                while group.slide_up(lines, changed) {
                    let synced = other.previous(other_changed);
                    debug_assert!(synced);
                }
                lines_up_with_other = !other.is_empty();
                while group.slide_down(lines, changed) {
                    let synced = other.next(other_changed);
                    debug_assert!(synced);
                    lines_up_with_other |= !other.is_empty();
                }
                if size == group.end - group.start {
                    break;
                }
            }
            if lines_up_with_other {
                while other.is_empty() {
                    let slid = group.slide_up(lines, changed);
                    let synced = other.previous(other_changed);
                    debug_assert!(slid && synced);
                }
            }
        }
        if !group.next(changed) {
            break;
        }
        let synced = other.next(other_changed);
        debug_assert!(synced);
    }
}

fn segments_from_changes(old_changed: &[bool], new_changed: &[bool]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < old_changed.len() || j < new_changed.len() {
        let (hunk_i, hunk_j) = (i, j);
        while i < old_changed.len() && old_changed[i] {
            i += 1;
        }
        while j < new_changed.len() && new_changed[j] {
            j += 1;
        }
        if i > hunk_i || j > hunk_j {
            push_segment(
                &mut segments,
                Segment::Hunk {
                    old: hunk_i..i,
                    new: hunk_j..j,
                },
            );
        }
        let (anchor_i, anchor_j) = (i, j);
        while i < old_changed.len() && j < new_changed.len() && !old_changed[i] && !new_changed[j] {
            i += 1;
            j += 1;
        }
        if i > anchor_i {
            push_segment(
                &mut segments,
                Segment::Anchor {
                    old: anchor_i..i,
                    new: anchor_j..j,
                },
            );
        } else if i == hunk_i && j == hunk_j {
            break;
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::{diff, merge_weak_anchors, slide_changes, Segment};

    fn anchor(old: std::ops::Range<usize>, new: std::ops::Range<usize>) -> Segment {
        Segment::Anchor { old, new }
    }

    fn hunk(old: std::ops::Range<usize>, new: std::ops::Range<usize>) -> Segment {
        Segment::Hunk { old, new }
    }

    #[test]
    fn identical_sequences_are_one_anchor() {
        assert_eq!(diff(&[1, 2, 3], &[1, 2, 3]), [anchor(0..3, 0..3)]);
        assert_eq!(diff(&[], &[]), []);
    }

    #[test]
    fn insertions_and_deletions_become_hunks() {
        assert_eq!(
            diff(&[1, 2, 3], &[1, 9, 2, 3]),
            [anchor(0..1, 0..1), hunk(1..1, 1..2), anchor(1..3, 2..4)]
        );
        assert_eq!(
            diff(&[1, 2, 3], &[1, 3]),
            [anchor(0..1, 0..1), hunk(1..2, 1..1), anchor(2..3, 1..2)]
        );
        assert_eq!(diff(&[1], &[2]), [hunk(0..1, 0..1)]);
    }

    #[test]
    fn a_small_block_moved_past_a_large_one_is_the_hunk() {
        let old = [10, 1, 2, 30, 31, 32, 33];
        let new = [10, 30, 31, 32, 33, 1, 2];
        assert_eq!(
            diff(&old, &new),
            [
                anchor(0..1, 0..1),
                hunk(1..3, 1..1),
                anchor(3..7, 1..5),
                hunk(7..7, 5..7)
            ]
        );
    }

    #[test]
    fn a_moved_block_keeps_its_own_closing_brace() {
        let old = [10, 1, 2, 99, 30, 31, 32, 99];
        let new = [10, 30, 31, 32, 99, 1, 2, 99];
        assert_eq!(
            diff(&old, &new),
            [
                anchor(0..1, 0..1),
                hunk(1..4, 1..1),
                anchor(4..8, 1..5),
                hunk(8..8, 5..8)
            ]
        );
    }

    #[test]
    fn a_deleted_block_slides_down_past_an_equal_edge_line() {
        let old = [1, 9, 3, 4, 9, 5, 6, 7];
        let new = [1, 9, 5, 6, 7];
        let raw = diff(&old, &new);
        assert_eq!(
            raw,
            [anchor(0..1, 0..1), hunk(1..4, 1..1), anchor(4..8, 1..5)]
        );
        assert_eq!(
            slide_changes(raw, &old, &new),
            [anchor(0..2, 0..2), hunk(2..5, 2..2), anchor(5..8, 2..5)]
        );
    }

    #[test]
    fn a_change_stays_lined_up_with_the_change_on_the_other_side() {
        let old = [1, 9, 2, 9, 3];
        let new = [1, 9, 4, 3];
        let raw = diff(&old, &new);
        assert_eq!(
            raw,
            [anchor(0..2, 0..2), hunk(2..4, 2..3), anchor(4..5, 3..4)]
        );
        assert_eq!(slide_changes(raw.clone(), &old, &new), raw);
    }

    #[test]
    fn sliding_keeps_every_anchor_on_equal_lines() {
        let old = [9, 1, 9, 1, 9, 2, 9, 1, 9];
        let new = [9, 1, 9, 2, 9, 1, 9, 1, 9, 3];
        for segment in slide_changes(diff(&old, &new), &old, &new) {
            if let Segment::Anchor { old: o, new: n } = segment {
                assert_eq!(old[o], new[n]);
            }
        }
    }

    #[test]
    fn rare_lines_win_over_common_ones() {
        assert_eq!(
            diff(&[7, 7, 1], &[1, 7, 7]),
            [hunk(0..2, 0..0), anchor(2..3, 0..1), hunk(3..3, 1..3)]
        );
    }

    #[test]
    fn repetitive_regions_stay_one_hunk_without_a_myers_fallback() {
        let mut old = vec![5];
        old.extend(std::iter::repeat_n(7, 70));
        let mut new: Vec<u64> = std::iter::repeat_n(7, 70).collect();
        new.push(6);
        assert_eq!(diff(&old, &new), [hunk(0..71, 0..71)]);
    }

    #[test]
    fn weak_anchors_between_hunks_are_merged() {
        let segments = vec![
            hunk(0..2, 0..2),
            anchor(2..3, 2..3),
            hunk(3..4, 3..5),
            anchor(4..5, 5..6),
            hunk(5..6, 6..6),
        ];
        assert_eq!(
            merge_weak_anchors(segments, &[4, 4, 0, 4, 1, 4]),
            [hunk(0..6, 0..6)]
        );
    }

    #[test]
    fn strong_anchors_and_edge_anchors_stay() {
        let segments = vec![
            anchor(0..1, 0..1),
            hunk(1..2, 1..2),
            anchor(2..3, 2..3),
            hunk(3..4, 3..4),
        ];
        assert_eq!(
            merge_weak_anchors(segments.clone(), &[0, 4, 3, 4]),
            segments
        );
    }
}
