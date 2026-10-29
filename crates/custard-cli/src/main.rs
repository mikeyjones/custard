//! `custard`: create Custard apps and work on them.
//!
//! Every command runs without prompting, so coding agents can use it as well
//! as people. `new` takes `--json` to report what it did as one JSON object.
//! The Topcoat CLI is built in: `custard dev`, `fmt`, `ui`, `asset` and `l10n`
//! are its commands, so nothing else needs installing.

mod add;
mod common;
mod feature;
mod files;
mod new;

use std::{ffi::OsString, process::ExitCode};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "custard",
    version,
    about = "Create and work on Custard apps for The Custard Factory"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create an app ready to run, and to deploy to The Custard Factory
    New(new::NewArgs),
    /// Add part of an app, ready to use: a page, a feature or a migration
    #[command(subcommand)]
    Add(add::AddCommand),
    /// Build, bundle and serve the app, reloading it on changes (topcoat dev)
    #[command(disable_help_flag = true)]
    Dev(Passthrough),
    /// Format Rust and the bodies of view! and other macros (topcoat fmt)
    #[command(disable_help_flag = true)]
    Fmt(Passthrough),
    /// Add, list and update Topcoat UI components (topcoat ui)
    #[command(disable_help_flag = true)]
    Ui(Passthrough),
    /// Inspect and bundle the app's assets (topcoat asset)
    #[command(disable_help_flag = true)]
    Asset(Passthrough),
    /// Check the app's translations (topcoat l10n)
    #[command(disable_help_flag = true)]
    L10n(Passthrough),
    /// Run any Topcoat CLI command
    #[command(disable_help_flag = true, hide = true)]
    Topcoat(Passthrough),
}

#[derive(clap::Args)]
struct Passthrough {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<OsString>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let (name, passthrough) = match Cli::parse().command {
        Command::New(args) => return new::run(args),
        Command::Add(command) => return add::run(command),
        Command::Dev(p) => ("dev", p),
        Command::Fmt(p) => ("fmt", p),
        Command::Ui(p) => ("ui", p),
        Command::Asset(p) => ("asset", p),
        Command::L10n(p) => ("l10n", p),
        Command::Topcoat(p) => ("", p),
    };
    let mut args: Vec<OsString> = vec!["topcoat".into()];
    if !name.is_empty() {
        args.push(name.into());
    }
    args.extend(passthrough.args);
    topcoat_cli::run_from(args).await;
    ExitCode::SUCCESS
}
