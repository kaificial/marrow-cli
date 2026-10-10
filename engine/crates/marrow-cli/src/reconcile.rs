//! `marrow reconcile`: the post-commit checkpoint.
//!
//! A write-time hook only ever sees one file at a time, and it only fires for an agent's writes.
//! Reconciliation closes both gaps. It compares everything marrow knows about the tree against
//! the tree HEAD actually has, so it catches edits made by hand, files renamed with `git mv`,
//! blocks moved between files, deletions, and anything that happened while the hook wasn't
//! running. Every line still present is then stamped with the commit it was last seen in, which
//! is the checkpoint survival analysis measures against.
//!
//! HEAD says what the tree holds now, but not what this commit did to it, and a file can be
//! missing for reasons nobody performed: a branch switch, a reset, work that was never staged.
//! So each finding is checked against the commit's parents and against the working tree before
//! it is recorded. See docs/decisions/0008-post-commit-reconciliation.md.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use marrow_core::align::bigram_dice;
use marrow_core::history::{CommitInfo, Repository};
use marrow_core::language::Language;
use marrow_core::normalize::{normalize, Line, NORMALIZER_VERSION};
use marrow_core::pipeline::{self, Snapshot, TrackedFile};
use marrow_core::select::{tracked_language, tracked_source};
use marrow_core::{path_hash, ContentId, Position, State};
use marrow_store::{BornLine, CommitRow, FateRow, ObservedBy, Origin, Recording};

use crate::capture::{
    backfill_unfinished, engine_line, language_name, open_store, repository_root,
    working_tree_files, BACKFILL_UNFINISHED,
};

pub enum Outcome {
    Reconciled(Report),
    Skipped(String),
}

pub struct Report {
    pub commit: String,
    /// Lines HEAD has that marrow had never seen. Nothing captured them as an agent's work, so
    /// they are recorded as written by a person.
    pub born: usize,
    pub edited: usize,
    pub moved: usize,
    pub dead: usize,
    /// Lines confirmed still alive and unchanged, and so checkpointed at this commit.
    pub alive: usize,
    /// Lines captured from an agent write that now have the commit they first appeared in.
    pub anchored: usize,
    /// Agent-written lines a person has since changed.
    pub revised: usize,
    /// Files in HEAD that tree-sitter gave up on, whose lines were left untouched.
    pub unparsed: Vec<String>,
    /// Files whose contents changed without this commit changing them, so the committed version
    /// was taken as a new baseline and no fates were recorded.
    pub rebaselined: Vec<String>,
}

/// One side of the comparison, as the store holds it: which file, when it was last seen, and the
/// identity of each line in it.
struct StoredFile {
    hash: String,
    observed_at: i64,
    line_ids: HashMap<u32, i64>,
}

pub fn reconcile(repo: Option<&Path>) -> Result<Outcome, String> {
    let start = match repo {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().map_err(|error| format!("can't find the cwd: {error}"))?,
    };
    let start = start.canonicalize().unwrap_or(start);
    let Some(root) = repository_root(&start) else {
        return Ok(Outcome::Skipped("not inside a git repository".to_owned()));
    };
    let repository = Repository::open(&root).map_err(|error| format!("{error}"))?;
    if !repository
        .has_commits()
        .map_err(|error| format!("{error}"))?
    {
        return Ok(Outcome::Skipped(
            "this repository has no commits yet".to_owned(),
        ));
    }
    let head = repository.head().map_err(|error| format!("{error}"))?;
    let committed = pipeline::tracked_files(&repository, head.id)
        .map_err(|error| format!("can't read the tree of {}: {error}", short(&head.sha)))?;

    let mut store = open_store(&root)?;
    let recording = store.begin().map_err(|error| format!("{error}"))?;
    if backfill_unfinished(&recording)? {
        return Ok(Outcome::Skipped(BACKFILL_UNFINISHED.to_owned()));
    }
    recording
        .mark_capture_started(head.committed_at)
        .map_err(|error| format!("{error}"))?;
    // With nothing recorded before this commit, every line in it was already there when marrow
    // started watching, so none of their births were seen — unless this is the root commit,
    // where everything in it was born.
    let births_observed = head.parents.is_empty()
        || recording
            .has_commits()
            .map_err(|error| format!("{error}"))?;
    recording
        .insert_commit(&CommitRow {
            sha: &head.sha,
            authored_at: head.authored_at,
            committed_at: head.committed_at,
            is_merge: head.is_merge,
            observed_by: ObservedBy::Reconcile,
            agent_trailer: head.agent_trailer.as_deref(),
        })
        .map_err(|error| format!("{error}"))?;

    // The store keeps only a hash of each path, so every path we can see is hashed and looked up
    // instead: the commit's own paths, the paths its parents had, and the paths on disk.
    let committed_by_hash: HashMap<String, &String> = committed
        .keys()
        .map(|path| (path_hash(path), path))
        .collect();
    let before = parent_trees(&repository, &head)?;
    let on_disk: HashMap<String, PathBuf> = working_tree_files(&root)
        .into_iter()
        .map(|(path, absolute)| (path_hash(&path), absolute))
        .collect();

    let mut report = Report::new(head.sha.clone());
    let observed_at = head.committed_at;
    let mut alive: Vec<i64> = Vec::new();
    let mut ahead_of_head: Vec<i64> = Vec::new();
    // Files reconciliation deliberately doesn't look at: already known, or known to be ahead
    // of the commit. Keeping them off the new side is what stops their lines being born again.
    let mut left_alone: HashSet<String> = HashSet::new();
    let mut old_side = Snapshot::new();
    let mut old_files: HashMap<String, StoredFile> = HashMap::new();
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
                "the store was built with normalizer version {}, this build is {NORMALIZER_VERSION}; rebuild it with marrow backfill",
                snapshot.normalizer_version
            ));
        }
        let mut file = StoredFile::new(hash.clone(), snapshot.observed_at);
        for line in &snapshot.lines {
            file.line_ids.insert(line.line_number, line.line_id);
        }
        let content = snapshot.content_id.as_deref().and_then(ContentId::from_hex);
        let line_ids = || file.line_ids.values().copied().collect::<Vec<i64>>();
        let key = match committed_by_hash.get(&hash) {
            // The commit doesn't have this file. It only died if the commit is what removed it,
            // which means a parent had it. A file no parent had is either work that was never
            // committed or a file from another branch, and neither is this commit's doing. A file
            // that was renamed rather than deleted is keyed by its path hash, which no real path
            // matches, and Layer 1 pairs files by content, so the rename still finds its way
            // across.
            None => {
                if on_disk.contains_key(&hash) {
                    // Written, not committed. Alive on the evidence of the file itself.
                    ahead_of_head.extend(line_ids());
                    continue;
                }
                if !before.any_parent_had(&hash) {
                    // Not in the tree, and no commit took it away. There is no evidence of what
                    // became of these lines, so they keep the checkpoint they already have and
                    // stay censored there rather than being called alive or dead.
                    continue;
                }
                hash.clone()
            }
            Some(path) => {
                // Contents we have already seen need no comparison: every line is alive and
                // unchanged, whether or not the parser can still read the file. Anything else
                // is decided once both sides are normalized, not from the bytes.
                if content.is_some() && content == Some(committed[*path]) {
                    left_alone.insert((*path).clone());
                    alive.extend(line_ids());
                    recording
                        .touch_snapshot(&hash, observed_at)
                        .map_err(|error| format!("{error}"))?;
                    continue;
                }
                (*path).clone()
            }
        };
        old_side.insert(
            key.clone(),
            TrackedFile {
                content: content.unwrap_or(ContentId::UNKNOWN),
                lines: Arc::new(snapshot.lines.iter().map(engine_line).collect()),
            },
        );
        old_files.insert(key, file);
    }

    let loaded =
        pipeline::snapshot_of_commit(&repository, head.id, |path| !left_alone.contains(path))
            .map_err(|error| format!("can't read the tree of {}: {error}", short(&head.sha)))?;
    report.unparsed = loaded.unparsed;
    let mut new_side = loaded.files;
    // A file HEAD has but the parser can't read is left exactly as it was: its lines keep the
    // checkpoint they already had rather than being called dead on no evidence.
    for path in &report.unparsed {
        old_side.remove(path);
        old_files.remove(path);
    }
    let languages: BTreeMap<String, Language> = new_side
        .keys()
        .filter_map(|path| tracked_language(path).map(|language| (path.clone(), language)))
        .collect();

    // Both sides are normalized now, so what changed can be decided line by line rather than
    // byte by byte. That matters: where git rewrites line endings, a file's bytes never match
    // its own blob, and every comparison of ids would answer "different" forever.
    let mut ahead = Vec::new();
    for (key, file) in &old_files {
        let Some(committed_file) = new_side.get(key) else {
            continue; // Not in the commit at all: a deletion or a rename, which Layer 1 sorts out.
        };
        if same_lines(&old_side[key].lines, &committed_file.lines) {
            continue; // Committed exactly as we knew it. The comparison will say so, cheaply.
        }
        if languages
            .get(key)
            .and_then(|language| disk_lines(on_disk.get(&file.hash)?, *language))
            .is_some_and(|disk| same_lines(&old_side[key].lines, &disk))
        {
            // What we have is what's on disk, and the commit doesn't have it: the changes were
            // left out of the commit. The working tree stays the truth, so this file is left
            // exactly as it is — not compared, not replaced, and nothing born from it.
            ahead.push(key.clone());
            continue;
        }
        // The commit didn't touch this file, so whatever made it differ from our picture wasn't
        // a commit: a checkout, a reset, or work thrown away. Take the committed version as the
        // new baseline without recording fates nobody performed.
        if !before.commit_touched(key, committed[key]) {
            report.rebaselined.push(key.clone());
        }
    }
    for key in ahead {
        if let Some(file) = old_files.remove(&key) {
            ahead_of_head.extend(file.line_ids.values().copied());
        }
        old_side.remove(&key);
        new_side.remove(&key);
    }

    let decisions = pipeline::compare_snapshots(&old_side, &new_side);
    let rebaselined: HashSet<&String> = report.rebaselined.iter().collect();
    let mut identity: HashMap<Position, i64> = HashMap::new();
    let mut fates: Vec<(i64, FateKind)> = Vec::new();
    for found in &decisions.matches {
        let line_id = line_id(&old_files, &found.old)?;
        if let Some(already) = identity.insert(found.new.clone(), line_id) {
            return Err(format!(
                "lines {already} and {line_id} both matched {}:{}",
                found.new.path, found.new.line
            ));
        }
        alive.push(line_id);
        if found.state != State::Verbatim && !rebaselined.contains(&found.old.path) {
            fates.push((
                line_id,
                FateKind::Changed {
                    state: found.state,
                    similarity: found.similarity,
                    layer: found.layer.as_str(),
                    at: found.new.clone(),
                    previous_seen_at: seen_at(&old_files, &found.old, observed_at),
                },
            ));
        }
    }
    for death in &decisions.deaths {
        let line_id = line_id(&old_files, &death.old)?;
        if rebaselined.contains(&death.old.path) {
            // Not in the baseline we just took, and not killed by any commit either. The line
            // keeps the checkpoint it already had, which is what censoring it means.
            continue;
        }
        fates.push((
            line_id,
            FateKind::Dead {
                similarity: death.similarity,
                layer: death.layer.as_str(),
                previous_seen_at: seen_at(&old_files, &death.old, observed_at),
                candidate: death.candidate.clone(),
            },
        ));
    }

    // Anything in the commit that no stored line accounts for was written without the hook
    // seeing it, which means a person wrote it.
    for (path, file) in &new_side {
        let hash = path_hash(path);
        // A line this commit put there was born in it. One in a file the commit didn't touch
        // was there already, from a branch or a history marrow never walked, at a time nobody
        // recorded.
        let born_here = births_observed && before.commit_touched(path, committed[path]);
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
                    birth_commit: Some(&head.sha),
                    birth_ts: observed_at,
                    birth_line_number: line.number,
                    birth_observed: born_here,
                    birth_bigrams: &line.bigrams,
                    origin: Origin::Human,
                    session_id: None,
                    model: None,
                    syntactic_role: line.role,
                    token_count: line.tokens.len() as u32,
                })
                .map_err(|error| format!("{error}"))?;
            identity.insert(position, line_id);
            alive.push(line_id);
            report.born += 1;
        }
    }

    for (line_id, fate) in &fates {
        let at_hash;
        // Asked before this commit anchors anything, so a line being anchored now counts as not
        // yet committed.
        let before_first_commit = !recording
            .committed(*line_id)
            .map_err(|error| format!("{error}"))?;
        let row = match fate {
            FateKind::Changed {
                state,
                similarity,
                layer,
                at,
                previous_seen_at,
            } => {
                match state {
                    State::Moved => report.moved += 1,
                    _ => report.edited += 1,
                }
                // A person changing an agent's line is marrow's third origin (PRD §6). Moving a
                // line doesn't touch what it says, so a move isn't a revision.
                if *state == State::Edited
                    && recording
                        .mark_revised(*line_id)
                        .map_err(|error| format!("{error}"))?
                {
                    report.revised += 1;
                }
                let now = line_at(&new_side, at)?;
                let birth = recording
                    .birth_bigrams(*line_id)
                    .map_err(|error| format!("{error}"))?;
                at_hash = path_hash(&at.path);
                FateRow {
                    line_id: *line_id,
                    observed_at,
                    commit: Some(&head.sha),
                    previous_seen_at: Some(*previous_seen_at),
                    state: state.as_str(),
                    similarity_score: *similarity,
                    deciding_layer: layer,
                    matched_candidate_id: None,
                    line_number: Some(at.line),
                    file_path_hash: Some(&at_hash),
                    birth_similarity: Some(bigram_dice(&birth, &now.bigrams)),
                    before_first_commit,
                }
            }
            FateKind::Dead {
                similarity,
                layer,
                previous_seen_at,
                candidate,
            } => {
                report.dead += 1;
                FateRow {
                    line_id: *line_id,
                    observed_at,
                    commit: Some(&head.sha),
                    previous_seen_at: Some(*previous_seen_at),
                    state: State::Dead.as_str(),
                    similarity_score: *similarity,
                    deciding_layer: layer,
                    matched_candidate_id: candidate
                        .as_ref()
                        .and_then(|position| identity.get(position))
                        .copied(),
                    line_number: None,
                    file_path_hash: None,
                    birth_similarity: None,
                    before_first_commit,
                }
            }
        };
        recording
            .insert_fate(&row)
            .map_err(|error| format!("{error}"))?;
    }

    report.alive = alive.len();
    recording
        .mark_alive(&alive, observed_at, Some(&head.sha))
        .map_err(|error| format!("{error}"))?;
    // Alive, but not in this commit: uncommitted work, and files another branch holds. They get
    // the time without the commit, because the commit isn't evidence of where they are.
    recording
        .mark_alive(&ahead_of_head, observed_at, None)
        .map_err(|error| format!("{error}"))?;
    write_snapshots(
        &recording, &head, &new_side, &languages, &committed, &identity,
    )?;
    for (key, file) in &old_files {
        if !new_side.contains_key(key) {
            recording
                .forget_snapshot(&file.hash)
                .map_err(|error| format!("{error}"))?;
        }
    }
    // Last, because anchoring takes a line's committed version from the files' last known
    // states: after the commit's are written, and after any file it removed is forgotten, since
    // a line moved out of a renamed file is briefly in both.
    report.anchored = recording
        .anchor_births(&alive, &head.sha)
        .map_err(|error| format!("{error}"))?;
    recording.commit().map_err(|error| format!("{error}"))?;
    Ok(Outcome::Reconciled(report))
}

enum FateKind {
    Changed {
        state: State,
        similarity: f64,
        layer: &'static str,
        at: Position,
        previous_seen_at: i64,
    },
    Dead {
        similarity: f64,
        layer: &'static str,
        previous_seen_at: i64,
        /// Resolved to a line id only after every new line has one, births included.
        candidate: Option<Position>,
    },
}

/// The committed line at a position the comparison decided on.
fn line_at<'a>(side: &'a Snapshot, at: &Position) -> Result<&'a Line, String> {
    side.get(&at.path)
        .and_then(|file| {
            file.lines
                .binary_search_by_key(&at.line, |line| line.number)
                .ok()
                .map(|index| &file.lines[index])
        })
        .ok_or_else(|| format!("no committed line at {}:{}", at.path, at.line))
}

/// The committed version becomes the state the next write is compared against.
fn write_snapshots(
    recording: &Recording<'_>,
    head: &CommitInfo,
    new_side: &Snapshot,
    languages: &BTreeMap<String, Language>,
    committed: &BTreeMap<String, ContentId>,
    identity: &HashMap<Position, i64>,
) -> Result<(), String> {
    for (path, file) in new_side {
        let snapshot = marrow_store::Snapshot {
            language: language_name(languages[path]),
            normalizer_version: NORMALIZER_VERSION,
            observed_at: head.committed_at,
            content_id: committed.get(path).map(|content| content.to_hex()),
            lines: file
                .lines
                .iter()
                .map(|line| {
                    let position = Position {
                        path: path.clone(),
                        line: line.number,
                    };
                    crate::capture::stored_line(identity[&position], line)
                })
                .collect(),
        };
        recording
            .replace_snapshot(&path_hash(path), &snapshot)
            .map_err(|error| format!("{error}"))?;
    }
    Ok(())
}

/// Every line on the old side came out of the store, so every decision must map back to one.
fn line_id(files: &HashMap<String, StoredFile>, at: &Position) -> Result<i64, String> {
    files
        .get(&at.path)
        .and_then(|file| file.line_ids.get(&at.line))
        .copied()
        .ok_or_else(|| format!("no stored line at {}:{}", at.path, at.line))
}

/// The last time we knew the line was alive and unchanged, never later than now.
fn seen_at(files: &HashMap<String, StoredFile>, at: &Position, observed_at: i64) -> i64 {
    files
        .get(&at.path)
        .map(|file| file.observed_at.min(observed_at))
        .unwrap_or(observed_at)
}

impl StoredFile {
    fn new(hash: String, observed_at: i64) -> StoredFile {
        StoredFile {
            hash,
            observed_at,
            line_ids: HashMap::new(),
        }
    }
}

impl Report {
    fn new(commit: String) -> Report {
        Report {
            commit,
            born: 0,
            edited: 0,
            moved: 0,
            dead: 0,
            alive: 0,
            anchored: 0,
            revised: 0,
            unparsed: Vec::new(),
            rebaselined: Vec::new(),
        }
    }
}

pub fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

/// What the commit's parents held, which is how reconciliation tells a deletion from a file it
/// simply isn't looking at.
struct ParentTrees {
    /// Path hashes at least one parent had.
    hashes: HashSet<String>,
    blobs: Vec<BTreeMap<String, ContentId>>,
}

impl ParentTrees {
    /// A root commit deleted nothing. Otherwise the commit removed the file if any parent had it:
    /// for a merge, that includes a file one side deleted and the merge resolved to gone.
    fn any_parent_had(&self, hash: &str) -> bool {
        self.hashes.contains(hash)
    }

    /// Did this commit decide what is in the file? A root commit decides everything; otherwise
    /// the commit touched the file if any parent had something else there.
    fn commit_touched(&self, path: &str, content: ContentId) -> bool {
        self.blobs.is_empty()
            || self
                .blobs
                .iter()
                .any(|parent| parent.get(path) != Some(&content))
    }
}

fn parent_trees(repository: &Repository, head: &CommitInfo) -> Result<ParentTrees, String> {
    let mut blobs = Vec::new();
    for parent in &head.parents {
        blobs.push(
            pipeline::tracked_files(repository, *parent)
                .map_err(|error| format!("can't read the tree of {parent}: {error}"))?,
        );
    }
    let mut hashes = HashSet::new();
    for parent in &blobs {
        for path in parent.keys() {
            hashes.insert(path_hash(path));
        }
    }
    Ok(ParentTrees { hashes, blobs })
}

/// The tracked lines of a file as it sits in the working tree, or `None` if it can't be read.
fn disk_lines(path: &Path, language: Language) -> Option<Vec<Line>> {
    let bytes = std::fs::read(path).ok()?;
    normalize(tracked_source(&bytes)?, language)
}

/// Are these the same tracked lines, in the same places? Fingerprints come from the token stream,
/// so this is blind to line endings and to whitespace the normalizer drops.
fn same_lines(stored: &[Line], other: &[Line]) -> bool {
    stored.len() == other.len()
        && stored
            .iter()
            .zip(other)
            .all(|(line, same)| line.number == same.number && line.fingerprint == same.fingerprint)
}

/// Writes `.git/hooks/post-commit`, or explains what to add when the repository already has one.
pub fn install(repo: Option<&Path>) -> Result<String, String> {
    let start = match repo {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().map_err(|error| format!("can't find the cwd: {error}"))?,
    };
    let Some(root) = repository_root(&start.canonicalize().unwrap_or(start)) else {
        return Err("not inside a git repository".to_owned());
    };
    let executable = std::env::current_exe()
        .map_err(|error| format!("can't find the marrow executable: {error}"))?
        .display()
        .to_string()
        .replace('\\', "/");
    let command = format!("\"{executable}\" reconcile --repo \"$(git rev-parse --show-toplevel)\"");
    let hooks = root.join(".git").join("hooks");
    std::fs::create_dir_all(&hooks)
        .map_err(|error| format!("can't create {}: {error}", hooks.display()))?;
    let mut done = Vec::new();
    let mut refused = Vec::new();
    let script = format!(
        "#!/bin/sh\n\
         {INSTALLED}\n\
         # Its exit code is ignored by git, so a marrow problem can never fail a commit.\n\
         {command} || true\n"
    );
    for name in HOOKS {
        let hook = hooks.join(name);
        let verb = match std::fs::read_to_string(&hook) {
            // A hook marrow wrote is marrow's to rewrite, so running init with another binary
            // points it there. One a person wrote, or added marrow to, is left as it is.
            Ok(current) if current == script => {
                done.push(format!("{} already runs marrow", display(&hook)));
                continue;
            }
            Ok(current) if current.contains(INSTALLED) => "updated",
            Ok(current) if current.contains("marrow") => {
                done.push(format!("{} already runs marrow", display(&hook)));
                continue;
            }
            Ok(_) => {
                refused.push(format!(
                    "{} already exists. Add this line to it:\n{command}",
                    display(&hook)
                ));
                continue;
            }
            Err(_) => "installed",
        };
        std::fs::write(&hook, &script)
            .map_err(|error| format!("can't write {}: {error}", display(&hook)))?;
        make_executable(&hook);
        done.push(format!("{verb} {}", display(&hook)));
    }
    if refused.is_empty() {
        Ok(done.join("; "))
    } else {
        Err(done
            .into_iter()
            .chain(refused)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

/// The git hooks that reconcile. A merge or a pull makes a commit without running post-commit,
/// so post-merge covers those.
const HOOKS: [&str; 2] = ["post-commit", "post-merge"];

/// The line that marks a git hook as one marrow wrote.
const INSTALLED: &str =
    "# Installed by marrow. Records a commit-anchored checkpoint of every tracked line.";

/// Canonical paths on Windows carry a `\\?\` prefix, which is noise in a message.
fn display(path: &Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}
