use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use clap::Parser;
use config::{Config, Environment, File as ConfigFile};
use indicatif::{ProgressBar, ProgressStyle};
use platform_dirs::AppDirs;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde_json::{Map, Value};
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

type AppResult<T> = Result<T, String>;

const DEFAULT_AUTH_PATH: &str = "~/.codex/auth.json";
const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api";
const DEFAULT_THEME_NAME: &str = "default";
const USER_AGENT_VALUE: &str = "codex-usage-rs/0.1.0";
const JWT_AUTH_CLAIM: &str = "https://api.openai.com/auth";
const JWT_PROFILE_CLAIM: &str = "https://api.openai.com/profile";
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
#[derive(Copy, Clone)]
struct Theme {
    name: &'static str,
    bar_ok: &'static str,
    bar_warning: &'static str,
    bar_exhausted: &'static str,
    bar_unknown: &'static str,
    bar_empty: &'static str,
    meter_color: &'static str,
    window_color: &'static str,
    reset_color: &'static str,
    error_color: &'static str,
}

const BUILTIN_THEMES: &[Theme] = &[
    Theme {
        bar_ok: "green",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "blue",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;39m",
        window_color: "\x1b[38;5;243m",
        reset_color: "\x1b[38;5;244m",
        error_color: "\x1b[31m",
        name: "default",
    },
    Theme {
    /// Disable progress bars (for logs/CI).
    #[arg(long)]
    no_progress: bool,
}
        bar_ok: "green",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "blue",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;39m",
        window_color: "\x1b[38;5;243m",
        reset_color: "\x1b[38;5;244m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "solarized-dark",
        bar_ok: "bright_cyan",
        bar_warning: "bright_yellow",
        bar_exhausted: "red",
        bar_unknown: "bright_blue",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;75m",
        window_color: "\x1b[38;5;144m",
        reset_color: "\x1b[38;5;245m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "solarized-light",
        bar_ok: "blue",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "cyan",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;33m",
        window_color: "\x1b[38;5;101m",
        reset_color: "\x1b[38;5;244m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "monokai",
        bar_ok: "green",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "magenta",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;77m",
        window_color: "\x1b[38;5;180m",
        reset_color: "\x1b[38;5;244m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "molokai",
        bar_ok: "bright_green",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "magenta",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;148m",
        window_color: "\x1b[38;5;244m",
        reset_color: "\x1b[38;5;245m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "dracula",
        bar_ok: "bright_magenta",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "bright_cyan",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;141m",
        window_color: "\x1b[38;5;103m",
        reset_color: "\x1b[38;5;243m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "gruvbox-dark",
        bar_ok: "yellow",
        bar_warning: "bright_yellow",
        bar_exhausted: "bright_red",
        bar_unknown: "cyan",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;214m",
        window_color: "\x1b[38;5;180m",
        reset_color: "\x1b[38;5;244m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "gruvbox-light",
        bar_ok: "blue",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "magenta",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;66m",
        window_color: "\x1b[38;5;59m",
        reset_color: "\x1b[38;5;244m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "one-dark",
        bar_ok: "bright_green",
        bar_warning: "bright_yellow",
        bar_exhausted: "bright_red",
        bar_unknown: "bright_cyan",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;114m",
        window_color: "\x1b[38;5;181m",
        reset_color: "\x1b[38;5;245m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "one-light",
        bar_ok: "green",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "blue",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;28m",
        window_color: "\x1b[38;5;101m",
        reset_color: "\x1b[38;5;244m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "nord",
        bar_ok: "bright_cyan",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "blue",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;74m",
        window_color: "\x1b[38;5;104m",
        reset_color: "\x1b[38;5;244m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "github-dark",
        bar_ok: "bright_green",
        bar_warning: "yellow",
        bar_exhausted: "bright_red",
        bar_unknown: "blue",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;47m",
        window_color: "\x1b[38;5;249m",
        reset_color: "\x1b[38;5;240m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "github-light",
        bar_ok: "blue",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "cyan",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;24m",
        window_color: "\x1b[38;5;244m",
        reset_color: "\x1b[38;5;244m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "nord-dark",
        bar_ok: "bright_blue",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "magenta",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;117m",
        window_color: "\x1b[38;5;152m",
        reset_color: "\x1b[38;5;245m",
        error_color: "\x1b[31m",
    },
    Theme {
        name: "catppuccin-mocha",
        bar_ok: "bright_magenta",
        bar_warning: "yellow",
        bar_exhausted: "bright_red",
        bar_unknown: "cyan",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;141m",
        window_color: "\x1b[38;5;103m",
        reset_color: "\x1b[38;5;243m",
        error_color: "\x1b[31m",
fn config_file_path() -> Option<PathBuf> {
    AppDirs::new(Some("codex-usage"), true).map(|app_dirs| app_dirs.config_dir.join("config.toml"))
}
    })
    })
}
        bar_ok: "bright_blue",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "magenta",
        bar_empty: "bright_black",
        meter_color: "\x1b[38;5;110m",
        window_color: "\x1b[38;5;246m",
        reset_color: "\x1b[38;5;242m",
        error_color: "\x1b[31m",
    },
];
const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api";
fn config_file_path() -> Option<PathBuf> {
    let app_dirs = AppDirs::new("codex-usage", true);
    app_dirs.config_dir().map(|path| path.join("config.toml"))
}

fn available_theme_names() -> String {
    BUILTIN_THEMES
        .iter()
        .map(|theme| theme.name)
        .collect::<Vec<_>>()
        .join(", ")
}

fn theme_by_name(name: &str) -> Option<Theme> {
    BUILTIN_THEMES
        .iter()
        .find(|theme| theme.name.eq_ignore_ascii_case(name))
        .copied()
}

fn resolve_theme_name(cli: &Cli) -> AppResult<String> {
    if let Some(theme) = &cli.theme {
        return Ok(theme.trim().to_string());
    }

    let mut builder = Config::builder()
        .set_default("theme", DEFAULT_THEME_NAME)
        .map_err(|e| e.to_string())?;

    if let Some(config_path) = config_file_path() {
        if config_path.exists() {
            builder = builder.add_source(ConfigFile::from(config_path));
        }
    }

    builder = builder.add_source(Environment::with_prefix("CODEX_USAGE"));

    let cfg = builder.build().map_err(|e| e.to_string())?;
    cfg.get_string("theme")
        .map_err(|e| e.to_string())
}

fn resolve_theme(cli: &Cli) -> AppResult<Theme> {
    let name = resolve_theme_name(cli)?;
    theme_by_name(&name).ok_or_else(|| {
        format!(
            "unknown theme '{name}'. Supported themes: {}",
            available_theme_names()
        )
    })
}

#[derive(Debug, Parser)]
#[command(
    name = "codex-usage",
    version,
    about = "Show Codex usage details from ~/.codex/auth.json"
)]
struct Cli {
    /// Path to auth JSON file (defaults to ~/.codex/auth.json).
    #[arg(short, long, default_value = DEFAULT_AUTH_PATH)]
    auth_file: String,

    /// Base URL override for Codex account endpoints.
    /// For chatgpt.com/chat.openai.com, path is normalized to `<origin>/backend-api`.
    #[arg(short = 'b', long, default_value = DEFAULT_BASE_URL)]
    base_url: String,

    /// Override the progress bar theme (e.g. default, solarized-dark, monokai, molokai).
    #[arg(long)]
    theme: Option<String>,

    /// Print the raw JSON payload and exit.
    #[arg(short, long)]
    json: bool,

    /// Disable progress bars (for logs/CI).
    #[arg(long)]
    no_progress: bool,
}

    /// Disable progress bars (for logs/CI).
    #[arg(long)]
    no_progress: bool,
}

#[derive(Clone)]
struct AuthRecord {
    access_token: String,
    account_id: Option<String>,
#[derive(Clone)]
#[derive(Clone)]
struct UsageWindow {
    used_percent: Option<f64>,
    limit_window_seconds: Option<u64>,
    reset_after_seconds: Option<u64>,
    reset_at: Option<u64>,
}

#[derive(Clone)]
struct RateLimit {
    allowed: Option<bool>,
    limit_reached: Option<bool>,
    primary_window: Option<UsageWindow>,
    secondary_window: Option<UsageWindow>,
}

#[derive(Clone)]
struct AdditionalRateLimit {
    limit_name: Option<String>,
    metered_feature: Option<String>,
    rate_limit: Option<RateLimit>,
}

fn run(cli: Cli) -> AppResult<()> {
    let theme = resolve_theme(&cli)?;
    let auth_path = expand_path(&cli.auth_file);
    let mut auth = load_auth(&auth_path)?;
    let original = auth.clone();

    let usage_result = fetch_usage(&cli, &mut auth, !cli.no_progress);
    if auth.needs_persisted_refresh(&original) {
        persist_auth(&auth_path, &auth)?;
    }

    let usage = usage_result?;
    if cli.json {
        let pretty = serde_json::to_string_pretty(&usage.raw).map_err(|e| e.to_string())?;
        println!("{pretty}");
        return Ok(());
    }

    print_usage_report(&usage, &auth, &theme, !cli.no_progress);
    Ok(())
}
    access_token: String,
    id_token: Option<String>,
    refresh_token: Option<String>,
    account_id: Option<String>,
    email: Option<String>,
    oauth_client_id: Option<String>,
}

impl AuthRecord {
    fn needs_persisted_refresh(&self, prior: &AuthRecord) -> bool {
        self.access_token != prior.access_token
            || self.id_token != prior.id_token
            || self.refresh_token != prior.refresh_token
            || self.account_id != prior.account_id
fn run(cli: Cli) -> AppResult<()> {
    let theme = resolve_theme(&cli)?;
    let auth_path = expand_path(&cli.auth_file);
    let mut auth = load_auth(&auth_path)?;
    let original = auth.clone();

    let usage_result = fetch_usage(&cli, &mut auth, !cli.no_progress);
    if auth.needs_persisted_refresh(&original) {
        persist_auth(&auth_path, &auth)?;
    }

    let usage = usage_result?;
    if cli.json {
        let pretty = serde_json::to_string_pretty(&usage.raw).map_err(|e| e.to_string())?;
        println!("{pretty}");
        return Ok(());
    }

    print_usage_report(&usage, &auth, &theme, !cli.no_progress);
    Ok(())
}
}
    limit_reached: Option<bool>,
    primary_window: Option<UsageWindow>,
    secondary_window: Option<UsageWindow>,
}

#[derive(Clone)]
struct AdditionalRateLimit {
    limit_name: Option<String>,
    metered_feature: Option<String>,
    rate_limit: Option<RateLimit>,
}

#[derive(Clone)]
#[derive(Clone)]
struct UsageItem {
    meter: String,
    window_label: String,
    used_percent: Option<f64>,
    status: UsageStatus,
    reset_text: Option<String>,
}

#[derive(Clone)]
struct UsageItem {
    label: String,
    used_percent: Option<f64>,
    status: UsageStatus,
    reset_text: Option<String>,
}

#[derive(Copy, Clone)]
enum UsageStatus {
    Ok,
    Warning,
    Exhausted,
    Unknown,
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> AppResult<()> {
    let auth_path = expand_path(&cli.auth_file);
    let auth = load_auth(&auth_path)?;
    let usage = fetch_usage(&cli, &auth, !cli.no_progress)?;
fn run(cli: Cli) -> AppResult<()> {
    let auth_path = expand_path(&cli.auth_file);
    let mut auth = load_auth(&auth_path)?;
    let usage_result = fetch_usage(&cli, &mut auth, !cli.no_progress);
    if auth.needs_persisted_refresh(&original) {
        persist_auth(&auth_path, &auth)?;
    }

    let usage = usage_result?;
    }

    if cli.json {
        let pretty = serde_json::to_string_pretty(&usage.raw).map_err(|e| e.to_string())?;
        println!("{pretty}");
        return Ok(());
    }

    print_usage_report(&usage, &auth, !cli.no_progress);
    Ok(())
fn load_auth(path: &Path) -> AppResult<AuthRecord> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let payload: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;

    let access_token = payload
        .pointer("/tokens/access_token")
        .and_then(as_string)
        .or_else(|| payload.pointer("/OPENAI_API_KEY").and_then(as_string))
        .ok_or_else(|| "auth file missing tokens.access_token".to_string())?;

    let id_token = payload.pointer("/tokens/id_token").and_then(as_string);
    let refresh_token = payload.pointer("/tokens/refresh_token").and_then(as_string);
    let oauth_client_id = id_token
        .as_deref()
        .and_then(parse_jwt_aud)
        .or_else(|| parse_jwt_aud(&access_token));

    let account_id = payload
        .pointer("/tokens/account_id")
        .and_then(as_string)
        .or_else(|| parse_jwt_claim(&access_token, JWT_AUTH_CLAIM, "chatgpt_account_id"));

    let email = id_token
        .as_deref()
        .and_then(|token| parse_jwt_claim(token, JWT_PROFILE_CLAIM, "email"))
        .or_else(|| parse_jwt_claim(&access_token, JWT_PROFILE_CLAIM, "email"));

    Ok(AuthRecord {
        access_token,
        id_token,
        refresh_token,
        account_id,
        email,
        oauth_client_id,
    })
}

fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn normalize_base_url(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    let parsed = Url::parse(trimmed);
    if let Ok(url) = parsed {
        if let Some(host) = url.host_str() {
            let host = host.to_ascii_lowercase();
            if host == "chatgpt.com" || host == "chat.openai.com" {
                let mut base = format!("{}://{}", url.scheme(), host);
                if let Some(port) = url.port() {
                    base.push(':');
                    base.push_str(&port.to_string());
                }
                return format!("{}/backend-api", base);
            }
        }

        return trimmed.to_string();
    }
    DEFAULT_BASE_URL.to_string()
}
    })
}

fn fetch_usage(cli: &Cli, auth: &mut AuthRecord, use_progress: bool) -> AppResult<ParsedUsage> {
    let base = normalize_base_url(&cli.base_url);
    let url = format!("{}/wham/usage", base);

    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;

    let spinner = if use_progress {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner} {msg}")
                .unwrap()
                .tick_strings(&["◒", "◐", "◓", "◑"]),
        );
        pb.set_message("Fetching Codex usage…");
        pb.enable_steady_tick(Duration::from_millis(80));
        Some(pb)
    } else {
        None
    let request = |auth: &AuthRecord| -> AppResult<reqwest::blocking::Response> {

    let mut request = |auth: &AuthRecord| -> AppResult<reqwest::blocking::Response> {
        let mut headers = HeaderMap::new();
        let bearer = format!("Bearer {}", auth.access_token);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&bearer).map_err(|e| e.to_string())?,
        );
        headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
        if let Some(account_id) = &auth.account_id {
            headers.insert(
                "ChatGPT-Account-Id",
                HeaderValue::from_str(account_id).map_err(|e| e.to_string())?,
            );
        }

        client
            .get(&url)
            .headers(headers)
            .send()
            .map_err(|e| format!("usage request failed: {e}"))
    };

    let first_response = request(auth)?;
    let first_status = first_response.status();

    if first_status.is_success() {
        if let Some(pb) = &spinner {
            pb.finish_and_clear();
        }
        let payload = first_response.json().map_err(|e| e.to_string())?;
        return Ok(parse_usage_payload(payload));
    }

    let first_body = first_response
        .text()
        .unwrap_or_else(|_| "unable to read response body".to_string());

    if (first_status == 401 || first_status == 403) && auth.refresh_token.is_some() {
        if let Some(pb) = &spinner {
            pb.set_message("Refreshing Codex access token…");
        }
        match refresh_access_token(auth) {
            Ok(()) => {
                let second_response = request(auth)?;
                if second_response.status().is_success() {
                    if let Some(pb) = &spinner {
                        pb.finish_and_clear();
                    }
                    let payload = second_response.json().map_err(|e| e.to_string())?;
                    return Ok(parse_usage_payload(payload));
                }

                let second_status = second_response.status();
                let second_body = second_response
                    .text()
                    .unwrap_or_else(|_| "unable to read response body".to_string());
                if let Some(pb) = &spinner {
                    pb.finish_and_clear();
                }
                return Err(format!(
                    "usage endpoint returned HTTP {} after refresh: {}",
                    second_status,
                    trim_body(&second_body)
                ));
            }
            Err(refresh_err) => {
                if let Some(pb) = &spinner {
                    pb.finish_and_clear();
                }
                return Err(format!(
                    "usage request returned HTTP {}: {}. token refresh failed: {}",
                    first_status,
                    trim_body(&first_body),
                    refresh_err
                ));
            }
        }
    }

    if let Some(pb) = &spinner {
        pb.finish_and_clear();
    }
struct TempAuthFile {
    path: PathBuf,
    remove_on_drop: bool,
}

impl TempAuthFile {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            remove_on_drop: true,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn commit(&mut self) {
        self.remove_on_drop = false;
    }
}

impl Drop for TempAuthFile {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn next_temp_auth_path(path: &Path, attempt: u128) -> PathBuf {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    directory.join(format!(
        ".codex-auth-{}-{}-{}.json.tmp",
        std::process::id(),
        nanos,
        attempt
    ))
}

fn create_private_temp_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        options.mode(0o600);
    }

    let file = options.open(path)?;

    #[cfg(unix)]
    {
        if let Err(err) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            let _ = fs::remove_file(path);
            return Err(err);
        }
    }

    Ok(file)
}
}

    let dir = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dir = fs::File::open(path).map_err(|e| e.to_string())?;
    dir.sync_all().map_err(|e| e.to_string())
}

fn persist_auth(path: &Path, auth: &AuthRecord) -> AppResult<()> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut payload: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;

    let tokens = payload
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "auth file missing tokens object".to_string())?;

    tokens.insert(
        "access_token".to_string(),
        Value::String(auth.access_token.clone()),
    );
    if let Some(id_token) = auth.id_token.as_ref() {
        tokens.insert("id_token".to_string(), Value::String(id_token.clone()));
    } else {
        tokens.remove("id_token");
    }
    if let Some(refresh_token) = auth.refresh_token.as_ref() {
        tokens.insert(
            "refresh_token".to_string(),
            Value::String(refresh_token.clone()),
        );
    } else {
        tokens.remove("refresh_token");
    }
    if let Some(account_id) = auth.account_id.as_ref() {
        tokens.insert("account_id".to_string(), Value::String(account_id.clone()));
    } else {
        tokens.remove("account_id");
    }

    let formatted = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    let directory = path.parent().unwrap_or_else(|| Path::new("."));

    let mut temp_file = None;
    let mut temp_path_guard = None;

    for attempt in 0..32 {
        let temp_path = next_temp_auth_path(path, attempt);
        match create_private_temp_file(&temp_path) {
            Ok(file) => {
                temp_file = Some(file);
                temp_path_guard = Some(TempAuthFile::new(temp_path));
                break;
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists && attempt < 31 => {
                continue;
            }
            Err(err) => {
                return Err(format!(
                    "failed to create temporary auth file {}: {}",
                    temp_path.display(),
                    err
                ));
            }
        }
    }

    let mut file = temp_file.ok_or_else(|| {
        "failed to create unique temporary auth file: collision on all candidates".to_string()
    })?;
    let mut guard = temp_path_guard
        .take()
        .expect("temporary path guard must exist if temp file exists");

    file.write_all(formatted.as_bytes())
        .map_err(|e| format!("failed writing temporary auth file {}: {e}", guard.path().display()))?;
    file.flush().map_err(|e| {
        format!(
            "failed flushing temporary auth file {}: {e}",
            guard.path().display()
        )
    })?;
    file.sync_all().map_err(|e| {
        format!(
            "failed syncing temporary auth file {}: {e}",
            guard.path().display()
        )
    })?;

    drop(file);

    fs::rename(guard.path(), path).map_err(|e| {
        format!(
            "failed replacing {} with temporary {}: {}",
            path.display(),
            guard.path().display(),
            e
        )
    })?;

    guard.commit();
    sync_directory_for(directory)
}
fn refresh_access_token(auth: &mut AuthRecord) -> AppResult<()> {
    let refresh_token = auth
        .refresh_token
        .as_deref()
        .ok_or_else(|| "auth file missing tokens.refresh_token".to_string())?;
    }
    if let Some(account_id) = auth.account_id.as_ref() {
        tokens.insert("account_id".to_string(), Value::String(account_id.clone()));
    } else {
        tokens.remove("account_id");
    }

    let formatted = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    fs::write(path, formatted).map_err(|e| e.to_string())?;
    Ok(())
}

fn refresh_access_token(auth: &mut AuthRecord) -> AppResult<()> {
    let refresh_token = auth
        .refresh_token
        .as_deref()
        .ok_or_else(|| "auth file missing tokens.refresh_token".to_string())?;

    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .post("https://auth.openai.com/oauth/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", auth.default_oauth_client_id()),
#[derive(Clone, PartialEq, Eq)]
struct BankReset {
struct BankReset {
    source: String,
    expires_in: String,
}

fn print_usage_report(usage: &ParsedUsage, auth: &AuthRecord, theme: &Theme, use_progress: bool) {
    println!("{}", colorize(theme.meter_color, "Codex plan usage"));
    println!("{}", colorize(theme.meter_color, "================"));

    println!("{}", format_account_plan_line(auth, usage, theme));
    if let Some(credits) = usage.reset_credits_available {
        println!(
            "{} {}",
            colorize(theme.window_color, "Reset credits:"),
            colorize(theme.meter_color, &credits.to_string())
        );
    }
    }
    println!();
    let now_ms = now_millis();
    let mut items = collect_items(usage, now_ms);
    if items.is_empty() {
        println!("No usage windows were reported by this endpoint.");
        return;
    }

    for item in items.drain(..) {
        render_usage_item(&item, theme, use_progress);
    }

    print_banked_resets(&collect_banked_resets(usage, now_ms), theme);
}

fn format_account_plan_line(auth: &AuthRecord, usage: &ParsedUsage, theme: &Theme) -> String {
    let account = auth.email.as_deref().unwrap_or("unknown");
    let plan = usage.plan_type.as_deref().unwrap_or("unknown");
    format!(
        "{} {} {}",
        colorize(theme.window_color, "Account:"),
        colorize(theme.meter_color, account),
        colorize(theme.reset_color, &format!("({plan})")),
    )
}
fn collect_banked_resets(payload: &ParsedUsage, now_ms: u64) -> Vec<BankReset> {
    let mut resets = Vec::new();

    if let Some(rate_limit) = &payload.rate_limit {
        if let Some(window) = &rate_limit.primary_window {
            push_banked_reset(
                &mut resets,
                format_window_labelled_source("Codex", window),
                reset_expiration_text(window, now_ms),
            );
        }
        if let Some(window) = &rate_limit.secondary_window {
            push_banked_reset(
                &mut resets,
                format_window_labelled_source("Codex", window),
                reset_expiration_text(window, now_ms),
            );
        }
    }

    for extra in &payload.additional_rate_limits {
        let slug = additional_limit_slug(
            extra.limit_name.as_deref(),
            extra.metered_feature.as_deref(),
        );
        let display_name = match slug.as_str() {
            "spark" => "Spark".to_string(),
            "chat" => "Codex".to_string(),
            _ => extra
                .limit_name
                .as_ref()
                .map(|name| normalize_usage_label(name))
                .unwrap_or_else(|| title_case_slug(&slug)),
        };
        let Some(rate_limit) = &extra.rate_limit else {
            continue;
        };

        if let Some(window) = &rate_limit.primary_window {
            push_banked_reset(
                &mut resets,
                format_window_labelled_source(&display_name, window),
                reset_expiration_text(window, now_ms),
            );
        }
        if let Some(window) = &rate_limit.secondary_window {
            push_banked_reset(
                &mut resets,
                format_window_labelled_source(&display_name, window),
                reset_expiration_text(window, now_ms),
            );
        }
    }

    resets
}

fn push_banked_reset(
    resets: &mut Vec<BankReset>,
    source: String,
    expiration: Option<String>,
) {
    let Some(expires_in) = expiration else {
        return;
    };
    let item = BankReset { source, expires_in };
    if !resets.iter().any(|known| known == &item) {
        resets.push(item);
    }
}

fn format_window_labelled_source(display_name: &str, window: &UsageWindow) -> String {
    let window_label = window_label(window.limit_window_seconds).to_lowercase();
    if window_label == "unknown" {
        display_name.to_string()
    } else {
        format!("{display_name} ({window_label})")
    }
}

fn reset_expiration_text(window: &UsageWindow, now_ms: u64) -> Option<String> {
    let reset_ms = resolve_reset_time(window, now_ms)?;
    let diff_secs = (reset_ms.saturating_sub(now_ms) / 1000).clamp(0, u64::MAX);
    Some(format!("expires in {}", human_duration(diff_secs)))
}

fn print_banked_resets(resets: &[BankReset], theme: &Theme) {
    if resets.is_empty() {
        return;
    }

    println!();
    println!("{}", colorize(theme.window_color, "Bank resets"));
    println!("{}", colorize(theme.window_color, "------------"));
    for reset in resets {
        println!(
            "  {} {}",
            colorize(theme.window_color, "•"),
            colorize(
                theme.window_color,
                &format!("{} — {}", reset.source, reset.expires_in),
            )
        );
    }
}

fn collect_items(payload: &ParsedUsage, now_ms: u64) -> Vec<UsageItem> {
    let mut items = Vec::new();

    if let Some(rate_limit) = &payload.rate_limit {
        if let Some(window) = &rate_limit.primary_window {
            items.push(build_usage_item(
                "Codex",
                window,
                rate_limit.allowed,
                rate_limit.limit_reached,
                now_ms,
            ));
        }
        if let Some(window) = &rate_limit.secondary_window {
            items.push(build_usage_item(
                "Codex",
                window,
                rate_limit.allowed,
                rate_limit.limit_reached,
                now_ms,
            ));
        }
    }

    for extra in &payload.additional_rate_limits {
        let slug = additional_limit_slug(
            extra.limit_name.as_deref(),
            extra.metered_feature.as_deref(),
        );
        let display_name = match slug.as_str() {
            "spark" => "Spark".to_string(),
            "chat" => "Codex".to_string(),
            _ => extra
                .limit_name
                .as_ref()
                .map(|name| normalize_usage_label(name))
                .unwrap_or_else(|| title_case_slug(&slug)),
        };

        if let Some(rate_limit) = &extra.rate_limit {
            if let Some(window) = &rate_limit.primary_window {
                items.push(build_usage_item(
                    &display_name,
                    window,
                    rate_limit.allowed,
                    rate_limit.limit_reached,
                    now_ms,
                ));
            }
            if let Some(window) = &rate_limit.secondary_window {
                items.push(build_usage_item(
                    &display_name,
                    window,
                    rate_limit.allowed,
                    rate_limit.limit_reached,
                    now_ms,
                ));
            }
        }
    }

    items
}

fn build_usage_item(
    meter: &str,
    window: &UsageWindow,
    allowed: Option<bool>,
    limit_reached: Option<bool>,
    now_ms: u64,
) -> UsageItem {
    UsageItem {
        meter: meter.to_string(),
        window_label: window_label(window.limit_window_seconds),
        used_percent: window.used_percent,
        status: usage_status(window.used_percent, allowed, limit_reached),
        reset_text: resolve_reset_text(window, now_ms),
    }
}
    }
}

fn collect_items(payload: &ParsedUsage, now_ms: u64) -> Vec<UsageItem> {
    let mut items = Vec::new();

    if let Some(rate_limit) = &payload.rate_limit {
        if let Some(window) = &rate_limit.primary_window {
            items.push(build_usage_item(
                "Codex",
                window,
                rate_limit.allowed,
                rate_limit.limit_reached,
                now_ms,
            ));
        }
        if let Some(window) = &rate_limit.secondary_window {
            items.push(build_usage_item(
                "Codex",
                window,
                rate_limit.allowed,
                rate_limit.limit_reached,
                now_ms,
            ));
        }
    }

    for extra in &payload.additional_rate_limits {
        let slug = additional_limit_slug(
            extra.limit_name.as_deref(),
            extra.metered_feature.as_deref(),
        );
        let display_name = match slug.as_str() {
            "spark" => "Spark".to_string(),
            "chat" => "Codex".to_string(),
            _ => extra
                .limit_name
                .as_ref()
                .map(|name| normalize_usage_label(name))
                .unwrap_or_else(|| title_case_slug(&slug)),
        };

        if let Some(rate_limit) = &extra.rate_limit {
            if let Some(window) = &rate_limit.primary_window {
                items.push(build_usage_item(
                    &display_name,
                    window,
                    rate_limit.allowed,
                    rate_limit.limit_reached,
                    now_ms,
                ));
            }
            if let Some(window) = &rate_limit.secondary_window {
            if use_progress {
                render_usage_bar(
                    &prefix,
                    &line,
                    bar_fill,
                    bar_empty,
                    filled,
                );
            } else {
                println!("{:<22} {}", prefix, line);
            }
        }
        None => {
            let mut line = String::from("[unknown usage percentage]");
            if let Some(reset_text) = &item.reset_text {
                line.push(' ');
                line.push_str(&colorize(theme.reset_color, reset_text));
            }
            if let Some(status) = status_label {
                line.push(' ');
                line.push_str(&colorize(theme.error_color, status));
            }
            if use_progress {
                println!("{: <22} {}", prefix, line);
            } else {
fn render_usage_bar(prefix: &str, line: &str, bar_fill: &str, bar_empty: &str, filled: u64) {
    let bar = ProgressBar::new(100);
    render_usage_bar_with_width(
        &bar,
        .progress_chars("█▓▒░ ");
        prefix,
        line,
        bar_fill,
        bar_empty,
        filled,
    );
}

fn render_usage_bar_with_width(
    bar: &ProgressBar,
    width: usize,
    prefix: &str,
    line: &str,
    bar_fill: &str,
    bar_empty: &str,
    filled: u64,
) {
    let template = format!(
        "{{prefix:<19}} {{bar:{width}.{bar_fill}/{bar_empty}}} {{msg}}",
        width = width,
        bar_fill = bar_fill,
        bar_empty = bar_empty
    );
    let bar_style = ProgressStyle::with_template(&template)
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("█▉▊▌ ");
    bar.set_style(bar_style);
    bar.set_prefix(prefix.to_string());
    bar.set_message(line.to_string());
    bar.set_position(filled);
    bar.abandon();
}
    bar.set_style(bar_style);
    bar.set_prefix(prefix.to_string());
    bar.set_message(line.to_string());
    bar.set_position(filled);
    bar.abandon();
}

fn normalize_usage_label(name: &str) -> String {
    let normalized = name.trim().to_lowercase();
    if normalized == "chat" {
        return "Codex".to_string();
    }
    if normalized == "spark" {
        return "Spark".to_string();
    }
    title_case_slug(&slugify(&normalized))
}

fn fit_width(value: &str, width: usize) -> String {
    let truncated: String = value.chars().take(width).collect();
    let pad = width.saturating_sub(truncated.chars().count());
    if pad == 0 {
        truncated
    } else {
        format!("{truncated}{}", " ".repeat(pad))
    }
}

fn colorize(ansi: &str, text: &str) -> String {
    format!("{ansi}{text}{ANSI_RESET}")
}

fn parse_usage_payload(payload: Value) -> ParsedUsage {
    window: &UsageWindow,
    allowed: Option<bool>,
    limit_reached: Option<bool>,
    now_ms: u64,
) -> UsageItem {
    UsageItem {
        meter: meter.to_string(),
        window_label: window_label(window.limit_window_seconds),
        used_percent: window.used_percent,
        status: usage_status(window.used_percent, allowed, limit_reached),
        reset_text: resolve_reset_text(window, now_ms),
    }
}

fn render_usage_item(item: &UsageItem, theme: &Theme, use_progress: bool) {
    let status_label = match item.status {
        UsageStatus::Ok => None,
        UsageStatus::Warning => Some("warning"),
        UsageStatus::Exhausted => Some("exhausted"),
        UsageStatus::Unknown => Some("unknown"),
    };
    let (bar_fill, bar_empty) = match item.status {
        UsageStatus::Ok => (theme.bar_ok, theme.bar_empty),
        UsageStatus::Warning => (theme.bar_warning, theme.bar_empty),
        UsageStatus::Exhausted => (theme.bar_exhausted, theme.bar_empty),
        UsageStatus::Unknown => (theme.bar_unknown, theme.bar_empty),
    };
    let meter = fit_width(&item.meter, 8);
    let window = fit_width(&item.window_label, 11);
    let prefix = format!(
        "{ANSI_BOLD}{meter_color}{meter}{ANSI_RESET} {window_color}({window}){ANSI_RESET}",
        meter_color = theme.meter_color,
        window_color = theme.window_color
    );

    match item.used_percent {
        Some(percent_raw) => {
            let percent = percent_raw.clamp(0.0, 100.0);
            let filled = (percent.round() as u64).min(100);
            let mut line = format!("{percent:>5.1}%");
            if let Some(reset_text) = &item.reset_text {
                line.push(' ');
                line.push_str(&colorize(theme.reset_color, reset_text));
            }
            if let Some(status) = status_label {
                line.push(' ');
                line.push_str(&colorize(theme.error_color, status));
            }

            if use_progress {
                let template = format!(
                    "{{prefix:<19}} {{bar:44.{bar_fill}/{bar_empty}}} {{msg}}",
                    bar_fill = bar_fill,
                    bar_empty = bar_empty
                );
                let bar_style = ProgressStyle::with_template(&template)
                    .unwrap_or_else(|_| ProgressStyle::default_bar())
                    .progress_chars("█▉▊▌ ");
                let bar = ProgressBar::new(100);
                bar.set_style(bar_style);
                bar.set_prefix(prefix);
                bar.set_message(line);
                bar.set_position(filled);
                bar.finish_with_message("");
            } else {
                println!("{:<22} {}", prefix, line);
            }
        }
        None => {
            let mut line = String::from("[unknown usage percentage]");
            if let Some(reset_text) = &item.reset_text {
                line.push(' ');
                line.push_str(&colorize(theme.reset_color, reset_text));
            }
            if let Some(status) = status_label {
                line.push(' ');
                line.push_str(&colorize(theme.error_color, status));
            }
            if use_progress {
                println!("{: <22} {}", prefix, line);
            } else {
                println!("{: <22} {}", prefix, line);
            }
        }
    }
}

fn normalize_usage_label(name: &str) -> String {
    let normalized = name.trim().to_lowercase();
    if normalized == "chat" {
        return "Codex".to_string();
    }
    if normalized == "spark" {
        return "Spark".to_string();
    }
    title_case_slug(&slugify(&normalized))
}

fn fit_width(value: &str, width: usize) -> String {
    let truncated: String = value.chars().take(width).collect();
    let pad = width.saturating_sub(truncated.chars().count());
    if pad == 0 {
        truncated
    } else {
        format!("{truncated}{}", " ".repeat(pad))
    }
}

fn colorize(ansi: &str, text: &str) -> String {
    format!("{ansi}{text}{ANSI_RESET}")
}
            let mut message = format!("{:<20} [unknown usage percentage] {}", item.label, status);
            if let Some(reset_text) = &item.reset_text {
                message.push_str(" | ");
                message.push_str(reset_text);
            }
            println!("{message}");
        }
    }
}

fn parse_usage_payload(payload: Value) -> ParsedUsage {
    let additional_rate_limits = raw_obj
        .get("additional_rate_limits")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(parse_additional_rate_limit)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let reset_credits_available = raw_obj
        .get("rate_limit_reset_credits")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("available_count"))
        .and_then(as_u64);

    ParsedUsage {
        plan_type,
        rate_limit,
        additional_rate_limits,
        reset_credits_available,
        raw: payload,
    }
}

fn parse_rate_limit(raw: &Map<String, Value>) -> Option<RateLimit> {
    let allowed = raw.get("allowed").and_then(as_bool);
    let limit_reached = raw.get("limit_reached").and_then(as_bool);

    let primary_window = raw
        .get("primary_window")
        .and_then(Value::as_object)
        .and_then(parse_usage_window);
    let secondary_window = raw
        .get("secondary_window")
        .and_then(Value::as_object)
        .and_then(parse_usage_window);

    if allowed.is_none()
        && limit_reached.is_none()
        && primary_window.is_none()
        && secondary_window.is_none()
    {
        return None;
    }

    Some(RateLimit {
        allowed,
        limit_reached,
        primary_window,
        secondary_window,
    })
}

fn parse_usage_window(raw: &Map<String, Value>) -> Option<UsageWindow> {
    let used_percent = raw.get("used_percent").and_then(as_f64);
    let limit_window_seconds = raw.get("limit_window_seconds").and_then(as_u64);
    let reset_after_seconds = raw.get("reset_after_seconds").and_then(as_u64);
    let reset_at = raw.get("reset_at").and_then(as_u64);

    if used_percent.is_none()
        && limit_window_seconds.is_none()
        && reset_after_seconds.is_none()
        && reset_at.is_none()
    {
        return None;
    }

    Some(UsageWindow {
        used_percent,
        limit_window_seconds,
        reset_after_seconds,
        reset_at,
    })
}

fn parse_additional_rate_limit(raw: &Value) -> Option<AdditionalRateLimit> {
    let obj = raw.as_object()?;
    let limit_name = obj.get("limit_name").and_then(as_string);
    let metered_feature = obj.get("metered_feature").and_then(as_string);
    let rate_limit = obj.get("rate_limit").and_then(Value::as_object).and_then(parse_rate_limit);

    if limit_name.is_none() && metered_feature.is_none() && rate_limit.is_none() {
    let source = source
        .strip_prefix("codex_")
        .or_else(|| source.strip_prefix("codex-"))
        .unwrap_or(&source)
        .to_string();
    let slug = slugify(&source);
    if slug.is_empty() {
        "extra".to_string()
    } else {
    if slug.is_empty() {
        "extra".to_string()
    } else {
        slug
    }
}

fn window_label(seconds: Option<u64>) -> String {
}

fn usage_status(
fn additional_limit_slug(limit_name: Option<&str>, metered_feature: Option<&str>) -> String {
    let probe = format!(
        "{} {}",
        limit_name.unwrap_or(""),
        metered_feature.unwrap_or("")
    )
    .to_lowercase();
    if probe.contains("spark") || probe.contains("bengalfox") {
        return "spark".to_string();
    }

    let source = source
        .strip_prefix("codex_")
        .or_else(|| source.strip_prefix("codex-"))
        .unwrap_or(&source)
        .to_string();
    let slug = slugify(&source);
    if slug.is_empty() {
        "extra".to_string()
    } else {
        slug
fn human_duration(total_seconds: u64) -> String {
    use super::{
        additional_limit_slug, collect_banked_resets, format_account_plan_line, human_duration,
        now_millis, persist_auth, render_usage_bar_with_width, AuthRecord, ParsedUsage,
        AdditionalRateLimit, BUILTIN_THEMES, RateLimit, UsageWindow,
    };
    use indicatif::{ProgressBar, ProgressDrawTarget, TermLike};
    let hours = (total_seconds % 86_400) / 3_600;
    let mins = (total_seconds % 3_600) / 60;
    let secs = total_seconds % 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    use std::env;
    use std::fmt;
    use std::fs;
    use std::io;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct RecordingTerm {
        output: Arc<Mutex<String>>,
    }

    impl RecordingTerm {
#[cfg(test)]
    use super::{
        additional_limit_slug,
        human_duration,
        now_millis,
        persist_auth,
        render_usage_bar_with_width,
        AuthRecord,
    };
    use indicatif::{ProgressBar, ProgressDrawTarget, TermLike};
    use indicatif::{ProgressBar, ProgressDrawTarget, TermLike};
        fn contents(&self) -> String {
            self.output.lock().unwrap().clone()
        }
    }

    impl fmt::Debug for RecordingTerm {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("RecordingTerm").finish()
        }
    }

    impl TermLike for RecordingTerm {
        fn width(&self) -> u16 {
            120
        }

        fn height(&self) -> u16 {
            24
        }

        fn move_cursor_up(&self, _n: usize) -> io::Result<()> {
            Ok(())
        }

        fn move_cursor_down(&self, _n: usize) -> io::Result<()> {
            Ok(())
        }

        fn move_cursor_right(&self, _n: usize) -> io::Result<()> {
            Ok(())
        }

        fn move_cursor_left(&self, _n: usize) -> io::Result<()> {
            Ok(())
        }

        fn write_line(&self, s: &str) -> io::Result<()> {
            let mut output = self.output.lock().unwrap();
            output.push_str(s);
            output.push('\n');
            Ok(())
        }

        fn write_str(&self, s: &str) -> io::Result<()> {
            let mut output = self.output.lock().unwrap();
    #[test]
    #[test]
    fn progress_bar_abandon_keeps_partial_fill_and_message() {
        let term = RecordingTerm::new();
        let bar = ProgressBar::with_draw_target(
            Some(100),
            ProgressDrawTarget::term_like(Box::new(term.clone())),
        );
        render_usage_bar_with_width(&bar, 10, "Codex (7 days)", " 42.0%", "green", "black", 42);

        assert_eq!(bar.position(), 42);
        assert_eq!(bar.message(), " 42.0%");

        let output = term.contents();
        assert!(output.contains("Codex (7 days)"));
        assert!(output.contains("42.0%"));

        let last_rendered_frame = output
            .split("\r\x1b[2K")
            .filter(|frame| !frame.is_empty())
            .filter(|frame| frame.contains('█'))
            .last()
            .expect("progress bar frame should be captured");
        let bar_start = last_rendered_frame
            .find('█')
            .expect("bar should render a filled segment");
        let msg_start = last_rendered_frame
            .rfind("42.0%")
            .expect("message should be rendered with progress bar");
        assert!(bar_start < msg_start, "bar should render before message");
        let bar_segment = &last_rendered_frame[bar_start..msg_start];

        let filled_cells = bar_segment
            .chars()
        let filled_cells = bar_segment
            .chars()
            .filter(|c| matches!(c, '█' | '▉' | '▊' | '▌' | '▓' | '▒' | '░'))
            .count();
        let empty_cells = bar_segment.chars().filter(|c| *c == ' ').count();

        assert!(filled_cells > 0, "partial bar should include fill chars");
        assert!(filled_cells < 10, "bar should not be fully filled at 42%");
        assert!(empty_cells > 0, "partial bar should include unfilled width");
    }
    #[test]
    fn format_account_plan_line_hides_account_id_and_keeps_email_with_plan() {
        let auth = AuthRecord {
            access_token: "x".to_string(),
            id_token: None,
            refresh_token: None,
            account_id: Some("a1b2c3d4".to_string()),
            email: Some("john.doe@gmail.com".to_string()),
            oauth_client_id: None,
        };
        let usage = ParsedUsage {
            plan_type: Some("prolite".to_string()),
            rate_limit: None,
            additional_rate_limits: vec![],
            reset_credits_available: None,
            raw: serde_json::Value::Null,
        };

        let line = format_account_plan_line(&auth, &usage, &BUILTIN_THEMES[0]);

        assert!(line.contains("john.doe@gmail.com"));
        assert!(line.contains("(prolite)"));
        assert!(!line.contains("a1b2c3d4"));
    }

    #[test]
    fn collect_banked_resets_collects_known_sources_with_expiration() {
        let usage = ParsedUsage {
            plan_type: Some("prolite".to_string()),
            rate_limit: Some(RateLimit {
                allowed: Some(true),
                limit_reached: Some(false),
                primary_window: Some(UsageWindow {
                    used_percent: Some(42.0),
                    limit_window_seconds: Some(86_400),
                    reset_after_seconds: Some(3_600),
                    reset_at: None,
                }),
                secondary_window: None,
            }),
            additional_rate_limits: vec![AdditionalRateLimit {
                limit_name: Some("codex_spark".to_string()),
                metered_feature: None,
                rate_limit: Some(RateLimit {
                    allowed: Some(true),
                    limit_reached: Some(false),
                    primary_window: Some(UsageWindow {
                        used_percent: Some(10.0),
                        limit_window_seconds: Some(30 * 60),
                        reset_after_seconds: Some(900),
                        reset_at: None,
                    }),
                    secondary_window: None,
                }),
            }],
            reset_credits_available: None,
            raw: serde_json::Value::Null,
        };

        let resets = collect_banked_resets(&usage, 0);
        assert_eq!(resets.len(), 2);
        assert!(
            resets
                .iter()
                .any(|item| item.source == "Codex (1 day)" && item.expires_in == "expires in 1h")
        );
        assert!(
            resets
                .iter()
                .any(|item| item.source == "Spark (30 minutes)" && item.expires_in == "expires in 15m")
        );
    }

    #[test]
    fn human_duration_keeps_day_hour_minute_for_90060() {

        assert!(filled_cells > 0, "partial bar should include fill chars");
        assert!(filled_cells < 10, "bar should not be fully filled at 42%");
        assert!(empty_cells > 0, "partial bar should include unfilled width");
    }
        }

        fn flush(&self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn progress_bar_abandon_keeps_partial_fill_and_message() {
        let term = RecordingTerm::new();
        let bar = ProgressBar::with_draw_target(
            Some(100),
            ProgressDrawTarget::term_like(Box::new(term.clone())),
        );
        render_usage_bar_with_width(
    #[test]
    fn progress_bar_abandon_keeps_partial_fill_and_message() {
        let term = RecordingTerm::new();
        let bar = ProgressBar::with_draw_target(
            Some(100),
            ProgressDrawTarget::term_like(Box::new(term.clone())),
        );
        render_usage_bar_with_width(&bar, 10, "Codex (7 days)", " 42.0%", "green", "black", 42);
    #[test]
    fn progress_bar_abandon_keeps_partial_fill_and_message() {
        let term = RecordingTerm::new();
        let bar = ProgressBar::with_draw_target(
            Some(100),
            ProgressDrawTarget::term_like(Box::new(term.clone())),
        );
        render_usage_bar_with_width(
            &bar,
            10,
            "Codex (7 days)",
            " 42.0%",
            "green",
            "black",
            42,
        );
        let output = term.contents();
        assert!(output.contains("Codex (7 days)"));
        assert!(output.contains("42.0%"));

        let last_rendered_frame = output
            .split("\r\x1b[2K")
            .filter(|frame| !frame.is_empty())
            .filter(|frame| frame.contains('█'))
            .last()
            .expect("progress bar frame should be captured");
        let bar_start = last_rendered_frame
            .find('█')
            .expect("bar should render a filled segment");
        let msg_start = last_rendered_frame
            .rfind("42.0%")
            .expect("message should be rendered with progress bar");
        assert!(bar_start < msg_start, "bar should render before message");
        let bar_segment = &last_rendered_frame[bar_start..msg_start];

        let filled_cells = bar_segment
            .chars()
            .filter(|c| matches!(c, '█' | '▉' | '▊' | '▌'))
            .count();
        let empty_cells = bar_segment
            .chars()
            .filter(|c| *c == ' ')
            .count();

        assert!(filled_cells > 0, "partial bar should include fill chars");
        assert!(filled_cells < 10, "bar should not be fully filled at 42%");
        assert!(empty_cells > 0, "partial bar should include unfilled width");
        assert!(filled_cells < 10, "bar should not be fully filled at 42%");
        assert!(empty_cells > 0, "partial bar should include unfilled width");
    }
            "extra-feature"
        );
        assert_eq!(additional_limit_slug(Some("codex-"), None), "extra");
        assert_eq!(additional_limit_slug(None, None), "extra");
    }

    #[test]
    fn additional_limit_slug_prefers_spark() {
        assert_eq!(additional_limit_slug(Some("codex_spark"), None), "spark");
    }

    #[test]
    fn persist_auth_preserves_unrelated_fields_and_replaces_temp_credentials() {
        let mut base = env::temp_dir();
        base.push(format!(
            "codex-usage-auth-test-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&base).expect("create temp dir");
        let auth_path = base.join("auth.json");
        fs::write(
            &auth_path,
            r#"{"plan_type":"legacy","tokens":{"access_token":"old","id_token":"old_id","refresh_token":"old_ref","account_id":"old_acct","keep":"keep-me"}}"#,
        )
        .expect("seed auth file");

        let auth = AuthRecord {
            access_token: "new_access".to_string(),
            id_token: Some("new_id".to_string()),
            refresh_token: None,
            account_id: Some("new_acct".to_string()),
            email: Some("x@example.com".to_string()),
            oauth_client_id: Some("id".to_string()),
        };

        persist_auth(&auth_path, &auth).expect("persist_auth");

        let updated = fs::read_to_string(&auth_path).expect("read updated auth");
        let updated: serde_json::Value = serde_json::from_str(&updated).expect("parse updated auth");

        assert_eq!(updated["plan_type"], "legacy");
        let tokens = updated.get("tokens").expect("tokens exists");
        assert_eq!(tokens["access_token"], "new_access");
        assert_eq!(tokens["id_token"], "new_id");
        assert_eq!(tokens["account_id"], "new_acct");
        assert!(tokens.get("refresh_token").is_none());
        assert_eq!(tokens["keep"], "keep-me");

        let mut has_temp = false;
        for entry in fs::read_dir(&base).expect("scan temp dir") {
            let entry = entry.expect("entry");
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".codex-auth-") && name.ends_with(".tmp") {
                has_temp = true;
                break;
            }
        }
        assert!(!has_temp);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&auth_path).expect("metadata").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        fs::remove_dir_all(base).expect("remove temp dir");
    }
}

fn parse_jwt_claim(token: &str, top: &str, inner: &str) -> Option<String> {
        .expect("seed auth file");

        let auth = AuthRecord {
            access_token: "new_access".to_string(),
            id_token: Some("new_id".to_string()),
            refresh_token: None,
            account_id: Some("new_acct".to_string()),
            email: Some("x@example.com".to_string()),
            oauth_client_id: Some("id".to_string()),
        };

        persist_auth(&auth_path, &auth).expect("persist_auth");

        let updated = fs::read_to_string(&auth_path).expect("read updated auth");
        let updated: serde_json::Value =
            serde_json::from_str(&updated).expect("parse updated auth");

        assert_eq!(updated["plan_type"], "legacy");
        let tokens = updated.get("tokens").expect("tokens exists");
        assert_eq!(tokens["access_token"], "new_access");
        assert_eq!(tokens["id_token"], "new_id");
        assert_eq!(tokens["account_id"], "new_acct");
        assert!(tokens.get("refresh_token").is_none());
        assert_eq!(tokens["keep"], "keep-me");

        let mut has_temp = false;
        for entry in fs::read_dir(&base).expect("scan temp dir") {
            let entry = entry.expect("entry");
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".codex-auth-") && name.ends_with(".tmp") {
                has_temp = true;
                break;
            }
        }
        assert!(!has_temp);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&auth_path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        fs::remove_dir_all(base).expect("remove temp dir");
    }
}

fn parse_jwt_claim(token: &str, top: &str, inner: &str) -> Option<String> {
            slug.push(' ');
        }
    }

    slug.trim_matches('-').to_string()
}
                if minutes == 1 { "" } else { "s" }
            )
        }
    } else {
        "unknown".to_string()
    }
}
        return "spark".to_string();
    }

    let source = metered_feature
        .or(limit_name)
        .unwrap_or("extra")
        .to_lowercase();
    let slug = slugify(&source);
    if slug.is_empty() { "extra".to_string() } else { slug }
}

fn slugify(input: &str) -> String {
    let normalized = input.trim().to_lowercase();
    let no_prefix = normalized
        .strip_prefix("codex-")
        .or_else(|| normalized.strip_prefix("codex_"))
        .unwrap_or(&normalized);

    let mut slug = String::new();
    let mut last_dash = false;

    for ch in no_prefix.chars() {
        let normalized_ch = match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' => ch.to_ascii_lowercase(),
            ' ' | '_' | '-' => '-',
            _ => {
                continue;
            }
        };

        if normalized_ch == '-' {
            if last_dash {
                continue;
            }
            last_dash = true;
            slug.push('-');
        } else {
            last_dash = false;
            slug.push(normalized_ch);
        }
    }

    slug.trim_matches('-').to_string()
}
}
    use std::fs;
    use std::path::Path;

    #[test]
    fn human_duration_keeps_day_hour_minute_for_90060() {
        assert_eq!(human_duration(90060), "1d 1h 1m");
    }

    #[test]
    fn human_duration_keeps_day_minute_second_for_86461() {
        assert_eq!(human_duration(86461), "1d 1m 1s");
    }

    #[test]
    fn human_duration_keeps_zero_minutes_when_necessary() {
        assert_eq!(human_duration(90000), "1d 1h");
    }

    #[test]
    fn persist_auth_preserves_unrelated_fields_and_replaces_temp_credentials() {
        let mut base = env::temp_dir();
        base.push(format!(
            "codex-usage-auth-test-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&base).expect("create temp dir");
        let auth_path = base.join("auth.json");
        fs::write(
            &auth_path,
            r#"{"plan_type":"legacy","tokens":{"access_token":"old","id_token":"old_id","refresh_token":"old_ref","account_id":"old_acct","keep":"keep-me"}}"#,
        )
        .expect("seed auth file");

        let auth = AuthRecord {
            access_token: "new_access".to_string(),
            id_token: Some("new_id".to_string()),
            refresh_token: None,
            account_id: Some("new_acct".to_string()),
            email: Some("x@example.com".to_string()),
            oauth_client_id: Some("id".to_string()),
        };

        persist_auth(&auth_path, &auth).expect("persist_auth");

        let updated = fs::read_to_string(&auth_path).expect("read updated auth");
        let updated: serde_json::Value = serde_json::from_str(&updated).expect("parse updated auth");

        assert_eq!(updated["plan_type"], "legacy");
        let tokens = updated.get("tokens").expect("tokens exists");
        assert_eq!(tokens["access_token"], "new_access");
        assert_eq!(tokens["id_token"], "new_id");
        assert_eq!(tokens["account_id"], "new_acct");
        assert!(tokens.get("refresh_token").is_none());
    let mins = (total_seconds % 3_600) / 60;
    let secs = total_seconds % 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if mins > 0 {
        parts.push(format!("{mins}m"));
    }
    if secs > 0 {
        parts.push(format!("{secs}s"));
    }

    if parts.is_empty() {
        return "0s".to_string();
    }

    let shown_parts = parts.into_iter().take(3).collect::<Vec<_>>();
    shown_parts.join(" ")
}
    use std::fs;

    #[test]
    fn human_duration_keeps_day_hour_minute_for_90060() {
    use super::{additional_limit_slug, human_duration, now_millis, persist_auth, AuthRecord};
    use std::env;
    use std::fs;
    #[test]
    fn human_duration_keeps_day_minute_second_for_86461() {
        assert_eq!(human_duration(86461), "1d 1m 1s");
    }

    #[test]
    fn human_duration_keeps_zero_minutes_when_necessary() {
        assert_eq!(human_duration(90000), "1d 1h");
    }

    #[test]
    fn persist_auth_preserves_unrelated_fields_and_replaces_temp_credentials() {
    fn human_duration_keeps_zero_minutes_when_necessary() {
        assert_eq!(human_duration(90000), "1d 1h");
    }

    #[test]
    fn additional_limit_slug_removes_codex_prefix_and_defaults_to_extra() {
        assert_eq!(
            additional_limit_slug(Some("codex_extra-feature"), None),
            "extra-feature"
        );
        assert_eq!(
            additional_limit_slug(Some("codex-extra-feature"), None),
            "extra-feature"
        );
        assert_eq!(additional_limit_slug(Some("codex-"), None), "extra");
        assert_eq!(additional_limit_slug(None, None), "extra");
    }

    #[test]
    fn additional_limit_slug_prefers_spark() {
        assert_eq!(additional_limit_slug(Some("codex_spark"), None), "spark");
    }
    #[test]
    fn persist_auth_preserves_unrelated_fields_and_replaces_temp_credentials() {
        let mut base = env::temp_dir();
        base.push(format!(
            "codex-usage-auth-test-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&base).expect("create temp dir");
        let auth_path = base.join("auth.json");
        fs::write(
            &auth_path,
            r#"{"plan_type":"legacy","tokens":{"access_token":"old","id_token":"old_id","refresh_token":"old_ref","account_id":"old_acct","keep":"keep-me"}}"#,
        )
        .expect("seed auth file");

        let auth = AuthRecord {
            access_token: "new_access".to_string(),
            id_token: Some("new_id".to_string()),
            refresh_token: None,
            account_id: Some("new_acct".to_string()),
            email: Some("x@example.com".to_string()),
            oauth_client_id: Some("id".to_string()),
        };
        };

        persist_auth(&auth_path, &auth).expect("persist_auth");

        let updated = fs::read_to_string(&auth_path).expect("read updated auth");
        let updated: serde_json::Value = serde_json::from_str(&updated).expect("parse updated auth");

        assert_eq!(updated["plan_type"], "legacy");
        let tokens = updated.get("tokens").expect("tokens exists");
        assert_eq!(tokens["access_token"], "new_access");
        assert_eq!(tokens["id_token"], "new_id");
        assert_eq!(tokens["account_id"], "new_acct");
        assert!(tokens.get("refresh_token").is_none());
        assert_eq!(tokens["keep"], "keep-me");

        let mut has_temp = false;
        for entry in fs::read_dir(&base).expect("scan temp dir") {
            let entry = entry.expect("entry");
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".codex-auth-") && name.ends_with(".tmp") {
                has_temp = true;
                break;
            }
        }
        assert!(!has_temp);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&auth_path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        fs::remove_dir_all(base).expect("remove temp dir");
    }
}
        .collect::<Vec<_>>()
        .join(" ")
}

fn resolve_reset_text(window: &UsageWindow, now_ms: u64) -> Option<String> {
    let reset_ms = resolve_reset_time(window, now_ms)?;
    let diff_secs = (reset_ms.saturating_sub(now_ms) / 1000).clamp(0, u64::MAX);
    Some(format!("resets in {}", human_duration(diff_secs)))
}

fn resolve_reset_time(window: &UsageWindow, now_ms: u64) -> Option<u64> {
    if let Some(raw_reset_at) = window.reset_at {
        let ms = if raw_reset_at > 1_000_000_000_000 {
            raw_reset_at
        } else {
            raw_reset_at.saturating_mul(1000)
        };
        return Some(ms);
    }
    window.reset_after_seconds
        .map(|after_secs| now_ms.saturating_add(after_secs.saturating_mul(1000)))
}

fn human_duration(total_seconds: u64) -> String {
    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let mins = (total_seconds % 3_600) / 60;
    let secs = total_seconds % 60;

    match (days, hours, mins, secs) {
        (d, 0, 0, s) if d == 0 && s > 0 => format!("{s}s"),
        (d, 0, m, 0) if d == 0 && m > 0 => format!("{m}m"),
        (d, h, 0, 0) if d == 0 && h > 0 => format!("{h}h"),
        (d, 0, 0, 0) if d == 0 => "0s".to_string(),
        (d, h, m, 0) if d > 0 => format!("{d}d {h}h"),
        (d, h, m, s) if d > 0 && h > 0 && m > 0 => format!("{d}d {h}h {m}m"),
        (0, h, m, s) if h > 0 && m > 0 => format!("{h}h {m}m {s}s"),
        (d, h, 0, s) if d > 0 => format!("{d}d {h}h {s}s"),
        (0, h, 0, s) if h > 0 => format!("{h}h {s}s"),
        (0, 0, m, s) => format!("{m}m {s}s"),
        _ => format!("{}s", total_seconds),
fn human_duration(total_seconds: u64) -> String {
    if total_seconds == 0 {
        return "0s".to_string();
    }

    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let mins = (total_seconds % 3_600) / 60;
    let secs = total_seconds % 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if mins > 0 {
        parts.push(format!("{mins}m"));
    }
    if secs > 0 {
        parts.push(format!("{secs}s"));
    }

    if parts.is_empty() {
        return "0s".to_string();
    }

    let shown_parts = parts.into_iter().take(3).collect::<Vec<_>>();
    shown_parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::human_duration;

    #[test]
    fn human_duration_keeps_day_hour_minute_for_90060() {
        assert_eq!(human_duration(90060), "1d 1h 1m");
    }

    #[test]
    fn human_duration_keeps_day_minute_second_for_86461() {
        assert_eq!(human_duration(86461), "1d 1m 1s");
    }

    #[test]
    fn human_duration_keeps_zero_minutes_when_necessary() {
        assert_eq!(human_duration(90000), "1d 1h");
    }
}

fn parse_jwt_claim(token: &str, top: &str, inner: &str) -> Option<String> {
        (0, h, 0, s) if h > 0 => format!("{h}h {s}s"),
        (0, 0, m, s) => format!("{m}m {s}s"),
    let bytes = URL_SAFE.decode(payload).ok()?;
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn as_string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}
    let mut payload = payload.to_string();
    while !payload.len().is_multiple_of(4) {
        payload.push('=');
    }
    let bytes = URL_SAFE.decode(&payload).ok()?;
    serde_json::from_slice(&bytes).ok()
    if let Some(v) = value.as_f64() {
        return Some(v);
    }
    if let Some(v) = value.as_str() {
        return v.parse::<f64>().ok();
    }
    None
}

fn as_u64(value: &Value) -> Option<u64> {
    let f = as_f64(value)?;
    if !f.is_finite() || f < 0.0 {
        return None;
    }
    Some(f as u64)
}
