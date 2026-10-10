//! `marrow report`: survival curves, by origin, syntactic role, or file (PRD §7.1), for both of
//! PRD §6's definitions of death, with 95% bootstrap intervals and the threshold sweep PRD §9.3
//! asks for.

use std::collections::HashMap;
use std::fmt::Write;

use marrow_core::history::Repository;
use marrow_stats::{bootstrap, Estimate, Interval, Observation};
use marrow_store::Store;

use crate::capture::repository_root;
use crate::paths::Paths;
use crate::survival::{fit, follow_up, known_until, Death, Population};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum By {
    Origin,
    Role,
    File,
}

#[derive(Clone, Copy)]
pub struct Options {
    pub by: By,
    pub threshold: f64,
    pub resamples: usize,
    pub seed: u64,
}

/// The ages survival is reported at, in days.
const TIMES: [f64; 4] = [1.0, 7.0, 30.0, 90.0];
/// The rewrite thresholds the sensitivity sweep covers.
const SWEEP: [f64; 5] = [0.3, 0.4, 0.5, 0.6, 0.7];
/// Beyond this many groups, the smallest are pooled as "other".
const MAX_GROUPS: usize = 8;
const LEVEL: f64 = 0.95;
/// A curve is drawn only as far as this many lines are still known to be alive: past that, one
/// death more or less swings it too far to read anything into its shape.
const MIN_KNOWN_ALIVE: usize = 10;
const CELL: usize = 21;

const CONFOUND: &str = "Agent and human lines weren't written for the same tasks, so these \
                        curves compare the code that survived, not the skill of whoever wrote it.";

struct Group {
    name: String,
    symbol: char,
    lines: Vec<usize>,
}

/// Everything a report says, before it is printed or sent anywhere.
pub struct Report {
    options: Options,
    groups: Vec<Group>,
    /// One table per definition of death; empty when no line has reached a commit.
    tables: Vec<Table>,
    /// PRD §9.4: agent and human lines are being compared, so the confound is stated.
    confounded: bool,
    unconverged: bool,
    sweep: Option<Sweep>,
    left_out: LeftOut,
}

struct Table {
    death: Death,
    /// One per group, in the same order.
    rows: Vec<Row>,
}

struct Row {
    lines: usize,
    deaths: usize,
    /// The longest any line was followed.
    followed: f64,
    /// The last age any line was known to be alive; past it the estimate isn't the data's.
    known: f64,
    /// How far the curve can be drawn: while at least MIN_KNOWN_ALIVE lines are known alive.
    drawn: f64,
    estimate: Estimate,
    /// Intervals at each of TIMES, then the median's; empty when the report was asked for
    /// without resampling.
    bands: Vec<Interval>,
    /// Lines known alive at each of TIMES.
    known_alive: Vec<usize>,
}

/// PRD §9.3: survival at one age, across rewrite thresholds.
struct Sweep {
    /// The age the sweep is read at; None when no line was followed for a day yet.
    at: Option<f64>,
    /// Per group, per threshold; None past what the data cover.
    cells: Vec<Vec<Option<f64>>>,
}

struct LeftOut {
    already_there: usize,
    inconsistent: usize,
    agent_deleted_uncommitted: usize,
    agent_pending: usize,
    agent_written: usize,
}

pub fn report(options: &Options) -> Result<String, String> {
    compute(options).map(|report| render(&report))
}

pub fn compute(options: &Options) -> Result<Report, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("can't find the cwd: {error}"))?;
    let root =
        repository_root(&cwd.canonicalize().unwrap_or(cwd)).ok_or("not inside a git repository")?;
    let database = root.join(".marrow").join("db.sqlite");
    if !database.exists() {
        return Err(
            "marrow has no records for this repository yet; run marrow backfill".to_owned(),
        );
    }
    let store = Store::open(&database).map_err(|error| format!("{error}"))?;
    let population = Population::load(&store)?;
    let left_out = LeftOut {
        already_there: population.already_there,
        inconsistent: population.inconsistent,
        agent_deleted_uncommitted: population.agent_deleted_uncommitted,
        agent_pending: population.agent_pending,
        agent_written: population.agent_written,
    };
    if population.lines.is_empty() {
        return Ok(Report {
            options: *options,
            groups: Vec::new(),
            tables: Vec::new(),
            confounded: false,
            unconverged: false,
            sweep: None,
            left_out,
        });
    }
    let groups = groups(&population, options.by, &root)?;
    let mut tables = Vec::new();
    let mut unconverged = false;
    for (definition, death) in [Death::Deleted, Death::Rewritten(options.threshold)]
        .into_iter()
        .enumerate()
    {
        let mut rows = Vec::new();
        for (number, group) in groups.iter().enumerate() {
            // Every table gets its own stream, so adding a group doesn't change another's.
            let seed = options
                .seed
                .wrapping_add(((definition * 64 + number) as u64).wrapping_mul(0x9e37_79b9));
            let observations: Vec<Observation> = group
                .lines
                .iter()
                .map(|&index| population.lifetime(&population.lines[index], death))
                .collect();
            let estimate = fit(&observations);
            unconverged |= !estimate.converged;
            let followed = follow_up(&observations);
            let drawn = drawable(&observations, followed);
            let clusters: Vec<usize> = group
                .lines
                .iter()
                .map(|&index| population.clusters[index])
                .collect();
            let bands = if options.resamples > 0 {
                bootstrap(
                    &observations,
                    &clusters,
                    options.resamples,
                    LEVEL,
                    seed,
                    |estimate| {
                        let mut values: Vec<Option<f64>> =
                            TIMES.iter().map(|&t| Some(estimate.survival(t))).collect();
                        values.push(estimate.median());
                        values
                    },
                )
            } else {
                Vec::new()
            };
            rows.push(Row {
                lines: group.lines.len(),
                deaths: observations.iter().filter(|o| o.is_death()).count(),
                followed,
                known: known_until(&observations),
                drawn,
                estimate,
                bands,
                known_alive: TIMES
                    .iter()
                    .map(|&t| known_alive(&observations, t))
                    .collect(),
            });
        }
        tables.push(Table { death, rows });
    }
    let compared = |name: &str| groups.iter().any(|group| group.name == name);
    let confounded = options.by == By::Origin && compared("agent") && compared("human");
    let sweep = sweep(&population, &groups);
    Ok(Report {
        options: *options,
        groups,
        tables,
        confounded,
        unconverged,
        sweep: Some(sweep),
        left_out,
    })
}

/// The report as the terminal shows it.
pub fn render(report: &Report) -> String {
    let options = &report.options;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Survival of lines since their first commit, by {}",
        match options.by {
            By::Origin => "origin",
            By::Role => "syntactic role",
            By::File => "the file they were born in",
        }
    );
    let _ = writeln!(
        out,
        "Turnbull's estimator (Turnbull 1976), with {}% intervals from {} bootstrap resamples \
         (Efron 1981) of code written together, seed {}.",
        (LEVEL * 100.0) as u32,
        options.resamples,
        options.seed
    );
    if report.tables.is_empty() {
        let _ = writeln!(
            out,
            "\nNothing to report yet: no line has reached a commit since marrow started watching."
        );
        left_out(&mut out, &report.left_out);
        return out;
    }
    let mut deletion: Vec<(char, Estimate, f64)> = Vec::new();
    let mut shortened = false;
    for table in &report.tables {
        let _ = writeln!(out, "\n{}", death_label(table.death));
        let _ = writeln!(
            out,
            "  {:<24} {:>7} {:>7} {:>10}  {:<CELL$}{:<CELL$}{:<CELL$}{:<CELL$}median",
            "", "lines", "deaths", "followed", "1 day", "7 days", "30 days", "90 days"
        );
        for (group, row) in report.groups.iter().zip(&table.rows) {
            let _ = write!(
                out,
                "  {:<24} {:>7} {:>7} {:>10}  ",
                truncate(&group.name, 24),
                row.lines,
                row.deaths,
                days(row.followed)
            );
            for (index, &t) in TIMES.iter().enumerate() {
                let cell = if t > row.known {
                    "-".to_owned()
                } else {
                    survival_cell(row.estimate.survival(t), row.bands.get(index))
                };
                let _ = write!(out, "{cell:<CELL$}");
            }
            let _ = writeln!(
                out,
                "{}",
                median_cell(row.estimate.median(), row.bands.get(TIMES.len()))
            );
            // How many lines each number rests on: those known to be alive at that age.
            let _ = write!(out, "  {:>53}", "known alive at that age:  ");
            for (index, &t) in TIMES.iter().enumerate() {
                let cell = if t > row.known {
                    String::new()
                } else {
                    format!("{} lines", row.known_alive[index])
                };
                let _ = write!(out, "{cell:<CELL$}");
            }
            let _ = writeln!(out);
            if table.death == Death::Deleted {
                // The curve is drawn only as far as enough lines are still followed.
                shortened |= row.drawn < row.followed;
                deletion.push((group.symbol, row.estimate.clone(), row.drawn));
            }
        }
    }
    if report.confounded {
        let _ = writeln!(out, "\n{CONFOUND}");
    }
    if report.unconverged {
        let _ = writeln!(
            out,
            "\nWarning: at least one fit stopped at its iteration limit before converging, so \
             its numbers may be slightly off."
        );
    }
    if let Some(sweep) = &report.sweep {
        render_sweep(&mut out, sweep, &report.groups);
    }
    plot(&mut out, &report.groups, &deletion, shortened);
    left_out(&mut out, &report.left_out);
    out
}

fn death_label(death: Death) -> String {
    match death {
        Death::Deleted => "Death: deleted".to_owned(),
        Death::Rewritten(threshold) => format!(
            "Death: deleted, or rewritten until less than {threshold:.2} similar to the version \
             first committed"
        ),
    }
}

fn groups(population: &Population, by: By, root: &std::path::Path) -> Result<Vec<Group>, String> {
    let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, line) in population.lines.iter().enumerate() {
        let key = match by {
            // As it stood at the line's first commit, not as later relabelling left it.
            By::Origin => population.origins[index].to_owned(),
            By::Role => line.syntactic_role.clone(),
            By::File => line.file_path_hash.clone(),
        };
        by_key.entry(key).or_default().push(index);
    }
    let mut keyed: Vec<(String, Vec<usize>)> = by_key.into_iter().collect();
    if by == By::Origin {
        let order = |key: &str| match key {
            "agent" => 0,
            "agent_then_human_revised" => 1,
            _ => 2,
        };
        keyed.sort_by_key(|(key, _)| order(key));
    } else {
        keyed.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
        if keyed.len() > MAX_GROUPS {
            let rest: Vec<usize> = keyed
                .drain(MAX_GROUPS - 1..)
                .flat_map(|(_, lines)| lines)
                .collect();
            keyed.push(("\u{0}other".to_owned(), rest));
        }
    }
    let paths = if by == By::File {
        let repository = Repository::open(root).map_err(|error| format!("{error}"))?;
        Some(Paths::new(root, &repository))
    } else {
        None
    };
    Ok(keyed
        .into_iter()
        .enumerate()
        .map(|(index, (key, lines))| {
            let (name, symbol) = match (by, key.as_str()) {
                (_, "\u{0}other") => ("other".to_owned(), '+'),
                (By::Origin, "agent") => ("agent".to_owned(), 'A'),
                (By::Origin, "agent_then_human_revised") => {
                    ("agent, revised by a person".to_owned(), 'R')
                }
                (By::Origin, "human") => ("human".to_owned(), 'H'),
                (By::File, hash) => (
                    paths
                        .as_ref()
                        .map_or(hash.to_owned(), |paths| paths.name(hash)),
                    symbol(index),
                ),
                (_, other) => (
                    if other.is_empty() {
                        "(no role)".to_owned()
                    } else {
                        other.to_owned()
                    },
                    symbol(index),
                ),
            };
            Group {
                name,
                symbol,
                lines,
            }
        })
        .collect())
}

fn symbol(index: usize) -> char {
    char::from_digit((index as u32 + 1) % 10, 10).unwrap_or('?')
}

/// PRD §9.3: where a result depends on a threshold, show how much.
fn sweep(population: &Population, groups: &[Group]) -> Sweep {
    let all: Vec<Observation> = population
        .lines
        .iter()
        .map(|line| population.lifetime(line, Death::Deleted))
        .collect();
    let known = known_until(&all);
    let Some(&at) = TIMES.iter().rev().find(|&&t| t <= known) else {
        return Sweep {
            at: None,
            cells: Vec::new(),
        };
    };
    let cells = groups
        .iter()
        .map(|group| {
            SWEEP
                .iter()
                .map(|&threshold| {
                    let observations: Vec<Observation> = group
                        .lines
                        .iter()
                        .map(|&index| {
                            population
                                .lifetime(&population.lines[index], Death::Rewritten(threshold))
                        })
                        .collect();
                    (at <= known_until(&observations)).then(|| fit(&observations).survival(at))
                })
                .collect()
        })
        .collect();
    Sweep {
        at: Some(at),
        cells,
    }
}

fn render_sweep(out: &mut String, sweep: &Sweep, groups: &[Group]) {
    let Some(at) = sweep.at else {
        let _ = writeln!(
            out,
            "\nThe rewrite threshold's effect needs at least a day of follow-up; there isn't \
             that much yet."
        );
        return;
    };
    let _ = writeln!(
        out,
        "\nHow the rewritten death depends on its threshold: survival at {} {}",
        at,
        if at == 1.0 { "day" } else { "days" }
    );
    let _ = write!(out, "  {:<24}", "similarity below");
    for threshold in SWEEP {
        let _ = write!(out, "{threshold:>8.2}");
    }
    let _ = writeln!(out);
    for (group, cells) in groups.iter().zip(&sweep.cells) {
        let _ = write!(out, "  {:<24}", truncate(&group.name, 24));
        for cell in cells {
            let cell = cell.map_or_else(|| "-".to_owned(), |survival| format!("{survival:.3}"));
            let _ = write!(out, "{cell:>8}");
        }
        let _ = writeln!(out);
    }
}

/// The deletion curves, drawn in the terminal.
fn plot(out: &mut String, groups: &[Group], curves: &[(char, Estimate, f64)], shortened: bool) {
    const WIDTH: usize = 60;
    const HEIGHT: usize = 12;
    let horizon = curves
        .iter()
        .map(|(_, _, horizon)| *horizon)
        .fold(0.0, f64::max);
    if horizon <= 0.0 {
        let _ = writeln!(
            out,
            "
No curve is drawn yet: none has {MIN_KNOWN_ALIVE} lines followed long enough to show a shape."
        );
        return;
    }
    let mut grid = vec![vec![' '; WIDTH]; HEIGHT];
    for (symbol, estimate, own) in curves {
        // Each curve stops where its own follow-up does: past it, the data say nothing.
        let rows = (0..WIDTH)
            .map(|column| horizon * column as f64 / (WIDTH - 1) as f64)
            .take_while(|t| t <= own)
            .map(|t| ((1.0 - estimate.survival(t)) * (HEIGHT - 1) as f64).round() as usize);
        for (column, row) in rows.enumerate() {
            let cell = &mut grid[row.min(HEIGHT - 1)][column];
            *cell = if *cell == ' ' || cell == symbol {
                *symbol
            } else {
                '*'
            };
        }
    }
    let _ = writeln!(
        out,
        "\nDeletion curves (survival against days since first commit)"
    );
    for (row, cells) in grid.iter().enumerate() {
        let label = match row {
            0 => "1.0",
            _ if row == HEIGHT / 2 => "0.5",
            _ if row == HEIGHT - 1 => "0.0",
            _ => "",
        };
        let _ = writeln!(out, "  {label:>4} |{}", cells.iter().collect::<String>());
    }
    let _ = writeln!(out, "       +{}", "-".repeat(WIDTH));
    let end = days(horizon);
    let _ = writeln!(out, "        0{:>width$}", end, width = WIDTH - 1);
    let legend: Vec<String> = groups
        .iter()
        .map(|group| format!("{} {}", group.symbol, group.name))
        .collect();
    let _ = writeln!(
        out,
        "        {}   (* where curves overlap)",
        legend.join(", ")
    );
    if shortened {
        let _ = writeln!(
            out,
            "        Each curve stops where fewer than {MIN_KNOWN_ALIVE} of its lines are still \
             known to be alive."
        );
    }
}

/// Lines known to be alive at age `t`: followed at least that long, and not dead before it.
fn known_alive(observations: &[Observation], t: f64) -> usize {
    observations
        .iter()
        .filter(|observation| observation.left >= t)
        .count()
}

/// How far a curve can be drawn: the last age at which at least MIN_KNOWN_ALIVE of its lines are
/// still known to be alive.
fn drawable(observations: &[Observation], horizon: f64) -> f64 {
    let mut lefts: Vec<f64> = observations.iter().map(|o| o.left).collect();
    lefts.sort_by(|a, b| b.total_cmp(a));
    match lefts.get(MIN_KNOWN_ALIVE - 1) {
        Some(&age) => age.min(horizon),
        None => 0.0,
    }
}

fn left_out(out: &mut String, left: &LeftOut) {
    let mut notes = Vec::new();
    if left.already_there > 0 {
        notes.push(format!(
            "{} {} already there when marrow started watching, so {} age is unknown",
            left.already_there,
            if left.already_there == 1 {
                "line was"
            } else {
                "lines were"
            },
            if left.already_there == 1 {
                "its"
            } else {
                "their"
            }
        ));
    }
    if left.inconsistent > 0 {
        notes.push(format!(
            "{} {} recorded times that run backwards by more than an hour, which only wrong \
             dates in the history can cause",
            left.inconsistent,
            if left.inconsistent == 1 {
                "line has"
            } else {
                "lines have"
            }
        ));
    }
    let uncommitted = left.agent_deleted_uncommitted + left.agent_pending;
    if uncommitted > 0 {
        notes.push(format!(
            "{uncommitted} of the {} lines agents wrote haven't reached a commit: {} were \
             deleted before one did, and {} are still uncommitted",
            left.agent_written, left.agent_deleted_uncommitted, left.agent_pending
        ));
    }
    if notes.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "
Left out of the curves:"
    );
    for note in notes {
        let _ = writeln!(out, "  {note}");
    }
}

fn survival_cell(point: f64, band: Option<&Interval>) -> String {
    match band {
        Some(Interval {
            low: Some(low),
            high: Some(high),
        }) => format!("{point:.3} ({low:.3}-{high:.3})"),
        _ => format!("{point:.3}"),
    }
}

/// The median, and its interval. A median the curve never falls far enough to reach is "not
/// reached", and so is an interval end that lies beyond the data.
fn median_cell(median: Option<f64>, band: Option<&Interval>) -> String {
    let point = median.map_or_else(|| "not reached".to_owned(), days);
    match band {
        Some(Interval {
            low: Some(low),
            high: Some(high),
        }) => format!("{point} ({}-{})", days(*low), days(*high)),
        Some(Interval {
            low: Some(low),
            high: None,
        }) => format!("{point} (at least {})", days(*low)),
        _ => point,
    }
}

fn days(days: f64) -> String {
    if days < 10.0 {
        format!("{days:.1} days")
    } else {
        format!("{days:.0} days")
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let tail: String = text
        .chars()
        .rev()
        .take(width - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}
