use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod audit;
mod backfill;
mod capture;
mod doctor;
mod explain;
mod init;
mod paths;
mod reconcile;
mod record;
mod report;
mod serve;
mod status;
mod survival;

use clap::{Parser, Subcommand};
use marrow_core::Trace;
use serde_json::{json, Value};

/// Marrow: tracks how long lines of code survive in a local git repo.
#[derive(Parser)]
#[command(name = "marrow", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print the engine version.
    Version,
    /// Start watching the repository in the current directory: create .marrow/, register the
    /// Claude Code write hook and the git hooks, and offer to record the history so far.
    Init {
        /// Record the existing history now, without asking.
        #[arg(long, conflicts_with = "no_backfill")]
        backfill: bool,
        /// Don't record the existing history, and don't ask.
        #[arg(long)]
        no_backfill: bool,
    },
    /// Record one agent write: diff the file against its last known state and store the fates.
    Record {
        /// Claude Code session that made the write.
        #[arg(long)]
        session: Option<String>,
        /// File the agent wrote.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Model that made the write, when the caller knows it.
        #[arg(long)]
        model: Option<String>,
        /// Take the session and file from Claude Code's hook payload on stdin.
        #[arg(long)]
        hook: bool,
    },
    /// Record the history of the repository in the current directory, from before marrow was
    /// watching. Resumes where it stopped if it was interrupted.
    Backfill {
        /// Only the last N commits, or commits made on or after a date (YYYY-MM-DD, UTC).
        #[arg(long)]
        since: Option<String>,
    },
    /// Show the full history of one line: where it was born, every change and why, and where
    /// it is now.
    Explain {
        /// The line, as <file>:<line>, like src/main.rs:12.
        target: String,
    },
    /// Survival curves for lines since their first commit, with confidence intervals.
    Report {
        /// What to compare.
        #[arg(long, value_enum, default_value = "origin")]
        by: report::By,
        /// The rewritten death: similarity to the first committed version below which a line
        /// counts as rewritten.
        #[arg(long, default_value_t = 0.5)]
        rewrite_threshold: f64,
        /// Bootstrap resamples for the confidence intervals; 0 leaves them out.
        #[arg(long, default_value_t = 200)]
        resamples: usize,
        /// Seed for the resampling, so a report can be reproduced exactly.
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Open the dashboard: the report, sessions, line histories and health checks in a browser,
    /// served on this machine only.
    Serve {
        /// Port on 127.0.0.1 to listen on; 0 picks a free one.
        #[arg(long, default_value_t = 4747)]
        port: u16,
    },
    /// What happened to the lines an agent session wrote, beside how lines here usually fare.
    Status {
        /// The session to show. Defaults to the most recent.
        #[arg(long)]
        session: Option<String>,
    },
    /// Check that marrow is watching this repository: its hooks, how much history it holds, and
    /// the mutation-corpus result for this build.
    Doctor,
    /// Print everything marrow stores about this repository, so anyone can check it holds no
    /// source code.
    Export {
        /// Dump every file in .marrow/ and every row of every table. Currently the only export.
        #[arg(long)]
        audit: bool,
    },
    /// Reconcile what marrow knows against HEAD, and checkpoint every line at that commit.
    Reconcile {
        /// Path to the local git repository. Defaults to the current directory.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Write the post-commit and post-merge hooks so every commit is reconciled, and do nothing
        /// else.
        #[arg(long)]
        install: bool,
    },
    /// Print what happened to every line in a repository's first-parent history.
    Trace {
        /// Path to the local git repository.
        #[arg(long)]
        repo: PathBuf,
        /// Print JSON (docs/specs/trace-cli.md). Currently the only output format.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Some(Command::Version) | None => {
            println!("marrow {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(Command::Trace { repo, json }) => trace(&repo, json),
        Some(Command::Init {
            backfill,
            no_backfill,
        }) => init_command(backfill, no_backfill),
        Some(Command::Record {
            session,
            file,
            model,
            hook,
        }) => record(session, file, model, hook),
        Some(Command::Backfill { since }) => backfill_command(since.as_deref()),
        Some(Command::Report {
            by,
            rewrite_threshold,
            resamples,
            seed,
        }) => {
            if !(rewrite_threshold > 0.0 && rewrite_threshold < 1.0) {
                eprintln!("marrow report: --rewrite-threshold must be between 0 and 1");
                return ExitCode::from(2);
            }
            print_or_fail(
                "report",
                report::report(&report::Options {
                    by,
                    threshold: rewrite_threshold,
                    resamples,
                    seed,
                }),
            )
        }
        Some(Command::Serve { port }) => match serve::serve(port) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("marrow serve: {message}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Status { session }) => {
            print_or_fail("status", status::status(session.as_deref()))
        }
        Some(Command::Explain { target }) => match explain::explain(&target) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("marrow explain: {message}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Doctor) => match doctor::doctor() {
            Ok(checkup) => {
                print!("{}", checkup.text);
                if checkup.healthy {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Err(message) => {
                eprintln!("marrow doctor: {message}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Export { audit }) => {
            if !audit {
                eprintln!("marrow export: only --audit is supported");
                return ExitCode::from(2);
            }
            print_or_fail("export", audit::export_audit())
        }
        Some(Command::Reconcile { repo, install }) => reconcile_command(repo.as_deref(), install),
    }
}

/// A write is never worth failing over: data problems warn and exit 0, so the agent's write
/// stands. Only a broken store or an unreadable payload exits non-zero.
fn record(
    session: Option<String>,
    file: Option<PathBuf>,
    model: Option<String>,
    hook: bool,
) -> ExitCode {
    let payload = if hook {
        match record::read_hook_payload() {
            Ok(payload) => payload,
            Err(message) => {
                eprintln!("marrow record: {message}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        record::HookPayload {
            session: None,
            file: None,
            tool: None,
        }
    };
    let (Some(session), Some(file)) = (session.or(payload.session), file.or(payload.file)) else {
        eprintln!(
            "marrow record: needs --session and --file, or --hook with Claude Code's payload on stdin"
        );
        return ExitCode::from(2);
    };
    match record::capture(&session, &file, model.as_deref(), payload.tool.as_deref()) {
        Ok(record::Capture::Recorded {
            born,
            changed,
            already_there,
        }) => {
            let already = if already_there > 0 {
                format!(" ({already_there} already in the file, not credited to the agent)")
            } else {
                String::new()
            };
            println!(
                "marrow record: {} {} born, {} {} changed{already}",
                born,
                if born == 1 { "line" } else { "lines" },
                changed,
                if changed == 1 { "line" } else { "lines" }
            );
            ExitCode::SUCCESS
        }
        Ok(record::Capture::Skipped(reason)) => {
            eprintln!("marrow record: skipped {} ({reason})", file.display());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("marrow record: {message}");
            ExitCode::FAILURE
        }
    }
}

fn init_command(backfill: bool, no_backfill: bool) -> ExitCode {
    let setup = match init::init() {
        Ok(setup) => setup,
        Err(message) => {
            eprintln!("marrow init: {message}");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", setup.text);
    let run_backfill = if backfill {
        true
    } else if no_backfill {
        false
    } else if std::io::stdin().is_terminal() {
        ask("Record this repository's history so far, labelled as written by people? [Y/n] ")
    } else {
        false
    };
    let backfilled = if run_backfill {
        backfill_command(None) == ExitCode::SUCCESS
    } else {
        println!(
            "Not recording the history so far. To record it, run marrow backfill before the next \
             commit or agent write; once capture has started, it can't be added \
             (docs/decisions/0009-backfill.md)."
        );
        true
    };
    if setup.complete && backfilled {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// A yes/no question on the terminal, yes unless the answer starts with n.
fn ask(question: &str) -> bool {
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    !answer.trim().to_lowercase().starts_with('n')
}

fn backfill_command(since: Option<&str>) -> ExitCode {
    let since = match since.map(backfill::Since::parse).transpose() {
        Ok(since) => since,
        Err(message) => {
            eprintln!("marrow backfill: {message}");
            return ExitCode::from(2);
        }
    };
    match backfill::backfill(since) {
        Ok(backfill::Outcome::Done(summary)) => {
            let counts = summary.counts;
            let through = backfill::short(&summary.through);
            if summary.commits == 0 {
                println!("marrow backfill: already recorded through {through}");
            } else {
                println!(
                    "marrow backfill: {} {} {} through {through}",
                    if summary.resumed {
                        "resumed and recorded"
                    } else {
                        "recorded"
                    },
                    summary.commits,
                    plural(summary.commits as i64, "commit", "commits")
                );
            }
            println!(
                "  in total: {} {} ({} {}), {} lines born, {} edits, {} moves, {} deaths, \
                 {} lines alive",
                counts.commits,
                plural(counts.commits, "commit", "commits"),
                counts.merges,
                plural(counts.merges, "merge", "merges"),
                counts.lines,
                counts.edited,
                counts.moved,
                counts.dead,
                counts.alive
            );
            if counts.trailer_commits > 0 {
                println!(
                    "  {} {} Claude Code's co-author trailer. Their lines stay labelled human; \
                     the trailer is stored with the commit.",
                    counts.trailer_commits,
                    plural(counts.trailer_commits, "commit carries", "commits carry")
                );
            }
            for (sha, path) in &summary.carried {
                eprintln!(
                    "marrow backfill: tree-sitter gave up on {path} at {}; kept its previous \
                     version's lines",
                    backfill::short(sha)
                );
            }
            for (sha, path) in &summary.skipped {
                eprintln!(
                    "marrow backfill: tree-sitter gave up on {path} at {}; not tracked until a \
                     version of it parses",
                    backfill::short(sha)
                );
            }
            ExitCode::SUCCESS
        }
        Ok(backfill::Outcome::Nothing(reason)) => {
            println!("marrow backfill: {reason}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("marrow backfill: {message}");
            ExitCode::FAILURE
        }
    }
}

fn print_or_fail(command: &str, result: Result<String, String>) -> ExitCode {
    match result {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("marrow {command}: {message}");
            ExitCode::FAILURE
        }
    }
}

fn plural(count: i64, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else {
        many
    }
}

/// Like `record`, a commit is never worth failing over: git ignores a post-commit hook's exit
/// code, and a reconciliation problem should be reported, not hidden.
fn reconcile_command(repo: Option<&Path>, install: bool) -> ExitCode {
    if install {
        return match reconcile::install(repo) {
            Ok(message) => {
                println!("marrow reconcile: {message}");
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("marrow reconcile: {message}");
                ExitCode::FAILURE
            }
        };
    }
    match reconcile::reconcile(repo) {
        Ok(reconcile::Outcome::Reconciled(report)) => {
            for path in &report.unparsed {
                eprintln!("marrow reconcile: left {path} alone (tree-sitter gave up parsing it)");
            }
            for path in &report.rebaselined {
                eprintln!(
                    "marrow reconcile: took {path} as it is (it changed without this commit \
                     changing it, so no fates were recorded for it)"
                );
            }
            println!(
                "marrow reconcile: {} — {} born, {} edited, {} moved, {} dead, {} alive",
                reconcile::short(&report.commit),
                report.born,
                report.edited,
                report.moved,
                report.dead,
                report.alive
            );
            ExitCode::SUCCESS
        }
        Ok(reconcile::Outcome::Skipped(reason)) => {
            eprintln!("marrow reconcile: nothing to do ({reason})");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("marrow reconcile: {message}");
            ExitCode::FAILURE
        }
    }
}

fn trace(repo: &Path, json: bool) -> ExitCode {
    if !json {
        eprintln!("marrow trace: only --json output is supported");
        return ExitCode::from(2);
    }
    let trace = match marrow_core::trace_repository(repo) {
        Ok(trace) => trace,
        Err(error) => {
            eprintln!("marrow trace: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut stdout = std::io::stdout().lock();
    let written = serde_json::to_writer(&mut stdout, &to_json(&trace))
        .map_err(|error| error.to_string())
        .and_then(|()| writeln!(stdout).map_err(|error| error.to_string()));
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("marrow trace: can't write output: {message}");
            ExitCode::FAILURE
        }
    }
}

fn to_json(trace: &Trace) -> Value {
    let lines: Vec<Value> = trace
        .lines
        .iter()
        .map(|line| {
            let fates: Vec<Value> = line
                .fates
                .iter()
                .map(|fate| {
                    let mut value = json!({
                        "commit": trace.commits[fate.commit],
                        "state": fate.state.as_str(),
                        "similarity_score": fate.similarity,
                        "deciding_layer": fate.layer.as_str(),
                    });
                    if let Some(position) = &fate.position {
                        value["path"] = json!(position.path);
                        value["line"] = json!(position.line);
                    }
                    value
                })
                .collect();
            json!({
                "birth": {
                    "commit": trace.commits[line.birth_commit],
                    "path": line.birth.path,
                    "line": line.birth.line,
                },
                "fates": fates,
            })
        })
        .collect();
    json!({
        "contract_version": 1,
        "engine": {"name": "marrow", "version": env!("CARGO_PKG_VERSION")},
        "commits": trace.commits,
        "lines": lines,
    })
}
