//! `marrow record`: capture one agent write (PRD §7.2).

use std::io::Read;
use std::path::{Path, PathBuf};

use marrow_core::align::bigram_dice;
use marrow_core::history::Repository;
use marrow_core::language::Language;
use marrow_core::normalize::{normalize, Line, NORMALIZER_VERSION};
use marrow_core::pipeline::{compare_file, LineFate};
use marrow_core::select::{tracked_language, tracked_source};
use marrow_core::{path_hash, ContentId, State};
use marrow_store::{BornLine, FateRow, Origin, Snapshot};

use crate::capture::{
    backfill_unfinished, engine_line, language_name, now, open_store, relative_path,
    repository_root, stored_line, BACKFILL_UNFINISHED, TOOL,
};

pub enum Capture {
    Recorded {
        born: usize,
        changed: usize,
        /// Lines that were in the file before this write, from a version marrow never saw.
        already_there: usize,
    },
    Skipped(String),
}

/// Who made a write, and when.
struct Writer<'a> {
    session: &'a str,
    model: Option<&'a str>,
    observed_at: i64,
    file_path_hash: &'a str,
}

/// Claude Code sends a hook its payload as JSON on stdin.
pub struct HookPayload {
    pub session: Option<String>,
    pub file: Option<PathBuf>,
    /// `Write` replaces a whole file; `Edit` and friends change part of one.
    pub tool: Option<String>,
    /// `PreToolUse` or `PostToolUse`.
    pub event: Option<String>,
    pub cwd: Option<PathBuf>,
    /// The shell command, for a Bash tool call.
    pub command: Option<String>,
    /// Tells apart tool calls running side by side.
    pub tool_use_id: Option<String>,
}

pub fn read_hook_payload() -> Result<HookPayload, String> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|error| format!("can't read the hook payload: {error}"))?;
    let payload: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("the hook payload isn't valid JSON: {error}"))?;
    Ok(HookPayload {
        session: payload["session_id"].as_str().map(str::to_owned),
        file: payload["tool_input"]["file_path"]
            .as_str()
            .map(PathBuf::from),
        tool: payload["tool_name"].as_str().map(str::to_owned),
        event: payload["hook_event_name"].as_str().map(str::to_owned),
        cwd: payload["cwd"].as_str().map(PathBuf::from),
        command: payload["tool_input"]["command"].as_str().map(str::to_owned),
        tool_use_id: payload["tool_use_id"].as_str().map(str::to_owned),
    })
}

pub fn capture(
    session: &str,
    file: &Path,
    model: Option<&str>,
    tool: Option<&str>,
) -> Result<Capture, String> {
    let file = file
        .canonicalize()
        .map_err(|error| format!("can't find {}: {error}", file.display()))?;
    let Some(root) = repository_root(&file) else {
        return Ok(Capture::Skipped("not inside a git repository".to_owned()));
    };
    let Some(relative) = relative_path(&root, &file) else {
        return Ok(Capture::Skipped("outside the repository".to_owned()));
    };
    let Some(language) = tracked_language(&relative) else {
        return Ok(Capture::Skipped(
            "not a tracked language, or an excluded path".to_owned(),
        ));
    };
    let bytes = std::fs::read(&file).map_err(|error| format!("can't read {relative}: {error}"))?;
    let Some(source) = tracked_source(&bytes) else {
        return Ok(Capture::Skipped(
            "not UTF-8, or over the size limit".to_owned(),
        ));
    };
    let Some(lines) = normalize(source, language) else {
        return Ok(Capture::Skipped(
            "tree-sitter gave up parsing it".to_owned(),
        ));
    };

    let mut store = open_store(&root)?;

    let content = ContentId::of(&bytes);
    let observed_at = now();
    let hash = path_hash(&relative);
    let recording = store.begin().map_err(|error| format!("{error}"))?;
    if backfill_unfinished(&recording)? {
        return Ok(Capture::Skipped(BACKFILL_UNFINISHED.to_owned()));
    }
    recording
        .mark_capture_started(observed_at)
        .map_err(|error| format!("{error}"))?;
    recording
        .ensure_session(session, TOOL, model, observed_at)
        .map_err(|error| format!("{error}"))?;
    let previous = recording
        .snapshot(&hash)
        .map_err(|error| format!("{error}"))?;

    let writer = Writer {
        session,
        model,
        observed_at,
        file_path_hash: &hash,
    };
    let mut identity: Vec<Option<i64>> = vec![None; lines.len()];
    let mut changed = 0;
    if let Some(previous) = &previous {
        if previous.normalizer_version != NORMALIZER_VERSION {
            return Ok(Capture::Skipped(format!(
                "the store was built with normalizer version {}, this build is {NORMALIZER_VERSION}; rebuild it with marrow backfill",
                previous.normalizer_version
            )));
        }
        // Never later than now: a snapshot can carry a commit's author date, and a commit can
        // be dated ahead of the clock.
        let previous_seen_at = previous.observed_at.min(observed_at);
        let old_lines: Vec<Line> = previous.lines.iter().map(engine_line).collect();
        let fates = compare_file(&old_lines, &lines);
        for (old_index, fate) in fates.iter().enumerate() {
            if let LineFate::Kept { new, .. } = fate {
                identity[*new] = Some(previous.lines[old_index].line_id);
            }
        }
        let born = insert_born(
            &recording,
            &writer,
            &lines,
            &mut identity,
            &vec![false; lines.len()],
        )?;
        for (old_index, fate) in fates.iter().enumerate() {
            let line_id = previous.lines[old_index].line_id;
            let (state, similarity, layer, candidate, kept_at) = match fate {
                // Still there and unchanged: the snapshot's own timestamp records that it was
                // alive, so there is no state change to store.
                LineFate::Kept {
                    state: State::Verbatim,
                    ..
                } => continue,
                LineFate::Kept {
                    new,
                    state,
                    similarity,
                    layer,
                } => (
                    state.as_str(),
                    *similarity,
                    layer.as_str(),
                    None,
                    Some(&lines[*new]),
                ),
                LineFate::Dead {
                    similarity,
                    candidate,
                    layer,
                } => (
                    State::Dead.as_str(),
                    *similarity,
                    layer.as_str(),
                    candidate.and_then(|index| identity[index]),
                    None,
                ),
            };
            let before_first_commit = !recording
                .committed(line_id)
                .map_err(|error| format!("{error}"))?;
            let birth_similarity = match kept_at {
                Some(line) => Some(bigram_dice(
                    &recording
                        .birth_bigrams(line_id)
                        .map_err(|error| format!("{error}"))?,
                    &line.bigrams,
                )),
                None => None,
            };
            recording
                .insert_fate(&FateRow {
                    line_id,
                    observed_at,
                    commit: None,
                    previous_seen_at: Some(previous_seen_at),
                    state,
                    similarity_score: similarity,
                    deciding_layer: layer,
                    matched_candidate_id: candidate,
                    line_number: kept_at.map(|line| line.number),
                    file_path_hash: kept_at.map(|_| hash.as_str()),
                    birth_similarity,
                    before_first_commit,
                })
                .map_err(|error| format!("{error}"))?;
            changed += 1;
        }
        // Everything still in the file is known alive as of this write.
        let alive: Vec<i64> = identity.iter().flatten().copied().collect();
        recording
            .mark_alive(&alive, observed_at, None)
            .map_err(|error| format!("{error}"))?;
        write_snapshot(
            &recording,
            &hash,
            language,
            observed_at,
            content,
            &lines,
            &identity,
        )?;
        recording.commit().map_err(|error| format!("{error}"))?;
        return Ok(Capture::Recorded {
            born,
            changed,
            already_there: 0,
        });
    }

    // Marrow has never seen this file. Whatever it held before this write wasn't the agent's,
    // so only lines the write can be shown to have added are credited to it (ADR 0010).
    let already = already_there(&root, &relative, language, &lines, tool);
    let born = insert_born(&recording, &writer, &lines, &mut identity, &already)?;
    write_snapshot(
        &recording,
        &hash,
        language,
        observed_at,
        content,
        &lines,
        &identity,
    )?;
    recording.commit().map_err(|error| format!("{error}"))?;
    Ok(Capture::Recorded {
        born,
        changed,
        already_there: already.iter().filter(|there| **there).count(),
    })
}

/// Which lines of a file marrow has never seen were there before the agent's write.
///
/// If HEAD has the file, the lines that match its committed version were. If it doesn't, a
/// write of the whole file put every line there, but an edit changed a file whose earlier
/// contents nothing recorded, so none of its lines can be credited to the agent.
fn already_there(
    root: &Path,
    relative: &str,
    language: Language,
    lines: &[Line],
    tool: Option<&str>,
) -> Vec<bool> {
    if let Some(committed) = committed_version(root, relative, language) {
        let mut there = vec![false; lines.len()];
        for fate in compare_file(&committed, lines) {
            if let LineFate::Kept { new, .. } = fate {
                there[new] = true;
            }
        }
        return there;
    }
    let whole_file = tool.is_none_or(|tool| tool == "Write");
    vec![!whole_file; lines.len()]
}

/// The file as HEAD has it, normalized, or None if HEAD doesn't have it or can't be read.
fn committed_version(root: &Path, relative: &str, language: Language) -> Option<Vec<Line>> {
    let repository = Repository::open(root).ok()?;
    if !repository.has_commits().ok()? {
        return None;
    }
    let head = repository.head().ok()?;
    let bytes = repository.file_at(head.id, relative).ok()??;
    normalize(tracked_source(&bytes)?, language)
}

/// Every new line without an identity yet is born here: as the agent's, unless it was already
/// in the file, in which case its real birth is earlier and unknown, and it stays out of the
/// survival curves. Returns how many lines were credited to the agent.
fn insert_born(
    recording: &marrow_store::Recording<'_>,
    writer: &Writer<'_>,
    lines: &[Line],
    identity: &mut [Option<i64>],
    already_there: &[bool],
) -> Result<usize, String> {
    let mut born = 0;
    for (index, line) in lines.iter().enumerate() {
        if identity[index].is_some() {
            continue;
        }
        let agent = !already_there[index];
        identity[index] = Some(
            recording
                .insert_line(&BornLine {
                    file_path_hash: writer.file_path_hash,
                    birth_commit: None,
                    birth_ts: writer.observed_at,
                    birth_line_number: line.number,
                    birth_observed: agent,
                    birth_bigrams: &line.bigrams,
                    origin: if agent { Origin::Agent } else { Origin::Human },
                    session_id: agent.then_some(writer.session),
                    model: if agent { writer.model } else { None },
                    syntactic_role: line.role,
                    token_count: line.tokens.len() as u32,
                })
                .map_err(|error| format!("{error}"))?,
        );
        if agent {
            born += 1;
        }
    }
    Ok(born)
}

fn write_snapshot(
    recording: &marrow_store::Recording<'_>,
    hash: &str,
    language: Language,
    observed_at: i64,
    content: ContentId,
    lines: &[Line],
    identity: &[Option<i64>],
) -> Result<(), String> {
    let snapshot = Snapshot {
        language: language_name(language),
        normalizer_version: NORMALIZER_VERSION,
        observed_at,
        content_id: Some(content.to_hex()),
        lines: lines
            .iter()
            .zip(identity)
            .map(|(line, line_id)| stored_line(line_id.expect("every line has an identity"), line))
            .collect(),
    };
    recording
        .replace_snapshot(hash, &snapshot)
        .map_err(|error| format!("{error}"))
}
