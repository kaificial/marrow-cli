//! `marrow backfill`: the history from before marrow was watching (PRD §7.1).
//!
//! Walks the first-parent history of the repository in the current directory, oldest commit
//! first, and runs the same genealogy the post-commit hook runs on each consecutive pair. Every
//! line it finds is labelled human, because nothing was watching the agent back then. A commit
//! whose message carries Claude Code's co-author trailer is flagged, and the flag is stored next
//! to the lines' origin rather than used to change it.
//!
//! Each commit is recorded in a transaction of its own, together with a note of how far the walk
//! has got, so a backfill that is killed resumes from the last commit it finished. See
//! docs/decisions/0009-backfill.md.

use std::collections::{BTreeMap, HashMap};
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use marrow_core::align::bigram_dice;
use marrow_core::history::{CommitId, CommitInfo, Repository};
use marrow_core::normalize::{Line, NORMALIZER_VERSION};
use marrow_core::pipeline::{self, Loader, Snapshot, TrackedFile};
use marrow_core::select::tracked_language;
use marrow_core::{path_hash, ContentId, Position, State};
use marrow_store::{BornLine, CommitRow, FateRow, ObservedBy, Origin, Recording, Store};

use crate::capture::{
    engine_line, language_name, now, open_store, repository_root, stored_line, BACKFILL_RUNNING,
    BACKFILL_STATE,
};

const LAST_COMMIT: &str = "backfill.last_commit";
const SINCE: &str = "backfill.since";
/// The walk's clock at the last commit recorded, which can be later than that commit's own date.
const CLOCK: &str = "backfill.clock";
const COMPLETE: &str = "complete";

/// Where the walk starts. Either way, the lines already present at the first commit of the
/// window were born earlier, at a time the walk never saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Since {
    /// The last N commits.
    Commits(usize),
    /// Commits made on or after this day, UTC.
    Date { day: String, at: i64 },
}

impl Since {
    pub fn parse(text: &str) -> Result<Since, String> {
        let text = text.trim();
        if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
            return match text.parse::<usize>() {
                Ok(count) if count > 0 => Ok(Since::Commits(count)),
                _ => Err(format!("--since {text}: give at least one commit")),
            };
        }
        let at = parse_day(text).ok_or_else(|| {
            format!("--since {text}: expected a number of commits, or a date like 2026-01-31")
        })?;
        Ok(Since::Date {
            day: text.to_owned(),
            at,
        })
    }

    fn describe(&self) -> String {
        match self {
            Since::Commits(count) => count.to_string(),
            Since::Date { day, .. } => day.clone(),
        }
    }
}

pub enum Outcome {
    Done(Summary),
    Nothing(String),
}

pub struct Summary {
    pub commits: usize,
    pub through: String,
    pub counts: marrow_store::Counts,
    pub resumed: bool,
    /// Commits in this run whose message carries Claude Code's trailer.
    pub trailers: usize,
    /// File versions tree-sitter gave up on, kept as the last version that parsed.
    pub carried: Vec<(String, String)>,
    /// Files tree-sitter gave up on before any version of them parsed.
    pub skipped: Vec<(String, String)>,
}

/// Everything the walk knows about the commit it last recorded.
struct Walk {
    previous: Snapshot,
    /// Line ids by path, then by line number.
    live: HashMap<String, HashMap<u32, i64>>,
    previous_commit: Option<(String, i64)>,
}

pub fn backfill(since: Option<Since>) -> Result<Outcome, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("can't find the cwd: {error}"))?;
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let Some(root) = repository_root(&cwd) else {
        return Err("not inside a git repository".to_owned());
    };
    let repository = Repository::open(&root).map_err(|error| format!("{error}"))?;
    if !repository
        .has_commits()
        .map_err(|error| format!("{error}"))?
    {
        return Ok(Outcome::Nothing(
            "this repository has no commits yet".to_owned(),
        ));
    }
    let (history, truncated) = repository
        .first_parent_walk()
        .map_err(|error| format!("can't read this repository's history: {error}"))?;
    // Every commit's date, and the walk's clock: the same dates made to run forwards, over the
    // whole history, so the clock at a commit doesn't depend on which window is being walked.
    let dates: Vec<i64> = history
        .iter()
        .map(|id| repository.commit(*id).map(|info| info.committed_at))
        .collect::<Result<_, _>>()
        .map_err(|error| format!("{error}"))?;
    let present = now();
    let clock = monotone_clock(
        &dates
            .iter()
            .map(|&date| date.min(present))
            .collect::<Vec<_>>(),
    );
    let mut store = open_store(&root)?;

    let (plan, mut walk, resumed) = {
        let recording = store.begin().map_err(|error| format!("{error}"))?;
        let captured = recording.captured().map_err(|error| format!("{error}"))?;
        if captured.started() {
            return Err(format!(
                "this store already holds live capture ({} agent sessions, {} changes recorded \
                 from writes, {} reconciled commits). A backfill has to come before capture \
                 starts, because the two histories can't be joined reliably afterwards; see \
                 docs/decisions/0009-backfill.md",
                captured.sessions, captured.write_fates, captured.reconciled_commits
            ));
        }
        let last = recording
            .meta(LAST_COMMIT)
            .map_err(|error| format!("{error}"))?;
        let (plan, walk, resumed) = match last {
            Some(last) => {
                let stored_since = recording.meta(SINCE).map_err(|error| format!("{error}"))?;
                if let Some(since) = &since {
                    if stored_since.as_deref() != Some(since.describe().as_str()) {
                        return Err(format!(
                            "a backfill {} is already recorded here; run marrow backfill \
                             without --since to carry it on",
                            match &stored_since {
                                Some(stored) => format!("with --since {stored}"),
                                None => "of the whole history".to_owned(),
                            }
                        ));
                    }
                }
                let Some(at) = history.iter().position(|id| id.to_string() == last) else {
                    return Err(format!(
                        "the backfill here stopped at commit {}, which is no longer in this \
                         branch's first-parent history (was it rebased or reset?). Delete \
                         .marrow/ and run marrow backfill again",
                        &last[..last.len().min(8)]
                    ));
                };
                let walk = restore(&recording, &repository, history[at])?;
                (Plan::after(&history, &clock, at), walk, true)
            }
            None => {
                let plan = Plan::fresh(&history, &dates, &clock, since.as_ref(), truncated);
                if plan.commits.is_empty() {
                    // Nothing in the window: leave the store exactly as it was.
                    return Ok(Outcome::Nothing(match &since {
                        Some(since) => format!("no commits since {}", since.describe()),
                        None => "nothing to backfill".to_owned(),
                    }));
                }
                // Nothing was recorded yet, so this run's window is the one that counts, even
                // if an earlier attempt was killed before its first commit and left another.
                match &since {
                    Some(since) => recording.set_meta(SINCE, &since.describe()),
                    None => recording.remove_meta(SINCE),
                }
                .map_err(|error| format!("{error}"))?;
                (plan, Walk::empty(), false)
            }
        };
        recording
            .set_meta(BACKFILL_STATE, BACKFILL_RUNNING)
            .map_err(|error| format!("{error}"))?;
        recording.commit().map_err(|error| format!("{error}"))?;
        (plan, walk, resumed)
    };

    if !plan.commits.is_empty() {
        eprintln!(
            "marrow backfill: reading {} {} to plan the work",
            plan.commits.len(),
            if plan.commits.len() == 1 {
                "commit"
            } else {
                "commits"
            }
        );
    }
    let mut progress = Progress::new(plan.work(&repository, &walk)?);
    let mut loader = Loader::default();
    let mut summary = Summary {
        commits: plan.commits.len(),
        through: String::new(),
        counts: marrow_store::Counts::default(),
        resumed,
        trailers: 0,
        carried: Vec::new(),
        skipped: Vec::new(),
    };
    for (index, id) in plan.commits.iter().enumerate() {
        let started = Instant::now();
        let info = repository.commit(*id).map_err(|error| format!("{error}"))?;
        let loaded = loader
            .load(&repository, *id)
            .map_err(|error| format!("can't read commit {}: {error}", short(&info.sha)))?;
        let mut current = loaded.files;
        // A file version tree-sitter gives up on is treated as unchanged until a version of it
        // parses again. Calling its lines dead and then reborn would invent history.
        for path in loaded.unparsed {
            match walk.previous.get(&path) {
                Some(file) => {
                    current.insert(
                        path.clone(),
                        TrackedFile {
                            content: file.content,
                            lines: Arc::clone(&file.lines),
                        },
                    );
                    summary.carried.push((info.sha.clone(), path));
                }
                None => summary.skipped.push((info.sha.clone(), path)),
            }
        }
        if info.agent_trailer.is_some() {
            summary.trailers += 1;
        }
        let baseline = plan.baseline && index == 0;
        let recording = store.begin().map_err(|error| format!("{error}"))?;
        record_commit(
            &recording,
            &mut walk,
            &info,
            plan.clock[index],
            current,
            baseline,
        )?;
        recording
            .set_meta(LAST_COMMIT, &info.sha)
            .map_err(|error| format!("{error}"))?;
        if let Some((_, clock)) = &walk.previous_commit {
            recording
                .set_meta(CLOCK, &clock.to_string())
                .map_err(|error| format!("{error}"))?;
        }
        crash_for_tests(index);
        recording.commit().map_err(|error| format!("{error}"))?;
        summary.through = info.sha;
        progress.advance(started.elapsed());
    }
    progress.finish();
    summary.counts = finish(&mut store, &walk)?;
    if summary.through.is_empty() {
        summary.through = walk
            .previous_commit
            .as_ref()
            .map(|(sha, _)| sha.clone())
            .unwrap_or_default();
    }
    Ok(Outcome::Done(summary))
}

/// Which commits a run records, oldest first.
struct Plan {
    commits: Vec<CommitId>,
    /// The walk's clock at each of those commits.
    clock: Vec<i64>,
    /// Whether the first commit is where watching begins rather than where its lines were born.
    baseline: bool,
}

impl Plan {
    fn after(history: &[CommitId], clock: &[i64], at: usize) -> Plan {
        Plan {
            commits: history[at + 1..].to_vec(),
            clock: clock[at + 1..].to_vec(),
            baseline: false,
        }
    }

    fn fresh(
        history: &[CommitId],
        dates: &[i64],
        clock: &[i64],
        since: Option<&Since>,
        truncated: bool,
    ) -> Plan {
        // The first commit of the window: every commit from here on is recorded. When it isn't
        // the root, the commit before it is recorded too, as the baseline the window starts from.
        // A date window goes by the commits' own dates, as `git log --since` does.
        let first = match since {
            None => 0,
            Some(Since::Commits(count)) => history.len().saturating_sub(*count),
            Some(Since::Date { at, .. }) => dates
                .iter()
                .position(|date| date >= at)
                .unwrap_or(history.len()),
        };
        if first == history.len() {
            return Plan {
                commits: Vec::new(),
                clock: Vec::new(),
                baseline: false,
            };
        }
        // Everything in a root commit was born there, so a root is never a baseline. The oldest
        // commit of a shallow clone isn't a root, though: its lines were born before it, at a
        // time the clone doesn't have, so it always is one.
        let first = if truncated { first.max(1) } else { first };
        let start = if first <= 1 && !truncated {
            0
        } else {
            first - 1
        };
        Plan {
            commits: history[start..].to_vec(),
            clock: clock[start..].to_vec(),
            baseline: start > 0 || truncated,
        }
    }

    /// What each commit will cost, in the two things that take the time: walking a tree of
    /// tracked files, and parsing and aligning the files that changed. Read from the trees and
    /// the object headers alone, without parsing anything.
    fn work(&self, repository: &Repository, walk: &Walk) -> Result<Vec<Work>, String> {
        let mut previous: BTreeMap<String, ContentId> = walk
            .previous
            .iter()
            .map(|(path, file)| (path.clone(), file.content))
            .collect();
        let mut work = Vec::with_capacity(self.commits.len());
        for id in &self.commits {
            let files =
                pipeline::tracked_files(repository, *id).map_err(|error| format!("{error}"))?;
            let mut changed_bytes = 0;
            for (path, content) in &files {
                if previous.get(path) != Some(content) {
                    changed_bytes += repository
                        .blob_size(*content)
                        .map_err(|error| format!("{error}"))?;
                }
            }
            work.push(Work {
                files: files.len() as f64,
                changed_kb: changed_bytes as f64 / 1024.0,
            });
            previous = files;
        }
        Ok(work)
    }
}

impl Walk {
    fn empty() -> Walk {
        Walk {
            previous: Snapshot::new(),
            live: HashMap::new(),
            previous_commit: None,
        }
    }

    fn line_id(&self, at: &Position) -> Result<i64, String> {
        self.live
            .get(&at.path)
            .and_then(|lines| lines.get(&at.line))
            .copied()
            .ok_or_else(|| format!("no recorded line at {}:{}", at.path, at.line))
    }
}

/// Picks a walk back up from the store: the files it last recorded, and the lines in them.
fn restore(
    recording: &Recording<'_>,
    repository: &Repository,
    last: CommitId,
) -> Result<Walk, String> {
    let info = repository
        .commit(last)
        .map_err(|error| format!("{error}"))?;
    let files = pipeline::tracked_files(repository, last).map_err(|error| format!("{error}"))?;
    let by_hash: HashMap<String, &String> =
        files.keys().map(|path| (path_hash(path), path)).collect();
    let mut walk = Walk::empty();
    for hash in recording
        .snapshot_files()
        .map_err(|error| format!("{error}"))?
    {
        let snapshot = recording
            .snapshot(&hash)
            .map_err(|error| format!("{error}"))?
            .expect("a listed snapshot can be read");
        if snapshot.normalizer_version != NORMALIZER_VERSION {
            return Err(format!(
                "the backfill here was made with normalizer version {}, this build is \
                 {NORMALIZER_VERSION}; delete .marrow/ and run marrow backfill again",
                snapshot.normalizer_version
            ));
        }
        let Some(path) = by_hash.get(&hash) else {
            return Err(format!(
                "the store's files don't match commit {}; delete .marrow/ and run marrow \
                 backfill again",
                short(&info.sha)
            ));
        };
        walk.live.insert(
            (*path).clone(),
            snapshot
                .lines
                .iter()
                .map(|line| (line.line_number, line.line_id))
                .collect(),
        );
        walk.previous.insert(
            (*path).clone(),
            TrackedFile {
                content: snapshot
                    .content_id
                    .as_deref()
                    .and_then(ContentId::from_hex)
                    .unwrap_or(ContentId::UNKNOWN),
                lines: Arc::new(snapshot.lines.iter().map(engine_line).collect()),
            },
        );
    }
    let clock = recording
        .meta(CLOCK)
        .map_err(|error| format!("{error}"))?
        .and_then(|clock| clock.parse().ok())
        .unwrap_or(info.committed_at);
    walk.previous_commit = Some((info.sha, clock));
    Ok(walk)
}

/// Records one commit: its row, the fate of every line it changed, and the lines it added.
///
/// Only the files the commit changed are compared. A file with the same contents at the same
/// path can't give lines to a move or take them from one, since all its lines match themselves,
/// so leaving it out decides exactly what comparing the whole tree would, in time proportional
/// to the change rather than to the repository.
fn record_commit(
    recording: &Recording<'_>,
    walk: &mut Walk,
    info: &CommitInfo,
    planned: i64,
    current: Snapshot,
    baseline: bool,
) -> Result<(), String> {
    recording
        .insert_commit(&CommitRow {
            sha: &info.sha,
            authored_at: info.authored_at,
            committed_at: info.committed_at,
            is_merge: info.is_merge,
            observed_by: ObservedBy::Backfill,
            agent_trailer: info.agent_trailer.as_deref(),
        })
        .map_err(|error| format!("{error}"))?;
    // The planned clock, and never behind a clock already recorded: a walk resumed after new
    // commits arrived plans over a longer history, which can move an earlier part of the fit.
    let observed_at = walk
        .previous_commit
        .as_ref()
        .map_or(planned, |(_, at)| planned.max(*at));
    let same =
        |old: &TrackedFile, new: &TrackedFile| old.content.is_known() && old.content == new.content;
    let old_side: Snapshot = walk
        .previous
        .iter()
        .filter(|(path, old)| current.get(*path).is_none_or(|new| !same(old, new)))
        .map(|(path, file)| (path.clone(), share(file)))
        .collect();
    let new_side: Snapshot = current
        .iter()
        .filter(|(path, new)| walk.previous.get(*path).is_none_or(|old| !same(old, new)))
        .map(|(path, file)| (path.clone(), share(file)))
        .collect();

    let (previous_sha, previous_at) = walk
        .previous_commit
        .clone()
        .map(|(sha, at)| (Some(sha), at))
        .unwrap_or((None, observed_at));
    let decisions = pipeline::compare_snapshots(&old_side, &new_side);
    let mut identity: HashMap<Position, i64> = HashMap::new();
    for found in &decisions.matches {
        let line_id = walk.line_id(&found.old)?;
        if let Some(already) = identity.insert(found.new.clone(), line_id) {
            return Err(format!(
                "lines {already} and {line_id} both matched {}:{} in {}",
                found.new.path,
                found.new.line,
                short(&info.sha)
            ));
        }
        if found.state == State::Verbatim {
            continue;
        }
        let now = line_at(&new_side, &found.new)?;
        let birth = recording
            .birth_bigrams(line_id)
            .map_err(|error| format!("{error}"))?;
        recording
            .insert_fate(&FateRow {
                line_id,
                observed_at,
                commit: Some(&info.sha),
                previous_seen_at: Some(previous_at),
                state: found.state.as_str(),
                similarity_score: found.similarity,
                deciding_layer: found.layer.as_str(),
                matched_candidate_id: None,
                line_number: Some(found.new.line),
                file_path_hash: Some(&path_hash(&found.new.path)),
                birth_similarity: Some(bigram_dice(&birth, &now.bigrams)),
                // Every line a backfill records was born at a commit.
                before_first_commit: false,
            })
            .map_err(|error| format!("{error}"))?;
    }

    for (path, file) in &new_side {
        let hash = path_hash(path);
        for line in file.lines.iter() {
            let position = Position {
                path: path.clone(),
                line: line.number,
            };
            if identity.contains_key(&position) {
                continue;
            }
            let line_id = recording
                .insert_line(&BornLine {
                    file_path_hash: &hash,
                    birth_commit: Some(&info.sha),
                    birth_ts: observed_at,
                    birth_line_number: line.number,
                    birth_observed: !baseline,
                    birth_bigrams: &line.bigrams,
                    origin: Origin::Human,
                    session_id: None,
                    model: None,
                    syntactic_role: line.role,
                    token_count: line.tokens.len() as u32,
                })
                .map_err(|error| format!("{error}"))?;
            identity.insert(position, line_id);
        }
    }

    // Deaths last, so the line each one was most nearly like has an id even if it was just born.
    let mut dead = Vec::with_capacity(decisions.deaths.len());
    for death in &decisions.deaths {
        let line_id = walk.line_id(&death.old)?;
        dead.push(line_id);
        recording
            .insert_fate(&FateRow {
                line_id,
                observed_at,
                commit: Some(&info.sha),
                previous_seen_at: Some(previous_at),
                state: State::Dead.as_str(),
                similarity_score: death.similarity,
                deciding_layer: death.layer.as_str(),
                matched_candidate_id: death
                    .candidate
                    .as_ref()
                    .and_then(|position| identity.get(position))
                    .copied(),
                line_number: None,
                file_path_hash: None,
                birth_similarity: None,
                before_first_commit: false,
            })
            .map_err(|error| format!("{error}"))?;
    }
    // A line that died was last seen alive at the commit before. Lines still alive are
    // checkpointed once, at the end of the walk.
    recording
        .mark_alive(&dead, previous_at, previous_sha.as_deref())
        .map_err(|error| format!("{error}"))?;

    for path in old_side.keys() {
        walk.live.remove(path);
        if !current.contains_key(path) {
            recording
                .forget_snapshot(&path_hash(path))
                .map_err(|error| format!("{error}"))?;
        }
    }
    for (path, file) in &new_side {
        let lines: HashMap<u32, i64> = file
            .lines
            .iter()
            .map(|line| {
                let position = Position {
                    path: path.clone(),
                    line: line.number,
                };
                (line.number, identity[&position])
            })
            .collect();
        let snapshot = marrow_store::Snapshot {
            language: tracked_language(path)
                .map(language_name)
                .unwrap_or_default(),
            normalizer_version: NORMALIZER_VERSION,
            observed_at,
            content_id: file.content.is_known().then(|| file.content.to_hex()),
            lines: file
                .lines
                .iter()
                .map(|line| stored_line(lines[&line.number], line))
                .collect(),
        };
        recording
            .replace_snapshot(&path_hash(path), &snapshot)
            .map_err(|error| format!("{error}"))?;
        walk.live.insert(path.clone(), lines);
    }
    walk.previous = current;
    walk.previous_commit = Some((info.sha.clone(), observed_at));
    Ok(())
}

/// The line at a position the comparison decided on.
fn line_at<'a>(side: &'a Snapshot, at: &Position) -> Result<&'a Line, String> {
    side.get(&at.path)
        .and_then(|file| {
            file.lines
                .binary_search_by_key(&at.line, |line| line.number)
                .ok()
                .map(|index| &file.lines[index])
        })
        .ok_or_else(|| format!("no line at {}:{}", at.path, at.line))
}

fn share(file: &TrackedFile) -> TrackedFile {
    TrackedFile {
        content: file.content,
        lines: Arc::clone(&file.lines),
    }
}

/// Checkpoints every line still alive at the last commit recorded, and marks the backfill done.
fn finish(store: &mut Store, walk: &Walk) -> Result<marrow_store::Counts, String> {
    let recording = store.begin().map_err(|error| format!("{error}"))?;
    if let Some((sha, at)) = &walk.previous_commit {
        recording
            .mark_tracked_lines_alive(*at, sha)
            .map_err(|error| format!("{error}"))?;
        recording
            .touch_all_snapshots(*at)
            .map_err(|error| format!("{error}"))?;
    }
    recording
        .set_meta(BACKFILL_STATE, COMPLETE)
        .map_err(|error| format!("{error}"))?;
    let counts = recording.counts().map_err(|error| format!("{error}"))?;
    recording.commit().map_err(|error| format!("{error}"))?;
    Ok(counts)
}

/// The walk's clock: the commits' dates, made to run forwards by moving only the commits that are
/// out of place.
///
/// Committer dates nearly always rise along first-parent history, but a machine with a wrong
/// clock can date a commit far too early or far too late. A running maximum fixes one too early,
/// but one too late holds every commit after it at its date; a running minimum from the end is
/// the other way round. So the longest run of commits whose dates are already in order keeps
/// them (a longest non-decreasing subsequence), and every other commit is set as close to its
/// own date as its neighbours in that run allow.
fn monotone_clock(dates: &[i64]) -> Vec<i64> {
    // Patience sorting: `tails[k]` ends the best run of length k + 1 found so far.
    let mut tails: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = vec![None; dates.len()];
    for (index, &date) in dates.iter().enumerate() {
        let length = tails.partition_point(|&tail| dates[tail] <= date);
        previous[index] = length.checked_sub(1).map(|shorter| tails[shorter]);
        if length == tails.len() {
            tails.push(index);
        } else {
            tails[length] = index;
        }
    }
    let mut kept = vec![false; dates.len()];
    let mut at = tails.last().copied();
    while let Some(index) = at {
        kept[index] = true;
        at = previous[index];
    }
    // The next kept date after each commit, the ceiling an out-of-place commit is held under.
    let mut ceiling = vec![i64::MAX; dates.len()];
    let mut next = i64::MAX;
    for index in (0..dates.len()).rev() {
        ceiling[index] = next;
        if kept[index] {
            next = dates[index];
        }
    }
    let mut clock = Vec::with_capacity(dates.len());
    let mut floor = i64::MIN;
    for (index, &date) in dates.iter().enumerate() {
        let at = if kept[index] {
            date
        } else {
            date.min(ceiling[index]).max(floor)
        };
        clock.push(at);
        floor = at;
    }
    clock
}

/// Debug builds only: stop dead in the middle of recording a commit, the way a kill would, so
/// the tests can check that resuming gives the same result as never having stopped.
fn crash_for_tests(index: usize) {
    if cfg!(debug_assertions)
        && std::env::var("MARROW_TEST_CRASH_BACKFILL_AT")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            == Some(index)
    {
        std::process::exit(101);
    }
}

pub fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

/// A day as `YYYY-MM-DD`, as the Unix time of its first second in UTC.
fn parse_day(text: &str) -> Option<i64> {
    let mut parts = text.splitn(3, '-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    let (year, month, day): (i64, i64, i64) =
        (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=days_in_month).contains(&day) {
        return None;
    }
    // Days from 1970-01-01, from Howard Hinnant's `days_from_civil`.
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some((era * 146_097 + day_of_era - 719_468) * 86_400)
}

/// What one commit costs to record: a walk over a tree of `files` tracked files, and parsing
/// and aligning `changed_kb` kilobytes of files that changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Work {
    files: f64,
    changed_kb: f64,
}

impl Work {
    /// A rough single number for "how far along", used for the percentage. The time estimate
    /// doesn't use it; it learns the real costs as it goes.
    fn size(self) -> f64 {
        1.0 + self.changed_kb + self.files / 100.0
    }
}

/// Fits how long a commit takes as `per_file * files + per_kb * changed_kb`, by least squares
/// over the commits recorded so far. Two costs, because a history has two kinds of commit: its
/// first reads every file, and later ones mostly walk a big tree to change a small part of it.
/// Refitted after every commit, so it follows a machine's speed and files that grow over time.
#[derive(Clone, Debug, Default)]
struct Fit {
    ff: f64,
    fk: f64,
    kk: f64,
    fd: f64,
    kd: f64,
}

impl Fit {
    fn add(&mut self, work: Work, seconds: f64) {
        let (f, k) = (work.files, work.changed_kb);
        self.ff += f * f;
        self.fk += f * k;
        self.kk += k * k;
        self.fd += f * seconds;
        self.kd += k * seconds;
    }

    /// Seconds per tracked file walked, and per kilobyte changed. Neither may be negative, so
    /// when the joint fit says one is, the other is fitted on its own.
    fn costs(&self) -> (f64, f64) {
        let single_file = if self.ff > 0.0 {
            self.fd / self.ff
        } else {
            0.0
        };
        let single_kb = if self.kk > 0.0 {
            self.kd / self.kk
        } else {
            0.0
        };
        let det = self.ff * self.kk - self.fk * self.fk;
        if det <= 1e-9 * self.ff * self.kk {
            return (single_file, 0.0);
        }
        let per_file = (self.kk * self.fd - self.fk * self.kd) / det;
        let per_kb = (self.ff * self.kd - self.fk * self.fd) / det;
        match (per_file < 0.0, per_kb < 0.0) {
            (false, false) => (per_file, per_kb),
            (true, _) => (0.0, single_kb),
            (_, true) => (single_file, 0.0),
        }
    }

    fn predict(&self, work: Work) -> f64 {
        let (per_file, per_kb) = self.costs();
        per_file * work.files + per_kb * work.changed_kb
    }
}

/// A progress line on stderr with an estimate of the time left. On a terminal it redraws in
/// place; anywhere else it writes a line every tenth of the way, so logs stay readable.
struct Progress {
    work: Vec<Work>,
    remaining: Work,
    total_size: f64,
    done_size: f64,
    done_commits: usize,
    fit: Fit,
    started: Instant,
    drawn: Option<Instant>,
    terminal: bool,
    logged_tenths: u64,
    width: usize,
}

impl Progress {
    fn new(work: Vec<Work>) -> Progress {
        let remaining = work.iter().fold(Work::default(), |sum, commit| Work {
            files: sum.files + commit.files,
            changed_kb: sum.changed_kb + commit.changed_kb,
        });
        let total_size = work
            .iter()
            .map(|commit| commit.size())
            .sum::<f64>()
            .max(1.0);
        Progress {
            work,
            remaining,
            total_size,
            done_size: 0.0,
            done_commits: 0,
            fit: Fit::default(),
            started: Instant::now(),
            drawn: None,
            terminal: std::io::stderr().is_terminal(),
            logged_tenths: 0,
            width: 0,
        }
    }

    fn advance(&mut self, took: Duration) {
        let work = self.work[self.done_commits];
        self.fit.add(work, took.as_secs_f64());
        self.remaining.files -= work.files;
        self.remaining.changed_kb -= work.changed_kb;
        self.done_size += work.size();
        self.done_commits += 1;
        let tenths = (self.done_size * 10.0 / self.total_size) as u64;
        if self.terminal {
            let due = self
                .drawn
                .is_none_or(|drawn| drawn.elapsed() >= Duration::from_millis(100));
            if due {
                self.draw("\r");
            }
        } else if tenths > self.logged_tenths && self.done_commits < self.work.len() {
            self.logged_tenths = tenths;
            self.draw("");
        }
    }

    fn draw(&mut self, prefix: &str) {
        let elapsed = self.started.elapsed();
        // Too early to say until a few commits of both kinds have been timed: a guess from the
        // first commit alone would be wrong in whichever direction the rest of history leans.
        let ready = elapsed >= Duration::from_secs(2)
            && self.done_commits >= 5
            && self.done_size * 20.0 >= self.total_size;
        let left = if ready {
            let seconds = self.fit.predict(self.remaining).max(0.0);
            format!("about {} left", duration(Duration::from_secs_f64(seconds)))
        } else {
            "estimating time left".to_owned()
        };
        let line = format!(
            "marrow backfill: {}/{} commits, {}%, {} elapsed, {}",
            self.done_commits,
            self.work.len(),
            (self.done_size * 100.0 / self.total_size) as u64,
            duration(elapsed),
            left
        );
        let padding = self.width.saturating_sub(line.len());
        let mut stderr = std::io::stderr().lock();
        if prefix.is_empty() {
            let _ = writeln!(stderr, "{line}");
        } else {
            let _ = write!(stderr, "{prefix}{line}{}", " ".repeat(padding));
            let _ = stderr.flush();
        }
        self.width = line.len();
        self.drawn = Some(Instant::now());
    }

    fn finish(&mut self) {
        if self.terminal && self.drawn.is_some() {
            self.draw("\r");
            let _ = writeln!(std::io::stderr());
        }
    }
}

/// Rounded to the nearest second, not truncated, so 0.9s left doesn't read as "0s".
fn duration(duration: Duration) -> String {
    let seconds = duration.as_secs_f64().round() as u64;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::{duration, monotone_clock, parse_day, Fit, Since, Work};
    use std::time::Duration;

    #[test]
    fn days_are_read_as_midnight_utc() {
        assert_eq!(parse_day("1970-01-01"), Some(0));
        assert_eq!(parse_day("2000-03-01"), Some(951_868_800));
        assert_eq!(parse_day("2024-02-29"), Some(1_709_164_800));
        assert_eq!(parse_day("2023-02-29"), None);
        assert_eq!(parse_day("2026-13-01"), None);
        assert_eq!(parse_day("26-01-01"), None);
        assert_eq!(parse_day("yesterday"), None);
    }

    #[test]
    fn since_takes_a_count_or_a_day_and_nothing_else() {
        assert_eq!(Since::parse("50"), Ok(Since::Commits(50)));
        assert!(matches!(Since::parse("2026-01-31"), Ok(Since::Date { .. })));
        assert!(Since::parse("0").is_err());
        assert!(Since::parse("last week").is_err());
        assert!(Since::parse("").is_err());
    }

    #[test]
    fn the_fit_recovers_both_costs_and_never_goes_negative() {
        let mut fit = Fit::default();
        // A heavy first commit, then small changes in a big tree: 2 ms a file, 5 ms a kilobyte.
        for (files, changed_kb) in [(100.0, 400.0), (100.0, 2.0), (120.0, 3.0), (130.0, 1.0)] {
            let work = Work { files, changed_kb };
            fit.add(work, 0.002 * files + 0.005 * changed_kb);
        }
        let (per_file, per_kb) = fit.costs();
        assert!((per_file - 0.002).abs() < 1e-9, "{per_file}");
        assert!((per_kb - 0.005).abs() < 1e-9, "{per_kb}");

        // Time that only ever falls as trees grow would fit a negative cost; it's refused.
        let mut odd = Fit::default();
        odd.add(
            Work {
                files: 10.0,
                changed_kb: 1.0,
            },
            1.0,
        );
        odd.add(
            Work {
                files: 20.0,
                changed_kb: 1.0,
            },
            0.5,
        );
        let (per_file, per_kb) = odd.costs();
        assert!(per_file >= 0.0 && per_kb >= 0.0, "{per_file} {per_kb}");
    }

    #[test]
    fn the_clock_moves_only_the_commits_out_of_place() {
        assert_eq!(monotone_clock(&[1, 2, 3, 5, 8]), [1, 2, 3, 5, 8]);
        // A machine years ahead: that commit waits for the next, and nothing after it moves.
        assert_eq!(
            monotone_clock(&[10, 20, 9_000, 30, 40, 50]),
            [10, 20, 30, 30, 40, 50]
        );
        // A machine behind: that commit catches up with the one before, and nothing before it
        // moves.
        assert_eq!(
            monotone_clock(&[10, 20, 30, 5, 40, 50]),
            [10, 20, 30, 30, 40, 50]
        );
        // A few commits from a wrong clock give way to the longer run that is in order.
        assert_eq!(
            monotone_clock(&[10, 20, 100, 110, 120, 30, 40, 50, 60]),
            [10, 20, 30, 30, 30, 30, 40, 50, 60]
        );
        let clock = monotone_clock(&[3, 1, 2, 9, 4, 7, 7, 6]);
        assert!(clock.windows(2).all(|pair| pair[0] <= pair[1]), "{clock:?}");
        assert_eq!(monotone_clock(&[]), Vec::<i64>::new());
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(Duration::from_secs(42)), "42s");
        assert_eq!(duration(Duration::from_secs(190)), "3m 10s");
        assert_eq!(duration(Duration::from_secs(3_900)), "1h 05m");
    }
}
