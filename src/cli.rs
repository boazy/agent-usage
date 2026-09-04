use crate::theme::{available_theme_names, theme_by_name, Theme};
use clap::Parser;
use config::{Config, Environment, File as ConfigFile};
use eyre::{eyre, Result, WrapErr};
use platform_dirs::AppDirs;
use std::path::PathBuf;

const DEFAULT_AUTH_PATH: &str = "~/.codex/auth.json";
pub(crate) const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api";
const DEFAULT_THEME_NAME: &str = "default";

#[derive(Debug, Parser)]
#[command(
    name = "codex-usage",
    version,
    about = "Show Codex usage details from ~/.codex/auth.json"
)]
pub(crate) struct Cli {
    /// Path to auth JSON file (defaults to ~/.codex/auth.json).
    #[arg(short, long, default_value = DEFAULT_AUTH_PATH)]
    pub(crate) auth_file: String,

    /// Base URL override for Codex account endpoints.
    /// For chatgpt.com/chat.openai.com, path is normalized to `<origin>/backend-api`.
    #[arg(short = 'b', long, default_value = DEFAULT_BASE_URL)]
    pub(crate) base_url: String,

    /// Override the progress bar theme (e.g. default, solarized-dark, monokai, molokai).
    #[arg(long)]
    pub(crate) theme: Option<String>,

    /// Print the raw JSON payload and exit.
    #[arg(short, long)]
    pub(crate) json: bool,

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

fn config_file_path() -> Option<PathBuf> {
    AppDirs::new(Some("codex-usage"), true)
        .map(|directories| directories.config_dir.join("config.toml"))
}

fn resolve_theme_name(cli: &Cli) -> Result<String> {
    if let Some(theme) = &cli.theme {
        return Ok(theme.trim().to_owned());
    }

    let mut builder = Config::builder()
        .set_default("theme", DEFAULT_THEME_NAME)
        .wrap_err("failed to set the default theme")?;
    if let Some(config_path) = config_file_path().filter(|path| path.exists()) {
        builder = builder.add_source(ConfigFile::from(config_path));
    }
    builder = builder.add_source(Environment::with_prefix("CODEX_USAGE"));
    builder
        .build()
        .wrap_err("failed to load theme configuration")?
        .get_string("theme")
        .wrap_err("theme configuration is not a string")
}
