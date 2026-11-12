//! `keep-mcp` — lets AI assistants use the MoneyKeep books you allow, over the
//! Model Context Protocol on stdin/stdout (mcp-server.md revision 3).
//!
//! Nothing but MCP messages is written to stdout while serving; errors go to
//! stderr. There is no password option: `keep-mcp` ignores `MONEYKEEP_PASSWORD`
//! and never prompts. Password-protected books are used only through the app.

// `keep-mcp config` prints its snippet to stdout.
#![allow(clippy::print_stdout)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use keep_mcp::{Router, Target, config_json};
use mk_commands::Registry;

#[derive(Parser)]
#[command(
    name = "keep-mcp",
    version,
    about = "keep-mcp — lets AI assistants use the MoneyKeep books you allow (MCP over stdio)",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
    /// The book file to serve.
    book: Option<PathBuf>,
    /// Serve whichever book the running MoneyKeep app has open (through the app).
    #[arg(long, conflicts_with = "book")]
    app: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the JSON to paste into an MCP client's configuration, and exit.
    Config {
        /// The client: claude-desktop, claude-code or generic.
        #[arg(long, default_value = "generic")]
        client: String,
        /// The book file.
        book: Option<PathBuf>,
        /// Use whichever book the running app has open.
        #[arg(long, conflicts_with = "book")]
        app: bool,
    },
}

fn target(book: Option<PathBuf>, app: bool) -> Result<Target, String> {
    match (book, app) {
        (_, true) => Ok(Target::App),
        (Some(b), false) => Ok(Target::Book(b)),
        (None, false) => Err("give a book file or --app (see keep-mcp --help)".into()),
    }
}

/// Where the app's `control.json` is looked for. `KEEP_MCP_CONTROL_DIR` points
/// elsewhere (tests).
fn control_dir() -> Option<PathBuf> {
    std::env::var_os("KEEP_MCP_CONTROL_DIR")
        .map(PathBuf::from)
        .or_else(mk_control::config_dir)
}

/// The direct route's registry, with network access for bank feeds.
fn registry() -> Registry {
    let mut r = Registry::with_builtins();
    r.set_connector(mk_connect::connector());
    r
}

fn run(cli: Cli) -> Result<(), String> {
    if let Some(Cmd::Config { client, book, app }) = cli.command {
        let kind = mk_control::ClientKind::parse(&client).ok_or_else(|| {
            format!("unknown client {client:?}: use claude-desktop, claude-code or generic")
        })?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let v = config_json(kind, &exe, &target(book, app)?);
        println!(
            "{}",
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())
        );
        return Ok(());
    }
    let target = target(cli.book, cli.app)?;
    let router = Router::new(target, control_dir(), Box::new(registry));
    let mut server = mk_mcp::Server::new(Box::new(router));
    server
        .serve(
            std::io::BufReader::new(std::io::stdin()),
            std::io::stdout().lock(),
        )
        .map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("keep-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
