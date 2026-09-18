use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod capture;
mod reconcile;
mod record;

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
    /// Reconcile what marrow knows against HEAD, and checkpoint every line at that commit.
    Reconcile {
        /// Path to the local git repository. Defaults to the current directory.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Write .git/hooks/post-commit so every commit is reconciled, and do nothing else.
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
        Some(Command::Record {
            session,
            file,
            model,
            hook,
        }) => record(session, file, model, hook),
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
        }
    };
    let (Some(session), Some(file)) = (session.or(payload.session), file.or(payload.file)) else {
        eprintln!(
            "marrow record: needs --session and --file, or --hook with Claude Code's payload on stdin"
        );
        return ExitCode::from(2);
    };
    match record::capture(&session, &file, model.as_deref()) {
        Ok(record::Capture::Recorded { born, changed }) => {
            println!(
                "marrow record: {} {} born, {} {} changed",
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
