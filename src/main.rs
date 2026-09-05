mod api;
mod auth;
mod cli;
mod config;
mod dashboard;
mod render;
mod sources;
mod theme;
mod time;
mod tui;
mod usage;

use crate::cli::Cli;
use crate::config::{DashboardConfig, SourceConfig, SourceKind};
use crate::dashboard::Dashboard;
use clap::Parser;
use eyre::{eyre, Result, WrapErr};
use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

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
    let cli = Cli::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .wrap_err("failed to create async runtime")?
        .block_on(run(cli))
}

async fn run(cli: Cli) -> Result<()> {
    if cli.tui && (!std::io::stdout().is_terminal() || !std::io::stdin().is_terminal()) {
        return Err(eyre!(
            "--tui requires an interactive terminal; use --report or --json for pipes"
        ));
    }
    let mut config = DashboardConfig::load(cli.config.as_deref())?;
    if let Some(base_url) = cli.base_url {
        config.codex_base_url = base_url;
    }
    if let Some(auth_file) = cli.auth_file {
        if let Some(source) = config
            .sources
            .iter_mut()
            .find(|source| source.kind == SourceKind::Codex)
        {
            source.path = auth_file.into();
            source.enabled = true;
            source.optional = false;
        } else {
            config.sources.push(SourceConfig {
                name: "codex-cli".to_owned(),
                kind: SourceKind::Codex,
                path: auth_file.into(),
                enabled: true,
                optional: false,
            });
        }
    }
    if let Some(theme) = cli.theme {
        theme.trim().clone_into(&mut config.theme);
    }
    config.validate()?;
    let theme = crate::theme::theme_by_name(&config.theme).ok_or_else(|| {
        eyre!(
            "unknown theme; supported themes: {}",
            crate::theme::available_theme_names()
        )
    })?;
    let refresh = Duration::from_secs(config.refresh_seconds);
    let dashboard = Arc::new(
        tokio::task::spawn_blocking(move || Dashboard::load(&config))
            .await
            .wrap_err("credential discovery worker failed")??,
    );
    for warning in dashboard.warnings() {
        anstream::eprintln!("Warning: {warning}");
    }
    if cli.tui {
        return tui::run(dashboard, &theme, refresh).await;
    }
    if !cli.no_progress && std::io::stderr().is_terminal() && !dashboard.accounts().is_empty() {
        anstream::eprintln!(
            "Fetching usage for {} account(s)…",
            dashboard.accounts().len()
        );
    }
    let ids: Vec<_> = dashboard
        .accounts()
        .iter()
        .map(|account| account.id.clone())
        .collect();
    let snapshots = dashboard.refresh(&ids).await;
    if cli.json {
        anstream::println!(
            "{}",
            serde_json::to_string_pretty(&snapshots)
                .wrap_err("failed to serialize account usage JSON")?
        );
    } else {
        render::render_report(&snapshots, render::report_width(cli.width), &theme)
            .wrap_err("failed to render usage report")?;
    }
    Ok(())
}
