use crate::theme::{available_theme_names, theme_by_name, Theme};
use clap::Parser;
use eyre::{eyre, Result};
use std::path::PathBuf;

pub(crate) const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api";
const DEFAULT_THEME_NAME: &str = "default";

#[derive(Debug, Parser)]
#[command(
    name = "agent-usage",
    version,
    about = "Show usage details for accounts and providers"
)]
pub(crate) struct Cli {
    /// Path to a dashboard TOML configuration file.
    #[arg(long, value_name = "PATH")]
    pub(crate) config: Option<PathBuf>,

    /// Optional legacy Codex auth override. When omitted, configured sources are used.
    #[arg(short, long, value_name = "PATH")]
    pub(crate) auth_file: Option<String>,

    /// Base URL override for Codex account endpoints. Overrides configured codex_base_url only when supplied.
    #[arg(short = 'b', long)]
    pub(crate) base_url: Option<String>,

    /// Override the progress bar theme (e.g. default, solarized-dark, monokai, molokai).
    #[arg(long)]
    pub(crate) theme: Option<String>,

    /// Print normalized account usage JSON and exit.
    #[arg(short, long, conflicts_with = "tui")]
    pub(crate) json: bool,
    /// Run the interactive Ratatui dashboard.
    #[arg(long, conflicts_with_all = ["report", "json"])]
    pub(crate) tui: bool,

    /// Render a non-interactive report to ordinary stdout.
    #[arg(long, conflicts_with = "tui")]
    pub(crate) report: bool,

    /// Force report width (otherwise use terminal width or a pipe-safe fallback).
    #[arg(long, value_name = "COLUMNS")]
    pub(crate) width: Option<u16>,

    /// Disable progress bars (for logs/CI).
    #[arg(long)]
    pub(crate) no_progress: bool,
}

pub(crate) fn resolve_theme(cli: &Cli) -> Result<Theme> {
    let name = resolve_theme_name(cli)?;
    theme_by_name(&name).ok_or_else(|| {
        eyre!(
            "unknown theme '{name}'. Supported themes: {}",
            available_theme_names()
        )
    })
}

fn resolve_theme_name(cli: &Cli) -> Result<String> {
    Ok(cli
        .theme
        .as_deref()
        .unwrap_or(DEFAULT_THEME_NAME)
        .trim()
        .to_owned())
}
