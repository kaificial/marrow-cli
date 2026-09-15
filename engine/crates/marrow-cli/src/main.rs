use clap::{Parser, Subcommand};

/// Marrow: tracks how long lines of code survive in a local git repo or just a project folder
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
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Version) | None => {
            println!("marrow {}", env!("CARGO_PKG_VERSION"));
        }
    }
}
