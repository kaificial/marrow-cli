//! `marrow explain <file>:<line>`: the full audit trail for one line (PRD §7.1): where and how it
//! was born, every change marrow recorded with the score and rule behind it, and where it is now.

use std::fmt::Write;
use std::path::PathBuf;

use marrow_core::history::Repository;
use marrow_core::normalize::{normalize, Line};
use marrow_core::pipeline::{compare_file, LineFate};
use marrow_core::select::{tracked_language, tracked_source};
use marrow_core::{path_hash, ContentId, State};
use marrow_store::{FateRecord, LineRecord, Store};

use crate::capture::{date, engine_line, relative_path, repository_root};
use crate::paths::Paths;

/// How the line a person pointed at relates to the line marrow recorded.
enum OnDisk {
    /// The file is exactly what marrow last saw.
    AsRecorded,
    /// The file changed since, and this line descends from a recorded one: what happened to it,
    /// and whether it is at a different line number now.
    Changed(State, bool),
}

pub fn explain(target: &str) -> Result<String, String> {
    let (file, number) = parse(target)?;
    let cwd = std::env::current_dir().map_err(|error| format!("can't find the cwd: {error}"))?;
    let file = cwd.join(file);
    let file = file
        .canonicalize()
        .map_err(|error| format!("can't find {}: {error}", file.display()))?;
    let root = repository_root(&file).ok_or("not inside a git repository")?;
    let relative = relative_path(&root, &file).ok_or("outside the repository")?;
    let language = tracked_language(&relative)
        .ok_or_else(|| format!("{relative} isn't in a language marrow tracks"))?;
    let database = root.join(".marrow").join("db.sqlite");
    if !database.exists() {
        return Err(
            "marrow has no records for this repository yet; run marrow backfill".to_owned(),
        );
    }
    let store = Store::open(&database).map_err(|error| format!("{error}"))?;
    let snapshot = store
        .snapshot(&path_hash(&relative))
        .map_err(|error| format!("{error}"))?
        .ok_or_else(|| format!("marrow has no record of {relative}"))?;

    let bytes = std::fs::read(&file).map_err(|error| format!("can't read {relative}: {error}"))?;
    let source = tracked_source(&bytes)
        .ok_or_else(|| format!("{relative} isn't UTF-8, or is over the size limit"))?;
    let text = source.lines().nth(number as usize - 1).map(str::trim_end);
    let on_disk = normalize(source, language)
        .ok_or_else(|| format!("tree-sitter gave up parsing {relative}"))?;
    if !on_disk.iter().any(|line| line.number == number) {
        return Err(format!(
            "line {number} of {relative} has no code marrow tracks (it's blank, or past the end)"
        ));
    }
    let recorded: Vec<Line> = snapshot.lines.iter().map(engine_line).collect();
    let same_file =
        snapshot.content_id.as_deref().and_then(ContentId::from_hex) == Some(ContentId::of(&bytes));
    let (index, disk) = if same_file {
        let index = recorded
            .iter()
            .position(|line| line.number == number)
            .ok_or_else(|| format!("marrow has no record of line {number}"))?;
        (index, OnDisk::AsRecorded)
    } else {
        // The file has changed since marrow last saw it, so the line numbers may not line up.
        // The engine's own comparison says which recorded line this one descends from.
        let fates = compare_file(&recorded, &on_disk);
        fates
            .iter()
            .enumerate()
            .find_map(|(index, fate)| match fate {
                LineFate::Kept { new, state, .. } if on_disk[*new].number == number => Some((
                    index,
                    OnDisk::Changed(*state, recorded[index].number != number),
                )),
                _ => None,
            })
            .ok_or_else(|| {
                format!(
                    "line {number} of {relative} is new since marrow last saw the file; it's \
                     recorded at the next agent write or commit"
                )
            })?
    };
    let line_id = snapshot.lines[index].line_id;
    let line = store
        .line(line_id)
        .map_err(|error| format!("{error}"))?
        .ok_or_else(|| format!("line {line_id} is missing from the store"))?;
    let fates = store
        .fates_of(line_id)
        .map_err(|error| format!("{error}"))?;

    let repository = Repository::open(&root).map_err(|error| format!("{error}"))?;
    let mut paths = Paths::new(&root, &repository);
    for sha in line.birth_commit.iter().chain(
        fates
            .iter()
            .filter_map(|fate| fate.observed_commit.as_ref()),
    ) {
        paths.add_commit(&repository, sha);
    }

    let mut out = String::new();
    let _ = writeln!(out, "{relative}:{number}  (marrow line {line_id})");
    if let Some(text) = text {
        let _ = writeln!(out, "    {}", text.trim());
    }
    let _ = writeln!(out);
    born(&mut out, &line, &store, &repository, &paths)?;
    let _ = writeln!(out);
    history(&mut out, &fates, &store, &paths)?;
    let _ = writeln!(out);
    now(&mut out, &line, &snapshot.lines[index], &relative, disk);
    Ok(out)
}

fn parse(target: &str) -> Result<(PathBuf, u32), String> {
    let usage = || format!("expected <file>:<line>, like src/main.rs:12, not {target}");
    let (file, number) = target.rsplit_once(':').ok_or_else(usage)?;
    let number: u32 = number.parse().map_err(|_| usage())?;
    if file.is_empty() || number == 0 {
        return Err(usage());
    }
    Ok((PathBuf::from(file), number))
}

fn born(
    out: &mut String,
    line: &LineRecord,
    store: &Store,
    repository: &Repository,
    paths: &Paths,
) -> Result<(), String> {
    let _ = writeln!(out, "Born");
    let commit = match &line.birth_commit {
        Some(sha) => store.commit(sha).map_err(|error| format!("{error}"))?,
        None => None,
    };
    match (&line.birth_commit, line.session_id.as_ref()) {
        (Some(sha), _) => {
            let subject = marrow_core::history::CommitId::from_hex(sha.as_bytes())
                .ok()
                .and_then(|id| repository.commit(id).ok())
                .map(|info| format!(" \"{}\"", info.subject))
                .unwrap_or_default();
            let when = commit.as_ref().map_or(line.birth_ts, |c| c.committed_at);
            let _ = writeln!(out, "  {}, commit {}{subject}", date(when), short(sha));
        }
        (None, _) => {
            let _ = writeln!(
                out,
                "  {}, written by an agent and not committed yet",
                date(line.birth_ts)
            );
        }
    }
    let _ = writeln!(
        out,
        "  at {}:{}",
        paths.name(&line.file_path_hash),
        line.birth_line_number
    );
    let agent = |out: &mut String| {
        let model = line
            .model
            .as_deref()
            .map(|model| format!(", {model}"))
            .unwrap_or_default();
        let _ = write!(
            out,
            "  written by Claude Code (session {}{model}), recorded by the write hook",
            line.session_id.as_deref().unwrap_or("unknown")
        );
    };
    match line.origin.as_str() {
        "agent" => {
            agent(out);
            let _ = writeln!(out);
        }
        "agent_then_human_revised" => {
            agent(out);
            let _ = writeln!(out, ",\n  then revised by a person");
        }
        _ => {
            let how = match commit.as_ref().map(|c| c.observed_by.as_str()) {
                Some("backfill") => {
                    "recorded by the backfill, from history before marrow was watching"
                }
                _ => "it arrived in a commit without passing through the write hook",
            };
            let _ = writeln!(
                out,
                "  written by a person, as far as marrow can tell: {how}"
            );
        }
    }
    if !line.birth_observed {
        let _ = writeln!(
            out,
            "  it was already there when marrow started watching, so its real birth is earlier"
        );
    }
    if let Some(trailer) = commit.as_ref().and_then(|c| c.agent_trailer.as_ref()) {
        let _ = writeln!(
            out,
            "  the commit carries Claude Code's trailer ({trailer}), so an agent may have been \
             involved; the origin is left as recorded"
        );
    }
    Ok(())
}

fn history(
    out: &mut String,
    fates: &[FateRecord],
    store: &Store,
    paths: &Paths,
) -> Result<(), String> {
    let _ = writeln!(out, "What happened to it");
    if fates.is_empty() {
        let _ = writeln!(
            out,
            "  nothing: it has stayed exactly as it was born. Lines around it may have moved it \
             up or down, which isn't a change to the line."
        );
        return Ok(());
    }
    for fate in fates {
        let when = match &fate.observed_commit {
            Some(sha) => format!("{}, commit {}", date(fate.observed_at), short(sha)),
            None => format!("{}, an agent write", date(fate.observed_at)),
        };
        let _ = writeln!(out, "  {when}: {}", fate.state);
        if fate.state == "dead" {
            let _ = write!(
                out,
                "      its best match scored {:.3} ({})",
                fate.similarity_score, fate.deciding_layer
            );
            if let Some(candidate) = fate.matched_candidate_id {
                let place = store
                    .position(candidate)
                    .map_err(|error| format!("{error}"))?
                    .map(|(hash, number)| format!("now {}:{number}", paths.name(&hash)))
                    .unwrap_or_else(|| "since removed".to_owned());
                let _ = write!(out, ": marrow line {candidate}, {place}");
            }
            let _ = writeln!(out);
        } else {
            let _ = write!(
                out,
                "      {:.3} like the version before it",
                fate.similarity_score
            );
            if let Some(original) = fate.birth_similarity {
                let _ = write!(out, ", {original:.3} like the original");
            }
            let _ = writeln!(out, " ({})", fate.deciding_layer);
            if let (Some(hash), Some(number)) = (&fate.file_path_hash, fate.line_number) {
                let _ = writeln!(out, "      now at {}:{number}", paths.name(hash));
            }
        }
        if let Some(previous) = fate.previous_seen_at {
            let _ = writeln!(out, "      last seen as it was before: {}", date(previous));
        }
    }
    Ok(())
}

fn now(
    out: &mut String,
    line: &LineRecord,
    stored: &marrow_store::SnapshotLine,
    relative: &str,
    disk: OnDisk,
) {
    let _ = writeln!(out, "Now");
    let checkpoint = match &line.last_seen_commit {
        Some(sha) => format!("{}, commit {}", date(line.last_seen_at), short(sha)),
        None => date(line.last_seen_at),
    };
    let _ = writeln!(
        out,
        "  alive at {relative}:{} as marrow last recorded it, last seen {checkpoint}",
        stored.line_number
    );
    // The same line, at the same place, needs no note: the file's bytes can differ from the
    // version marrow recorded without anything happening to this line, as when git rewrites
    // line endings on checkout.
    if let OnDisk::Changed(state, shifted) = disk {
        let what = match (state, shifted) {
            (State::Verbatim, false) => return,
            (State::Verbatim, true) => "unchanged, though lines around it have moved",
            (State::Edited, _) => "edited",
            (State::Moved, _) => "moved",
            (State::Dead, _) => "gone",
        };
        let _ = writeln!(
            out,
            "  on disk since then: {what}. That isn't recorded until the next agent write or \
             commit."
        );
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn a_target_is_a_file_and_a_line() {
        assert_eq!(parse("src/main.rs:12").unwrap().1, 12);
        assert_eq!(parse(r"C:\repo\src\main.rs:3").unwrap().1, 3);
        assert!(parse("src/main.rs").is_err());
        assert!(parse("src/main.rs:0").is_err());
        assert!(parse(":4").is_err());
    }
}
