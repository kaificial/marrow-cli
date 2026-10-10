//! `marrow status`: what happened to the lines one agent session wrote, beside a baseline of how
//! lines in this repository usually fare (PRD §7.1). A predicted survival for this session is a v2
//! feature; v1 shows the cohort instead.

use std::collections::HashSet;
use std::fmt::Write;

use marrow_store::Store;

use crate::capture::{date, now, repository_root};
use crate::survival::{fit, known_until, Death, Population};

pub fn status(session: Option<&str>) -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("can't find the cwd: {error}"))?;
    let root =
        repository_root(&cwd.canonicalize().unwrap_or(cwd)).ok_or("not inside a git repository")?;
    let database = root.join(".marrow").join("db.sqlite");
    if !database.exists() {
        return Err("marrow has no records for this repository yet".to_owned());
    }
    let store = Store::open(&database).map_err(|error| format!("{error}"))?;
    let sessions = store.sessions().map_err(|error| format!("{error}"))?;
    let chosen =
        match session {
            Some(id) => sessions
                .iter()
                .find(|candidate| candidate.session_id == id)
                .ok_or_else(|| format!("no agent session called {id}"))?,
            // Not an error: a fresh install, or a repository only backfilled, has none yet.
            None => match sessions.first() {
                Some(session) => session,
                None => return Ok(
                    "No agent sessions recorded yet. The write hook records one the first time \
                     Claude Code edits a file here.\n"
                        .to_owned(),
                ),
            },
        };

    let lines: Vec<_> = store
        .lines()
        .map_err(|error| format!("{error}"))?
        .into_iter()
        .filter(|line| line.session_id.as_deref() == Some(chosen.session_id.as_str()))
        .collect();
    let ids: HashSet<i64> = lines.iter().map(|line| line.line_id).collect();
    let alive = store.alive().map_err(|error| format!("{error}"))?;
    let mut edited = HashSet::new();
    let mut deleted = HashSet::new();
    for fate in store.fates().map_err(|error| format!("{error}"))? {
        if !ids.contains(&fate.line_id) {
            continue;
        }
        match fate.state.as_str() {
            "edited" => {
                edited.insert(fate.line_id);
            }
            "dead" => {
                deleted.insert(fate.line_id);
            }
            _ => {}
        }
    }
    let written = lines.len();
    let there = lines
        .iter()
        .filter(|line| alive.contains(&line.line_id))
        .count();
    let committed = lines
        .iter()
        .filter(|line| line.birth_commit.is_some())
        .count();
    let revised = lines
        .iter()
        .filter(|line| line.origin == "agent_then_human_revised")
        .count();

    let mut out = String::new();
    let model = chosen
        .model
        .as_deref()
        .map(|model| format!(" ({model})"))
        .unwrap_or_default();
    let _ = writeln!(out, "Session {}{model}", chosen.session_id);
    let _ = writeln!(
        out,
        "  started {}, {} ago",
        date(chosen.started_at),
        ago(now() - chosen.started_at)
    );
    let _ = writeln!(out);
    let share = |count: usize| {
        if written == 0 {
            String::new()
        } else {
            format!(" ({:.0}%)", 100.0 * count as f64 / written as f64)
        }
    };
    let _ = writeln!(out, "  written                {written:>6} lines");
    let _ = writeln!(out, "  still there            {there:>6}{}", share(there));
    let _ = writeln!(
        out,
        "  reached a commit       {committed:>6}{}",
        share(committed)
    );
    let _ = writeln!(
        out,
        "  edited since           {:>6}{}",
        edited.len(),
        share(edited.len())
    );
    let _ = writeln!(
        out,
        "  revised by a person    {revised:>6}{}",
        share(revised)
    );
    let _ = writeln!(
        out,
        "  deleted                {:>6}{}",
        deleted.len(),
        share(deleted.len())
    );

    let population = Population::load(&store)?;
    let _ = writeln!(
        out,
        "\nFor comparison, how lines in this repository fare after their first commit (deletion):"
    );
    let mut any = false;
    for (origin, label) in [("agent", "agent lines"), ("human", "human lines")] {
        // Grouped by origin as it stood at each line's first commit, as `report` does.
        let observations: Vec<_> = population
            .lines
            .iter()
            .zip(&population.origins)
            .filter(|(_, entered_as)| **entered_as == origin)
            .map(|(line, _)| population.lifetime(line, Death::Deleted))
            .collect();
        if observations.is_empty() {
            continue;
        }
        let estimate = fit(&observations);
        let known = known_until(&observations);
        let spans: Vec<String> = [(1.0, "a day"), (7.0, "a week"), (30.0, "a month")]
            .into_iter()
            .filter(|(t, _)| *t <= known)
            .map(|(t, span)| format!("{:.0}% survive {span}", 100.0 * estimate.survival(t)))
            .collect();
        let text = if spans.is_empty() {
            "not followed for a day yet".to_owned()
        } else {
            spans.join(", ")
        };
        let _ = writeln!(out, "  {label:<12} {text}  ({} lines)", observations.len());
        any = true;
    }
    if !any {
        let _ = writeln!(out, "  no lines have reached a commit yet");
    }
    Ok(out)
}

fn ago(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..=89 => "moments".to_owned(),
        90..=5_399 => format!("{} minutes", (seconds + 30) / 60),
        5_400..=129_599 => format!("{} hours", (seconds + 1_800) / 3_600),
        _ => format!("{} days", (seconds + 43_200) / 86_400),
    }
}
