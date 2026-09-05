use clap::Parser;
use std::path::PathBuf;

#[expect(
    clippy::struct_excessive_bools,
    reason = "clap models independent command-line switches; output-mode conflicts are enforced by its argument schema"
)]
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

    /// Base URL override for Codex account endpoints. Overrides configured `codex_base_url` only when supplied.
    #[arg(short = 'b', long)]
    pub(crate) base_url: Option<String>,

    /// Choose the report and dashboard palette (e.g. default, kanagawa, rose-pine).
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

    /// Disable progress messages (report gauges remain visible).
    #[arg(long)]
    pub(crate) no_progress: bool,
}
