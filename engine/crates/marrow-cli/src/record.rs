//! `marrow record`: capture one agent write (PRD §7.2).

use std::io::Read;
use std::path::{Path, PathBuf};

use marrow_core::language::Language;
use marrow_core::normalize::{normalize, Line, NORMALIZER_VERSION};
use marrow_core::pipeline::{compare_file, LineFate};
use marrow_core::select::{tracked_language, tracked_source};
use marrow_core::{path_hash, ContentId, State};
use marrow_store::{BornLine, FateRow, Origin, Snapshot};

use crate::capture::{
    engine_line, language_name, now, open_store, relative_path, repository_root, stored_line, TOOL,
};

pub enum Capture {
    Recorded { born: usize, changed: usize },
    Skipped(String),
}

/// Claude Code sends a hook its payload as JSON on stdin.
pub struct HookPayload {
    pub session: Option<String>,
    pub file: Option<PathBuf>,
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
    })
}

pub fn capture(session: &str, file: &Path, model: Option<&str>) -> Result<Capture, String> {
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
    recording
        .ensure_session(session, TOOL, model, observed_at)
        .map_err(|error| format!("{error}"))?;
    let previous = recording
        .snapshot(&hash)
        .map_err(|error| format!("{error}"))?;

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
            &hash,
            session,
            model,
            observed_at,
            &lines,
            &mut identity,
        )?;
        for (old_index, fate) in fates.iter().enumerate() {
            let line_id = previous.lines[old_index].line_id;
            let (state, similarity, layer, candidate, line_number) = match fate {
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
                    Some(lines[*new].number),
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
            recording
                .insert_fate(&FateRow {
                    line_id,
                    observed_at,
                    previous_seen_at: Some(previous_seen_at),
                    state,
                    similarity_score: similarity,
                    deciding_layer: layer,
                    matched_candidate_id: candidate,
                    line_number,
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
        return Ok(Capture::Recorded { born, changed });
    }

    let born = insert_born(
        &recording,
        &hash,
        session,
        model,
        observed_at,
        &lines,
        &mut identity,
    )?;
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
    Ok(Capture::Recorded { born, changed })
}

/// Every new line without an identity yet is born here, as agent-written.
fn insert_born(
    recording: &marrow_store::Recording<'_>,
    hash: &str,
    session: &str,
    model: Option<&str>,
    observed_at: i64,
    lines: &[Line],
    identity: &mut [Option<i64>],
) -> Result<usize, String> {
    let mut born = 0;
    for (index, line) in lines.iter().enumerate() {
        if identity[index].is_some() {
            continue;
        }
        identity[index] = Some(
            recording
                .insert_line(&BornLine {
                    file_path_hash: hash,
                    birth_commit: None,
                    birth_ts: observed_at,
                    origin: Origin::Agent,
                    session_id: Some(session),
                    model,
                    syntactic_role: line.role,
                    token_count: line.tokens.len() as u32,
                })
                .map_err(|error| format!("{error}"))?,
        );
        born += 1;
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
