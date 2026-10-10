//! Turning what the store recorded into lifetimes, the input to the survival estimator
//! (docs/decisions/0010-survival.md).
//!
//! A line's clock starts at its first commit, for agent and human code alike: the hook sees an
//! agent's lines the moment they're written, but a person's only when they're committed, so
//! starting any earlier would compare the two unfairly. Only lines that reached a commit are in
//! the curves, and only what happened to them after that commit counts.

use std::collections::HashMap;

use marrow_stats::{turnbull, Estimate, Observation};
use marrow_store::{FateRecord, LineRecord, Store};

pub const DAY: f64 = 86_400.0;

/// How far a line's recorded times may contradict their order before the line is left out
/// rather than repaired. Clocks disagree by seconds all the time; an hour means something is
/// wrong with the dates themselves.
const TOLERANCE: i64 = 3_600;

/// When a line counts as dead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Death {
    /// PRD §6 (a): deleted.
    Deleted,
    /// PRD §6 (b): deleted, or edited until it is less than this similar to the version it was
    /// first committed as. A deleted line keeps nothing of its original, so it counts too.
    Rewritten(f64),
}

/// The lines in the curves, and what was left out and why.
pub struct Population {
    pub lines: Vec<LineRecord>,
    /// Each line's origin as it stood at its first commit. A line relabelled later, when a
    /// person revised it, stays in the group it entered in: a line can only be revised if it
    /// lived long enough, so regrouping it would let the future pick its group.
    pub origins: Vec<&'static str>,
    /// Each line's cluster: the lines born in the same commit and file, which tend to live and
    /// die together, and which the bootstrap resamples as a unit.
    pub clusters: Vec<usize>,
    fates: HashMap<i64, Vec<FateRecord>>,
    /// Lines already there when marrow started watching. Their real birth is earlier and
    /// unknown, so their age is too, and they can't be placed on the curves.
    pub already_there: usize,
    /// Lines whose recorded times contradict their own order by more than an hour, which only
    /// wrong dates in the history can cause. Repairing them would be a guess.
    pub inconsistent: usize,
    /// Agent-written lines that haven't reached a commit: deleted before one did, or still
    /// waiting for one.
    pub agent_deleted_uncommitted: usize,
    pub agent_pending: usize,
    pub agent_written: usize,
}

impl Population {
    pub fn load(store: &Store) -> Result<Population, String> {
        let mut fates: HashMap<i64, Vec<FateRecord>> = HashMap::new();
        for fate in store.fates().map_err(|error| format!("{error}"))? {
            fates.entry(fate.line_id).or_default().push(fate);
        }
        let mut population = Population {
            lines: Vec::new(),
            origins: Vec::new(),
            clusters: Vec::new(),
            fates: HashMap::new(),
            already_there: 0,
            inconsistent: 0,
            agent_deleted_uncommitted: 0,
            agent_pending: 0,
            agent_written: 0,
        };
        let mut cluster_of: HashMap<(String, String), usize> = HashMap::new();
        for line in store.lines().map_err(|error| format!("{error}"))? {
            let by_agent = line.session_id.is_some();
            if by_agent {
                population.agent_written += 1;
            }
            let history = fates.remove(&line.line_id).unwrap_or_default();
            let Some(birth_commit) = line.birth_commit.clone() else {
                if by_agent {
                    if history.iter().any(|fate| fate.state == "dead") {
                        population.agent_deleted_uncommitted += 1;
                    } else {
                        population.agent_pending += 1;
                    }
                }
                continue;
            };
            if !line.birth_observed {
                population.already_there += 1;
                continue;
            }
            if contradicts_itself(&line, &history) {
                population.inconsistent += 1;
                continue;
            }
            population.origins.push(origin_at_entry(&line, &history));
            let next = cluster_of.len();
            population.clusters.push(
                *cluster_of
                    .entry((birth_commit, line.file_path_hash.clone()))
                    .or_insert(next),
            );
            population.fates.insert(line.line_id, history);
            population.lines.push(line);
        }
        Ok(population)
    }

    /// Each line's lifetime under one definition of death, in days since its first commit.
    pub fn lifetime(&self, line: &LineRecord, death: Death) -> Observation {
        let entered = entered_at(line);
        let days = |at: i64| (at.max(entered) - entered) as f64 / DAY;
        let history = self.fates.get(&line.line_id).map_or(&[][..], Vec::as_slice);
        for fate in after_entry(line, history) {
            let dies = match death {
                Death::Deleted => fate.state == "dead",
                Death::Rewritten(threshold) => {
                    fate.state == "dead"
                        || fate
                            .birth_similarity
                            .is_some_and(|similarity| similarity < threshold)
                }
            };
            if dies {
                let last_alive = fate.previous_seen_at.unwrap_or(entered);
                return Observation::died(days(last_alive), days(fate.observed_at));
            }
        }
        Observation::alive(days(line.last_seen_at))
    }
}

/// What happened to a line after its clock started. What happened before or in its first
/// commit doesn't count: the agent's edits before committing, or a person's changes to the
/// agent's version on the way into that commit.
fn after_entry<'a>(
    line: &'a LineRecord,
    fates: &'a [FateRecord],
) -> impl Iterator<Item = &'a FateRecord> {
    fates.iter().filter(move |fate| {
        let before = match &fate.observed_commit {
            Some(commit) => Some(commit) == line.birth_commit.as_ref(),
            // A write the hook caught says whether the line had reached a commit yet, which
            // decides it by the order things happened in, not by clocks that tick in seconds.
            None => fate.before_first_commit,
        };
        !before
    })
}

/// A line's origin when its clock started. An agent-written line a person changed on its way
/// into its first commit entered revised; one revised later entered as the agent's.
pub fn origin_at_entry(line: &LineRecord, fates: &[FateRecord]) -> &'static str {
    if line.session_id.is_none() {
        return "human";
    }
    let revised_on_entry = fates.iter().any(|fate| {
        fate.state == "edited"
            && fate.observed_commit.is_some()
            && fate.observed_commit == line.birth_commit
    });
    if revised_on_entry {
        "agent_then_human_revised"
    } else {
        "agent"
    }
}

/// Whether a line's times run backwards by more than clocks disagree: a change dated long before
/// the line's first commit, or a last sighting long before it.
fn contradicts_itself(line: &LineRecord, fates: &[FateRecord]) -> bool {
    let entered = entered_at(line);
    let early = |at: i64| at < entered - TOLERANCE;
    after_entry(line, fates).any(|fate| early(fate.observed_at))
        || (!fates.iter().any(|fate| fate.state == "dead") && early(line.last_seen_at))
}

/// When a line's clock starts: its first commit. A line the hook caught earlier than that has an
/// earlier birth time, and the backfill's clock can run a little ahead of a commit's own date.
pub fn entered_at(line: &LineRecord) -> i64 {
    line.entered_at
        .map_or(line.birth_ts, |at| at.max(line.birth_ts))
}

/// Identical lifetimes fitted once, weighted by how many there are.
pub fn grouped(observations: &[Observation]) -> Vec<(Observation, f64)> {
    let mut sorted = observations.to_vec();
    sorted.sort_by(|a, b| a.left.total_cmp(&b.left).then(a.right.total_cmp(&b.right)));
    let mut grouped: Vec<(Observation, f64)> = Vec::new();
    for observation in sorted {
        match grouped.last_mut() {
            Some((last, count)) if *last == observation => *count += 1.0,
            _ => grouped.push((observation, 1.0)),
        }
    }
    grouped
}

pub fn fit(observations: &[Observation]) -> Estimate {
    turnbull(&grouped(observations))
}

/// The longest any line in the group was followed, to its death or its last sighting.
pub fn follow_up(observations: &[Observation]) -> f64 {
    observations
        .iter()
        .map(|observation| {
            if observation.right.is_finite() {
                observation.right
            } else {
                observation.left
            }
        })
        .fold(0.0, f64::max)
}

/// The last age at which some line was known to be alive. Past it, what the estimator says
/// isn't determined by the data, only by how it spreads deaths across their intervals.
pub fn known_until(observations: &[Observation]) -> f64 {
    observations
        .iter()
        .map(|observation| observation.left)
        .fold(0.0, f64::max)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use marrow_store::{FateRecord, LineRecord};

    use super::{contradicts_itself, origin_at_entry, Death, Population, DAY};

    const COMMIT: i64 = 1_000_000;

    fn line(line_id: i64, birth_ts: i64) -> LineRecord {
        LineRecord {
            line_id,
            file_path_hash: "f".to_owned(),
            birth_commit: Some("c1".to_owned()),
            birth_ts,
            birth_line_number: 1,
            birth_observed: true,
            origin: "agent".to_owned(),
            session_id: Some("s".to_owned()),
            model: None,
            syntactic_role: String::new(),
            last_seen_at: COMMIT + 10 * DAY as i64,
            last_seen_commit: None,
            entered_at: Some(COMMIT),
        }
    }

    fn fate(
        line_id: i64,
        at: i64,
        commit: Option<&str>,
        state: &str,
        original: Option<f64>,
    ) -> FateRecord {
        FateRecord {
            line_id,
            observed_at: at,
            observed_commit: commit.map(str::to_owned),
            previous_seen_at: Some(at - DAY as i64),
            state: state.to_owned(),
            similarity_score: 0.8,
            deciding_layer: "within_hunk_alignment".to_owned(),
            matched_candidate_id: None,
            line_number: None,
            file_path_hash: None,
            birth_similarity: original,
            before_first_commit: false,
        }
    }

    fn population(lines: Vec<LineRecord>, fates: Vec<FateRecord>) -> Population {
        let mut by_line: HashMap<i64, Vec<FateRecord>> = HashMap::new();
        for fate in fates {
            by_line.entry(fate.line_id).or_default().push(fate);
        }
        Population {
            origins: vec!["agent"; lines.len()],
            clusters: vec![0; lines.len()],
            lines,
            fates: by_line,
            already_there: 0,
            inconsistent: 0,
            agent_deleted_uncommitted: 0,
            agent_pending: 0,
            agent_written: 0,
        }
    }

    #[test]
    fn the_clock_starts_at_the_first_commit_not_the_write() {
        let written = COMMIT - DAY as i64;
        let deleted = COMMIT + 3 * DAY as i64;
        let people = population(
            vec![line(1, written)],
            vec![fate(1, deleted, Some("c4"), "dead", None)],
        );
        let lifetime = people.lifetime(&people.lines[0], Death::Deleted);
        assert!((lifetime.right - 3.0).abs() < 1e-9, "{lifetime:?}");
        assert!(
            (lifetime.left - 2.0).abs() < 1e-9,
            "last seen alive a day before"
        );
    }

    #[test]
    fn what_happened_before_or_in_the_first_commit_doesnt_count() {
        let mut before = fate(1, COMMIT - 3_600, None, "edited", Some(0.1));
        before.before_first_commit = true;
        // An agent write in the very second of the commit, but before it: the flag, not the
        // clock, says which side of the commit it fell on.
        let mut same_second = fate(1, COMMIT, None, "edited", Some(0.1));
        same_second.before_first_commit = true;
        let people = population(
            vec![line(1, COMMIT - DAY as i64)],
            vec![
                before,
                same_second,
                fate(1, COMMIT, Some("c1"), "edited", Some(0.1)),
            ],
        );
        let lifetime = people.lifetime(&people.lines[0], Death::Rewritten(0.5));
        assert!(!lifetime.is_death(), "{lifetime:?}");
        assert!((lifetime.left - 10.0).abs() < 1e-9, "alive until last seen");
    }

    #[test]
    fn a_write_after_the_commit_counts_even_if_a_clock_says_otherwise() {
        // The machine's clock says a few seconds before the commit; the order says after.
        let people = population(
            vec![line(1, COMMIT)],
            vec![fate(1, COMMIT - 5, None, "dead", None)],
        );
        assert!(people.lifetime(&people.lines[0], Death::Deleted).is_death());
    }

    #[test]
    fn a_rewrite_is_a_death_only_under_the_rewritten_definition() {
        let edited = COMMIT + 5 * DAY as i64;
        let people = population(
            vec![line(1, COMMIT)],
            vec![fate(1, edited, Some("c3"), "edited", Some(0.3))],
        );
        let line = &people.lines[0];
        assert!(!people.lifetime(line, Death::Deleted).is_death());
        assert!(people.lifetime(line, Death::Rewritten(0.5)).is_death());
        assert!(!people.lifetime(line, Death::Rewritten(0.2)).is_death());
    }

    #[test]
    fn a_deletion_counts_under_both_definitions() {
        let people = population(
            vec![line(1, COMMIT)],
            vec![fate(1, COMMIT + DAY as i64, Some("c2"), "dead", None)],
        );
        let line = &people.lines[0];
        assert!(people.lifetime(line, Death::Deleted).is_death());
        assert!(people.lifetime(line, Death::Rewritten(0.5)).is_death());
    }

    #[test]
    fn a_line_keeps_the_origin_it_entered_with() {
        let agent = line(1, COMMIT);
        let later = [fate(
            1,
            COMMIT + DAY as i64,
            Some("c5"),
            "edited",
            Some(0.9),
        )];
        assert_eq!(
            origin_at_entry(&agent, &later),
            "agent",
            "revised later, not on entry"
        );
        let on_entry = [fate(1, COMMIT, Some("c1"), "edited", Some(0.9))];
        assert_eq!(
            origin_at_entry(&agent, &on_entry),
            "agent_then_human_revised"
        );
        let mut person = line(2, COMMIT);
        person.session_id = None;
        assert_eq!(origin_at_entry(&person, &on_entry), "human");
    }

    #[test]
    fn times_running_backwards_by_hours_are_left_out_not_repaired() {
        let mut early = line(1, COMMIT);
        early.last_seen_at = COMMIT - 2 * 3_600;
        assert!(contradicts_itself(&early, &[]));
        let mut close = line(2, COMMIT);
        close.last_seen_at = COMMIT - 30;
        assert!(
            !contradicts_itself(&close, &[]),
            "seconds of skew are repaired"
        );
    }
}
