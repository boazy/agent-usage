mod api;
mod auth;
mod cli;
mod render;
mod theme;
mod time;
mod usage;

use crate::api::fetch_usage;
use crate::auth::{expand_path, load_auth, persist_auth};
use crate::cli::{resolve_theme, Cli};
use crate::render::print_usage_report;
use clap::Parser;
use eyre::{Result, WrapErr};

fn main() -> std::process::ExitCode {
    match start() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            anstream::eprintln!("Error: {error:?}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn start() -> Result<()> {
    color_eyre::install().wrap_err("failed to install error reporting")?;
    run(&Cli::parse())
}

fn run(cli: &Cli) -> Result<()> {
    let theme = resolve_theme(cli)?;
    let auth_path = expand_path(&cli.auth_file);
    let mut auth = load_auth(&auth_path)?;
    let original_auth = auth.clone();
    let usage_result = fetch_usage(cli, &mut auth, !cli.no_progress);

    if auth.needs_persisted_refresh(&original_auth) {
        if let Err(error) = persist_auth(&auth_path, &auth) {
            anstream::eprintln!("warning: refreshed credentials could not be saved: {error:#}");
        }
    }

    let usage = usage_result?;
    if cli.json {
        anstream::println!(
            "{}",
            serde_json::to_string_pretty(&usage.raw).wrap_err("failed to serialize usage JSON")?
        );
        return Ok(());
    }
    print_usage_report(&usage, &auth, theme, !cli.no_progress);
    Ok(())
}
