use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use gix::ObjectId;
use rayon::prelude::*;

use crate::align::{self, FileAlignment, NewLine, OldLine, Rewrite};
use crate::content::ContentId;
use crate::correspondence;
use crate::error::Error;
use crate::histogram;
use crate::history::Repository;
use crate::language::Language;
use crate::moves::{self, FileResidue, Move, Residue};
use crate::normalize::{normalize, Line};
use crate::select::{tracked_language, tracked_source};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Verbatim,
    Edited,
    Moved,
    Dead,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Verbatim => "verbatim",
            State::Edited => "edited",
            State::Moved => "moved",
            State::Dead => "dead",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecidingLayer {
    UnchangedFile,
    HistogramDiff,
    Reflow,
    WithinHunkAlignment,
    HunkRewrite,
    FileRewrite,
    IntraFileMove,
    CrossFileMove,
    NoMatch,
}

impl DecidingLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            DecidingLayer::UnchangedFile => "unchanged_file",
            DecidingLayer::HistogramDiff => "histogram_diff",
            DecidingLayer::Reflow => "reflow",
            DecidingLayer::WithinHunkAlignment => "within_hunk_alignment",
            DecidingLayer::HunkRewrite => "hunk_rewrite",
            DecidingLayer::FileRewrite => "file_rewrite",
            DecidingLayer::IntraFileMove => "intra_file_move",
            DecidingLayer::CrossFileMove => "cross_file_move",
            DecidingLayer::NoMatch => "no_match",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    pub path: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct Fate {
    pub commit: usize,
    pub state: State,
    pub position: Option<Position>,
    pub similarity: f64,
    pub layer: DecidingLayer,
}

#[derive(Clone, Debug)]
pub struct TracedLine {
    pub birth_commit: usize,
    pub birth: Position,
    pub fates: Vec<Fate>,
}

#[derive(Clone, Debug)]
pub struct Trace {
    pub commits: Vec<String>,
    pub lines: Vec<TracedLine>,
}

pub struct TrackedFile {
    pub content: ContentId,
    pub lines: Arc<Vec<Line>>,
}

pub type Snapshot = BTreeMap<String, TrackedFile>;

/// Every tracked file of one commit, normalized, and the paths the parser gave up on.
pub struct CommitSnapshot {
    pub files: Snapshot,
    pub unparsed: Vec<String>,
}

pub struct Match {
    pub old: Position,
    pub new: Position,
    pub state: State,
    pub similarity: f64,
    pub layer: DecidingLayer,
}

pub struct Death {
    pub old: Position,
    pub similarity: f64,
    pub layer: DecidingLayer,
    /// Where the line it was most nearly like ended up, when there was one. The spec asks for
    /// this on every decision, not only the ones that kept a line.
    pub candidate: Option<Position>,
}

#[derive(Default)]
pub struct Decisions {
    pub matches: Vec<Match>,
    pub deaths: Vec<Death>,
}

enum Normalized {
    Untracked,
    Lines(Arc<Vec<Line>>),
    ParserGaveUp,
}

type NormalizedCache = HashMap<(ObjectId, Language), Normalized>;

pub fn trace_repository(path: &Path) -> Result<Trace, Error> {
    let repository = Repository::open(path)?;
    let commits = repository.first_parent_history()?;
    let mut cache = NormalizedCache::new();
    let mut lines: Vec<TracedLine> = Vec::new();
    let mut live: HashMap<Position, usize> = HashMap::new();
    let mut previous = Snapshot::new();
    for (index, commit) in commits.iter().enumerate() {
        let current = load_snapshot(&repository, *commit, &mut cache)?;
        let mut next_live: HashMap<Position, usize> = HashMap::new();
        if index > 0 {
            let decisions = compare_snapshots(&previous, &current);
            let mut decided = HashSet::new();
            for decision in decisions.matches {
                let line = decided_line(&live, &mut decided, &decision.old)?;
                if next_live.contains_key(&decision.new) {
                    return Err(Error::Internal(format!(
                        "two lines were placed at {}:{} in commit {commit}",
                        decision.new.path, decision.new.line
                    )));
                }
                lines[line].fates.push(Fate {
                    commit: index,
                    state: decision.state,
                    position: Some(decision.new.clone()),
                    similarity: decision.similarity,
                    layer: decision.layer,
                });
                next_live.insert(decision.new, line);
            }
            for death in decisions.deaths {
                let line = decided_line(&live, &mut decided, &death.old)?;
                lines[line].fates.push(Fate {
                    commit: index,
                    state: State::Dead,
                    position: None,
                    similarity: death.similarity,
                    layer: death.layer,
                });
            }
            if decided.len() != live.len() {
                return Err(Error::Internal(format!(
                    "{} lines got no fate in commit {commit}",
                    live.len() - decided.len()
                )));
            }
        }
        for (path, file) in &current {
            for line in file.lines.iter() {
                let position = Position {
                    path: path.clone(),
                    line: line.number,
                };
                if next_live.contains_key(&position) {
                    continue;
                }
                next_live.insert(position.clone(), lines.len());
                lines.push(TracedLine {
                    birth_commit: index,
                    birth: position,
                    fates: Vec::new(),
                });
            }
        }
        live = next_live;
        previous = current;
    }
    Ok(Trace {
        commits: commits.iter().map(|id| id.to_string()).collect(),
        lines,
    })
}

fn decided_line(
    live: &HashMap<Position, usize>,
    decided: &mut HashSet<usize>,
    old: &Position,
) -> Result<usize, Error> {
    let Some(&line) = live.get(old) else {
        return Err(Error::Internal(format!(
            "a fate was decided for {}:{}, which isn't a live line",
            old.path, old.line
        )));
    };
    if !decided.insert(line) {
        return Err(Error::Internal(format!(
            "{}:{} was given two fates",
            old.path, old.line
        )));
    }
    Ok(line)
}

/// Every tracked file of a commit, with the id of its contents. Enough to tell which files a
/// caller already knows about without normalizing any of them.
pub fn tracked_files(
    repository: &Repository,
    commit: ObjectId,
) -> Result<BTreeMap<String, ContentId>, Error> {
    let mut files = BTreeMap::new();
    for (path, id) in repository.files(commit)? {
        if tracked_language(&path).is_some() {
            files.insert(path, content_id(&id)?);
        }
    }
    Ok(files)
}

/// Reads one commit on its own, normalizing only the files `keep` asks for. A file the parser
/// gives up on is reported rather than fatal, so reconciliation can skip it and carry on.
pub fn snapshot_of_commit(
    repository: &Repository,
    commit: ObjectId,
    keep: impl Fn(&str) -> bool,
) -> Result<CommitSnapshot, Error> {
    load_commit(repository, commit, &mut NormalizedCache::new(), keep)
}

/// A trace can't skip a file: leaving one out mid-history would look like every line in it dying.
fn load_snapshot(
    repository: &Repository,
    commit: ObjectId,
    cache: &mut NormalizedCache,
) -> Result<Snapshot, Error> {
    let loaded = load_commit(repository, commit, cache, |_| true)?;
    match loaded.unparsed.into_iter().next() {
        None => Ok(loaded.files),
        Some(path) => Err(Error::ParserGaveUp {
            commit: commit.to_string(),
            path,
        }),
    }
}

fn load_commit(
    repository: &Repository,
    commit: ObjectId,
    cache: &mut NormalizedCache,
    keep: impl Fn(&str) -> bool,
) -> Result<CommitSnapshot, Error> {
    let mut wanted = Vec::new();
    for (path, id) in repository.files(commit)? {
        if let Some(language) = tracked_language(&path) {
            if keep(&path) {
                wanted.push((path, id, language));
            }
        }
    }
    let used: HashSet<(ObjectId, Language)> = wanted
        .iter()
        .map(|(_, id, language)| (*id, *language))
        .collect();
    cache.retain(|key, _| used.contains(key));
    let mut queued = HashSet::new();
    let mut blobs = Vec::new();
    for (_, id, language) in &wanted {
        let key = (*id, *language);
        if !cache.contains_key(&key) && queued.insert(key) {
            blobs.push((key, repository.blob(*id)?));
        }
    }
    let normalized: Vec<_> = blobs
        .into_par_iter()
        .map(|(key, bytes)| {
            let normalized = match tracked_source(&bytes) {
                None => Normalized::Untracked,
                Some(source) => match normalize(source, key.1) {
                    Some(lines) => Normalized::Lines(Arc::new(lines)),
                    None => Normalized::ParserGaveUp,
                },
            };
            (key, normalized)
        })
        .collect();
    cache.extend(normalized);
    let mut files = Snapshot::new();
    let mut unparsed = Vec::new();
    for (path, id, language) in wanted {
        match &cache[&(id, language)] {
            Normalized::Untracked => {}
            Normalized::Lines(lines) => {
                files.insert(
                    path,
                    TrackedFile {
                        content: content_id(&id)?,
                        lines: Arc::clone(lines),
                    },
                );
            }
            Normalized::ParserGaveUp => unparsed.push(path),
        }
    }
    Ok(CommitSnapshot { files, unparsed })
}

/// Compares two whole trees: file correspondence, then Layers 2 and 3 per pair, then moves
/// across the whole tree. This is what a trace step and the post-commit reconciliation both use.
pub fn compare_snapshots(previous: &Snapshot, current: &Snapshot) -> Decisions {
    let files = correspondence::correspond(previous, current);
    let old_paths: Vec<&str> = previous.keys().map(String::as_str).collect();
    let new_paths: Vec<&str> = current.keys().map(String::as_str).collect();
    let old_lines = |file: usize| &previous[old_paths[file]].lines[..];
    let new_lines = |file: usize| &current[new_paths[file]].lines[..];
    let file_of = |paths: &[&str], path: &str| {
        paths
            .binary_search(&path)
            .expect("a paired path is in the snapshot")
    };

    let mut old_residue: Vec<FileResidue> = (0..old_paths.len())
        .map(|file| residue_of(old_lines(file)))
        .collect();
    let mut new_residue: Vec<FileResidue> = (0..new_paths.len())
        .map(|file| residue_of(new_lines(file)))
        .collect();
    let mut alignments: Vec<(usize, FileAlignment)> = Vec::with_capacity(files.pairs.len());
    let mut alignment_of: Vec<Option<usize>> = vec![None; old_paths.len()];
    for (pair, file_pair) in files.pairs.iter().enumerate() {
        let old_file = file_of(&old_paths, &file_pair.old_path);
        let new_file = file_of(&new_paths, &file_pair.new_path);
        let (old, new) = (
            &previous[&file_pair.old_path],
            &current[&file_pair.new_path],
        );
        let alignment = if old.content.is_known() && old.content == new.content {
            align::unchanged_file(&old.lines, &new.lines)
        } else {
            align_changed_file(&old.lines, &new.lines)
        };
        mark_residue(
            &mut old_residue[old_file],
            &mut new_residue[new_file],
            &alignment,
        );
        old_residue[old_file].pair = Some(pair);
        new_residue[new_file].pair = Some(pair);
        alignment_of[old_file] = Some(alignments.len());
        alignments.push((new_file, alignment));
    }

    let moved: HashMap<(usize, usize), Move> =
        moves::detect_moves(&mut old_residue, &mut new_residue)
            .into_iter()
            .map(|found| ((found.old_file, found.old_line), found))
            .collect();

    let mut decisions = Decisions::default();
    for (old_file, path) in old_paths.iter().enumerate() {
        for (index, line) in old_lines(old_file).iter().enumerate() {
            let old = position(path, line);
            if let Some(found) = moved.get(&(old_file, index)) {
                decisions.matches.push(Match {
                    old,
                    new: position(
                        new_paths[found.new_file],
                        &new_lines(found.new_file)[found.new_line],
                    ),
                    state: State::Moved,
                    similarity: 1.0,
                    layer: found.layer,
                });
                continue;
            }
            let Some((new_file, alignment)) =
                alignment_of[old_file].map(|at| (alignments[at].0, &alignments[at].1))
            else {
                decisions.deaths.push(Death {
                    old,
                    similarity: 0.0,
                    layer: DecidingLayer::NoMatch,
                    candidate: None,
                });
                continue;
            };
            match alignment.old[index] {
                OldLine::Matched {
                    new: new_index,
                    state,
                    similarity,
                    layer,
                } => decisions.matches.push(Match {
                    old,
                    new: position(new_paths[new_file], &new_lines(new_file)[new_index]),
                    state,
                    similarity,
                    layer,
                }),
                OldLine::ReflowDead { similarity, new } => decisions.deaths.push(Death {
                    old,
                    similarity,
                    layer: DecidingLayer::Reflow,
                    candidate: Some(position(new_paths[new_file], &new_lines(new_file)[new])),
                }),
                OldLine::Residue {
                    similarity,
                    best_candidate,
                    rewrite,
                } => decisions.deaths.push(Death {
                    old,
                    similarity,
                    layer: match rewrite {
                        None => DecidingLayer::NoMatch,
                        Some(Rewrite::Hunk) => DecidingLayer::HunkRewrite,
                        Some(Rewrite::File) => DecidingLayer::FileRewrite,
                    },
                    candidate: best_candidate
                        .map(|at| position(new_paths[new_file], &new_lines(new_file)[at])),
                }),
            }
        }
    }
    decisions
}

/// Marrow reads git's own blob ids, so it has to be told when a repository uses ids it can't
/// hold. Quietly turning one into a placeholder would make every file look like every other.
fn content_id(id: &ObjectId) -> Result<ContentId, Error> {
    ContentId::from_bytes(id.as_bytes()).ok_or_else(|| {
        Error::Internal(format!(
            "this repository's object ids are {} bytes, and marrow only understands sha-1",
            id.as_bytes().len()
        ))
    })
}

fn residue_of(lines: &[Line]) -> FileResidue<'_> {
    FileResidue {
        lines,
        pair: None,
        residue: vec![Residue::Free { rewrite: false }; lines.len()],
    }
}

/// Only the lines Layers 2 and 3 left unmatched are residue Layer 4 may claim.
fn mark_residue(old: &mut FileResidue, new: &mut FileResidue, alignment: &FileAlignment) {
    for (slot, outcome) in old.residue.iter_mut().zip(&alignment.old) {
        *slot = match outcome {
            OldLine::Residue { rewrite, .. } => Residue::Free {
                rewrite: rewrite.is_some(),
            },
            _ => Residue::Taken,
        };
    }
    for (slot, outcome) in new.residue.iter_mut().zip(&alignment.new) {
        *slot = match outcome {
            NewLine::Residue { rewrite } => Residue::Free {
                rewrite: rewrite.is_some(),
            },
            _ => Residue::Taken,
        };
    }
}

/// What happened to one old line of a single file.
#[derive(Clone, Debug, PartialEq)]
pub enum LineFate {
    Kept {
        new: usize,
        state: State,
        similarity: f64,
        layer: DecidingLayer,
    },
    Dead {
        similarity: f64,
        candidate: Option<usize>,
        layer: DecidingLayer,
    },
}

/// Compares two versions of one file, which is all a capture hook sees: Layers 2 and 3, then
/// moves within the file. A move to another file needs the whole tree, so it waits for the
/// post-commit reconciliation.
pub fn compare_file(old: &[Line], new: &[Line]) -> Vec<LineFate> {
    let identical = old.len() == new.len()
        && old
            .iter()
            .zip(new)
            .all(|(a, b)| a.number == b.number && a.fingerprint == b.fingerprint);
    let alignment = if identical {
        align::unchanged_file(old, new)
    } else {
        align_changed_file(old, new)
    };
    let mut old_files = [residue_of(old)];
    let mut new_files = [residue_of(new)];
    mark_residue(&mut old_files[0], &mut new_files[0], &alignment);
    old_files[0].pair = Some(0);
    new_files[0].pair = Some(0);

    let mut fates: Vec<LineFate> = alignment
        .old
        .iter()
        .map(|outcome| match outcome {
            OldLine::Matched {
                new,
                state,
                similarity,
                layer,
            } => LineFate::Kept {
                new: *new,
                state: *state,
                similarity: *similarity,
                layer: *layer,
            },
            OldLine::ReflowDead { similarity, new } => LineFate::Dead {
                similarity: *similarity,
                candidate: Some(*new),
                layer: DecidingLayer::Reflow,
            },
            OldLine::Residue {
                similarity,
                best_candidate,
                rewrite,
            } => LineFate::Dead {
                similarity: *similarity,
                candidate: *best_candidate,
                layer: match rewrite {
                    None => DecidingLayer::NoMatch,
                    Some(Rewrite::Hunk) => DecidingLayer::HunkRewrite,
                    Some(Rewrite::File) => DecidingLayer::FileRewrite,
                },
            },
        })
        .collect();
    for found in moves::detect_moves(&mut old_files, &mut new_files) {
        fates[found.old_line] = LineFate::Kept {
            new: found.new_line,
            state: State::Moved,
            similarity: 1.0,
            layer: found.layer,
        };
    }
    fates
}

fn align_changed_file(old: &[Line], new: &[Line]) -> FileAlignment {
    let old_fingerprints: Vec<u64> = old.iter().map(|line| line.fingerprint).collect();
    let new_fingerprints: Vec<u64> = new.iter().map(|line| line.fingerprint).collect();
    let segments = histogram::diff(&old_fingerprints, &new_fingerprints);
    let segments = histogram::slide_changes(segments, &old_fingerprints, &new_fingerprints);
    let old_content_tokens: Vec<u32> = old.iter().map(|line| line.content_tokens).collect();
    let segments = histogram::merge_weak_anchors(segments, &old_content_tokens);
    align::align_file(old, new, &segments)
}

fn position(path: &str, line: &Line) -> Position {
    Position {
        path: path.to_owned(),
        line: line.number,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{compare_snapshots, DecidingLayer, Snapshot, State, TrackedFile};
    use crate::content::ContentId;
    use crate::language::Language;
    use crate::normalize::normalize;

    fn file(id: u8, source: &str, language: Language) -> TrackedFile {
        TrackedFile {
            content: ContentId::from_bytes(&[id; 20]).expect("twenty bytes"),
            lines: Arc::new(normalize(source, language).expect("parses")),
        }
    }

    #[test]
    fn identical_content_stays_verbatim_across_a_grammar_change() {
        let source = "export const n = <number>value;\nexport const m = 1;\n";
        let previous: Snapshot =
            [("a.ts".to_owned(), file(1, source, Language::TypeScript))].into();
        let current: Snapshot = [("a.tsx".to_owned(), file(1, source, Language::Tsx))].into();
        assert_ne!(
            previous["a.ts"].lines[0].fingerprint,
            current["a.tsx"].lines[0].fingerprint
        );
        let decisions = compare_snapshots(&previous, &current);
        assert!(decisions.deaths.is_empty());
        assert_eq!(decisions.matches.len(), 2);
        assert!(decisions.matches.iter().all(|decision| {
            decision.state == State::Verbatim && decision.layer == DecidingLayer::UnchangedFile
        }));
    }
}
