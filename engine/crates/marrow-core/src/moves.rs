//! Layer 4: move detection (docs/specs/genealogy.md).

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::constants::{
    MIN_MOVE_BLOCK_CONTENT_TOKENS, MIN_MOVE_BLOCK_LINES, MOVE_SEED_MAX_CANDIDATES,
    SINGLE_LINE_MOVE_MIN_CONTENT_TOKENS,
};
use crate::normalize::Line;
use crate::pipeline::DecidingLayer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Residue {
    Taken,
    Free { rewrite: bool },
}

/// One file on one side of a commit pair: its tracked lines, which are still residue, and the
/// file pair it belongs to (None for a deleted or added file).
pub struct FileResidue<'a> {
    pub lines: &'a [Line],
    pub pair: Option<usize>,
    pub residue: Vec<Residue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub old_file: usize,
    pub old_line: usize,
    pub new_file: usize,
    pub new_line: usize,
    pub layer: DecidingLayer,
}

/// Files on each side must be sorted by path, because their order breaks ties.
/// Blocks come before single lines, because a block is the stronger evidence: claiming a line on
/// its own first could split a block leaving the file into pieces too small to count.
pub fn detect_moves(old: &mut [FileResidue], new: &mut [FileResidue]) -> Vec<Move> {
    let mut moves = Vec::new();
    accept_blocks(old, new, Scope::SameFile, &mut moves);
    accept_blocks(old, new, Scope::OtherFiles, &mut moves);
    single_line_moves(old, new, &mut moves);
    moves
}

#[derive(Clone, Copy)]
enum Scope {
    SameFile,
    OtherFiles,
}

impl Scope {
    fn allows(self, old: &FileResidue, new: &FileResidue) -> bool {
        let same = old.pair.is_some() && old.pair == new.pair;
        match self {
            Scope::SameFile => same,
            Scope::OtherFiles => !same,
        }
    }

    fn layer(self) -> DecidingLayer {
        match self {
            Scope::SameFile => DecidingLayer::IntraFileMove,
            Scope::OtherFiles => DecidingLayer::CrossFileMove,
        }
    }
}

fn is_free(file: &FileResidue, line: usize) -> bool {
    matches!(file.residue[line], Residue::Free { .. })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Block {
    old_file: usize,
    old_start: usize,
    new_file: usize,
    new_start: usize,
    len: usize,
    content_tokens: u32,
}

impl Block {
    fn qualifies(&self) -> bool {
        self.len >= MIN_MOVE_BLOCK_LINES && self.content_tokens >= MIN_MOVE_BLOCK_CONTENT_TOKENS
    }

    /// Larger means better: more lines, more content, a smaller gap, then earlier files and starts.
    #[allow(clippy::type_complexity)]
    fn priority(
        &self,
    ) -> (
        usize,
        u32,
        Reverse<usize>,
        Reverse<usize>,
        Reverse<usize>,
        Reverse<usize>,
        Reverse<usize>,
    ) {
        (
            self.len,
            self.content_tokens,
            Reverse(self.old_start.abs_diff(self.new_start)),
            Reverse(self.old_file),
            Reverse(self.old_start),
            Reverse(self.new_file),
            Reverse(self.new_start),
        )
    }
}

impl Ord for Block {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority().cmp(&other.priority())
    }
}

impl PartialOrd for Block {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn accept_blocks(
    old: &mut [FileResidue],
    new: &mut [FileResidue],
    scope: Scope,
    moves: &mut Vec<Move>,
) {
    let mut heap: BinaryHeap<Block> = find_blocks(old, new, scope).into_iter().collect();
    while let Some(block) = heap.pop() {
        let pieces = free_pieces(old, new, block);
        if pieces != [block] {
            heap.extend(pieces.into_iter().filter(Block::qualifies));
            continue;
        }
        for offset in 0..block.len {
            old[block.old_file].residue[block.old_start + offset] = Residue::Taken;
            new[block.new_file].residue[block.new_start + offset] = Residue::Taken;
            moves.push(Move {
                old_file: block.old_file,
                old_line: block.old_start + offset,
                new_file: block.new_file,
                new_line: block.new_start + offset,
                layer: scope.layer(),
            });
        }
    }
}

/// Every maximal block the scope allows that meets the size limits.
fn find_blocks(old: &[FileResidue], new: &[FileResidue], scope: Scope) -> Vec<Block> {
    let mut index: HashMap<u64, Vec<(usize, usize)>> = HashMap::new();
    for (new_file, file) in new.iter().enumerate() {
        for (line, content) in file.lines.iter().enumerate() {
            if is_free(file, line) {
                index
                    .entry(content.fingerprint)
                    .or_default()
                    .push((new_file, line));
            }
        }
    }
    // Pairs already inside a block found from an earlier seed, so each block is walked once.
    let mut covered: HashSet<(usize, usize, usize, usize)> = HashSet::new();
    let mut blocks = Vec::new();
    for (old_file, file) in old.iter().enumerate() {
        for (line, content) in file.lines.iter().enumerate() {
            if !is_free(file, line) {
                continue;
            }
            let Some(targets) = index.get(&content.fingerprint) else {
                continue;
            };
            let allowed: Vec<(usize, usize)> = targets
                .iter()
                .copied()
                .filter(|&(new_file, _)| scope.allows(file, &new[new_file]))
                .take(MOVE_SEED_MAX_CANDIDATES + 1)
                .collect();
            if allowed.len() > MOVE_SEED_MAX_CANDIDATES {
                continue;
            }
            for (new_file, new_line) in allowed {
                if !covered.insert((old_file, new_file, line, new_line)) {
                    continue;
                }
                let block = extend(old, new, old_file, line, new_file, new_line);
                covered.extend((0..block.len).map(|offset| {
                    (
                        block.old_file,
                        block.new_file,
                        block.old_start + offset,
                        block.new_start + offset,
                    )
                }));
                if block.qualifies() {
                    blocks.push(block);
                }
            }
        }
    }
    blocks
}

/// Grows a seed backwards and forwards while both sides stay consecutive, free, and identical.
fn extend(
    old: &[FileResidue],
    new: &[FileResidue],
    old_file: usize,
    old_line: usize,
    new_file: usize,
    new_line: usize,
) -> Block {
    let (o, n) = (&old[old_file], &new[new_file]);
    let same = |a: usize, b: usize| {
        is_free(o, a) && is_free(n, b) && o.lines[a].fingerprint == n.lines[b].fingerprint
    };
    let mut back = 0;
    while old_line > back && new_line > back && same(old_line - back - 1, new_line - back - 1) {
        back += 1;
    }
    let (old_start, new_start) = (old_line - back, new_line - back);
    let mut len = 1;
    while old_start + len < o.lines.len()
        && new_start + len < n.lines.len()
        && same(old_start + len, new_start + len)
    {
        len += 1;
    }
    Block {
        old_file,
        old_start,
        new_file,
        new_start,
        len,
        content_tokens: o.lines[old_start..old_start + len]
            .iter()
            .map(|line| line.content_tokens)
            .sum(),
    }
}

/// The maximal runs of a block whose lines are still free on both sides.
fn free_pieces(old: &[FileResidue], new: &[FileResidue], block: Block) -> Vec<Block> {
    let (o, n) = (&old[block.old_file], &new[block.new_file]);
    let mut pieces = Vec::new();
    let mut start = None;
    for offset in 0..=block.len {
        let free = offset < block.len
            && is_free(o, block.old_start + offset)
            && is_free(n, block.new_start + offset);
        match (free, start) {
            (true, None) => start = Some(offset),
            (false, Some(first)) => {
                let old_start = block.old_start + first;
                pieces.push(Block {
                    old_start,
                    new_start: block.new_start + first,
                    len: offset - first,
                    content_tokens: o.lines[old_start..block.old_start + offset]
                        .iter()
                        .map(|line| line.content_tokens)
                        .sum(),
                    ..block
                });
                start = None;
            }
            _ => {}
        }
    }
    pieces
}

/// A line that appears exactly once in both versions of a file, and is residue on both sides,
/// changed position on its own.
fn single_line_moves(old: &mut [FileResidue], new: &mut [FileResidue], moves: &mut Vec<Move>) {
    let new_by_pair: HashMap<usize, usize> = new
        .iter()
        .enumerate()
        .filter_map(|(index, file)| file.pair.map(|pair| (pair, index)))
        .collect();
    let plain = Residue::Free { rewrite: false };
    for (old_file, file) in old.iter_mut().enumerate() {
        let Some(&new_file) = file.pair.and_then(|pair| new_by_pair.get(&pair)) else {
            continue;
        };
        let target = &mut new[new_file];
        let mut old_counts: HashMap<u64, usize> = HashMap::new();
        for line in file.lines {
            *old_counts.entry(line.fingerprint).or_default() += 1;
        }
        let mut new_positions: HashMap<u64, (usize, usize)> = HashMap::new();
        for (index, line) in target.lines.iter().enumerate() {
            let entry = new_positions.entry(line.fingerprint).or_insert((0, index));
            entry.0 += 1;
        }
        for (old_line, line) in file.lines.iter().enumerate() {
            if file.residue[old_line] != plain
                || line.content_tokens < SINGLE_LINE_MOVE_MIN_CONTENT_TOKENS
                || old_counts[&line.fingerprint] != 1
            {
                continue;
            }
            let Some(&(1, new_line)) = new_positions.get(&line.fingerprint) else {
                continue;
            };
            if target.residue[new_line] != plain {
                continue;
            }
            file.residue[old_line] = Residue::Taken;
            target.residue[new_line] = Residue::Taken;
            moves.push(Move {
                old_file,
                old_line,
                new_file,
                new_line,
                layer: DecidingLayer::IntraFileMove,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{detect_moves, FileResidue, Move, Residue};
    use crate::language::Language;
    use crate::normalize::{normalize, Line};
    use crate::pipeline::DecidingLayer;

    const HELPER: &str = "def normalize_name(raw):\n    trimmed = raw.strip()\n    lowered = trimmed.lower()\n    return lowered.replace(\" \", \"_\")\n";

    fn lines(source: &str) -> Vec<Line> {
        normalize(source, Language::Python).expect("parses")
    }

    fn file<'a>(lines: &'a [Line], pair: Option<usize>, free: &[usize]) -> FileResidue<'a> {
        let mut residue = vec![Residue::Taken; lines.len()];
        for &index in free {
            residue[index] = Residue::Free { rewrite: false };
        }
        FileResidue {
            lines,
            pair,
            residue,
        }
    }

    fn pairs(moves: &[Move]) -> Vec<(usize, usize, usize, usize, DecidingLayer)> {
        moves
            .iter()
            .map(|m| (m.old_file, m.old_line, m.new_file, m.new_line, m.layer))
            .collect()
    }

    #[test]
    fn a_block_moved_within_a_file_is_found() {
        let old_lines = lines(&format!(
            "{HELPER}config = load_config(path)\nrun(config)\n"
        ));
        let new_lines = lines(&format!(
            "config = load_config(path)\nrun(config)\n{HELPER}"
        ));
        let mut old = [file(&old_lines, Some(0), &[0, 1, 2, 3])];
        let mut new = [file(&new_lines, Some(0), &[2, 3, 4, 5])];
        let moves = detect_moves(&mut old, &mut new);
        assert_eq!(
            pairs(&moves),
            (0..4)
                .map(|k| (0, k, 0, k + 2, DecidingLayer::IntraFileMove))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn short_or_contentless_runs_do_not_move() {
        let old_lines = lines("x = 1\ny = 2\n");
        let new_lines = lines("y = 2\nx = 1\nx = 1\ny = 2\n");
        let mut old = [file(&old_lines, Some(0), &[0, 1])];
        let mut new = [file(&new_lines, Some(0), &[2, 3])];
        assert!(detect_moves(&mut old, &mut new).is_empty());
        let old_lines = normalize(
            "fn a() {\n    {\n        {\n        }\n    }\n}\n",
            Language::Rust,
        )
        .unwrap();
        let braces: Vec<usize> = (0..old_lines.len()).collect();
        let mut old = [file(&old_lines, None, &braces)];
        let mut new = [file(&old_lines, None, &braces)];
        assert!(detect_moves(&mut old, &mut new).is_empty());
    }

    #[test]
    fn a_unique_line_that_changed_position_moves_on_its_own() {
        let old_lines = lines("import sys\nimport os\nrun()\n");
        let new_lines = lines("import os\nrun()\nimport sys\n");
        let mut old = [file(&old_lines, Some(0), &[0])];
        let mut new = [file(&new_lines, Some(0), &[2])];
        assert_eq!(
            pairs(&detect_moves(&mut old, &mut new)),
            [(0, 0, 0, 2, DecidingLayer::IntraFileMove)]
        );
    }

    #[test]
    fn single_line_moves_need_unique_lines_outside_rewrites() {
        let old_lines = lines("import sys\nimport os\nimport sys\n");
        let new_lines = lines("import os\nimport sys\n");
        let mut old = [file(&old_lines, Some(0), &[0])];
        let mut new = [file(&new_lines, Some(0), &[1])];
        assert!(
            detect_moves(&mut old, &mut new).is_empty(),
            "duplicated in the old file"
        );
        let old_lines = lines("import sys\nimport os\n");
        let new_lines = lines("import os\nimport sys\n");
        let mut old = [file(&old_lines, Some(0), &[0])];
        old[0].residue[0] = Residue::Free { rewrite: true };
        let mut new = [file(&new_lines, Some(0), &[1])];
        assert!(detect_moves(&mut old, &mut new).is_empty(), "rewrite line");
    }

    #[test]
    fn a_block_extracted_into_another_file_moves_across_files() {
        let old_lines = lines(&format!("import os\n{HELPER}"));
        let new_lines = lines(&format!("from os import path\n{HELPER}"));
        let mut old = [file(&old_lines, Some(0), &[1, 2, 3, 4])];
        let mut new = [
            file(&old_lines[..1], Some(0), &[]),
            file(&new_lines, None, &[0, 1, 2, 3, 4]),
        ];
        assert_eq!(
            pairs(&detect_moves(&mut old, &mut new)),
            (0..4)
                .map(|k| (0, k + 1, 1, k + 1, DecidingLayer::CrossFileMove))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_bigger_block_wins_and_ties_go_to_the_earlier_file() {
        let helper = lines(HELPER);
        let partial = lines(
            "def normalize_name(raw):\n    trimmed = raw.strip()\n    lowered = trimmed.lower()\n",
        );
        let all = [0, 1, 2, 3];
        let mut old = [file(&helper, None, &all)];
        let mut new = [file(&partial, None, &[0, 1, 2]), file(&helper, None, &all)];
        let moves = detect_moves(&mut old, &mut new);
        assert!(
            moves.len() == 4 && moves.iter().all(|m| m.new_file == 1),
            "{moves:?}"
        );
        let mut old = [file(&helper, None, &all)];
        let mut new = [file(&helper, None, &all), file(&helper, None, &all)];
        let moves = detect_moves(&mut old, &mut new);
        assert!(
            moves.len() == 4 && moves.iter().all(|m| m.new_file == 0),
            "{moves:?}"
        );
    }

    #[test]
    fn what_is_left_of_an_overlapping_block_can_still_move() {
        let long = lines(&format!(
            "{HELPER}def slugify(title):\n    words = title.lower().split()\n    joined = \"-\".join(words)\n    return joined.strip(\"-\")\n"
        ));
        let slug_only = lines("def slugify(title):\n    words = title.lower().split()\n    joined = \"-\".join(words)\n    return joined.strip(\"-\")\n");
        let all: Vec<usize> = (0..long.len()).collect();
        let mut old = [
            file(&long, None, &all),
            file(&slug_only, None, &[0, 1, 2, 3]),
        ];
        let mut new = [file(&long, None, &all)];
        let moves = detect_moves(&mut old, &mut new);
        assert_eq!(
            moves.len(),
            8,
            "the full 8-line block moves from the first file: {moves:?}"
        );
        assert!(moves.iter().all(|m| m.old_file == 0));
    }
}
