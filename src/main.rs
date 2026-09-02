use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use clap::Parser;
use config::{Config, Environment, File as ConfigFile};
use indicatif::{ProgressBar, ProgressStyle};
use platform_dirs::AppDirs;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde_json::{Map, Value};
use std::fs;
use std::io::{ErrorKind, IsTerminal, Write};
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

#[derive(Clone, Copy)]
struct Theme {
    name: &'static str,
    /// indicatif-style color name used for status word text
    bar_warning: &'static str,
    bar_exhausted: &'static str,
    bar_unknown: &'static str,
    /// truecolor ANSI: theme primary used for the bar fill
    bar_primary: &'static str,
    /// truecolor ANSI: muted same-hue secondary for the unfilled region
    bar_background: &'static str,
    meter_color: &'static str,
    window_color: &'static str,
    reset_color: &'static str,
}

const BUILTIN_THEMES: &[Theme] = &[
    Theme {
        name: "default",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "blue",
        meter_color: "\x1b[38;5;39m",
        window_color: "\x1b[38;5;243m",
        reset_color: "\x1b[38;5;244m",
        bar_primary: "\x1b[38;2;63;185;80m",
        bar_background: "\x1b[38;2;41;92;50m",
    },
    Theme {
        name: "solarized-dark",
        bar_warning: "bright_yellow",
        bar_exhausted: "red",
        bar_unknown: "bright_blue",
        meter_color: "\x1b[38;5;75m",
        window_color: "\x1b[38;5;144m",
        reset_color: "\x1b[38;5;245m",
        bar_primary: "\x1b[38;2;42;161;152m",
        bar_background: "\x1b[38;2;24;88;84m",
    },
    Theme {
        name: "solarized-light",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "cyan",
        meter_color: "\x1b[38;5;33m",
        window_color: "\x1b[38;5;101m",
        reset_color: "\x1b[38;5;244m",
        bar_primary: "\x1b[38;2;38;139;210m",
        bar_background: "\x1b[38;2;190;213;233m",
    },
    Theme {
        name: "monokai",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "magenta",
        meter_color: "\x1b[38;5;77m",
        window_color: "\x1b[38;5;180m",
        reset_color: "\x1b[38;5;244m",
        bar_primary: "\x1b[38;2;166;226;46m",
        bar_background: "\x1b[38;2;85;102;34m",
    },
    Theme {
        name: "molokai",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "magenta",
        meter_color: "\x1b[38;5;148m",
        window_color: "\x1b[38;5;244m",
        reset_color: "\x1b[38;5;245m",
        bar_primary: "\x1b[38;2;184;230;62m",
        bar_background: "\x1b[38;2;92;104;40m",
    },
    Theme {
        name: "dracula",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "bright_cyan",
        meter_color: "\x1b[38;5;141m",
        window_color: "\x1b[38;5;103m",
        reset_color: "\x1b[38;5;243m",
        bar_primary: "\x1b[38;2;189;147;249m",
        bar_background: "\x1b[38;2;88;70;120m",
    },
    Theme {
        name: "gruvbox-dark",
        bar_warning: "bright_yellow",
        bar_exhausted: "bright_red",
        bar_unknown: "cyan",
        meter_color: "\x1b[38;5;214m",
        window_color: "\x1b[38;5;180m",
        reset_color: "\x1b[38;5;244m",
        bar_primary: "\x1b[38;2;250;189;47m",
        bar_background: "\x1b[38;2;110;88;24m",
    },
    Theme {
        name: "gruvbox-light",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "magenta",
        meter_color: "\x1b[38;5;66m",
        window_color: "\x1b[38;5;59m",
        reset_color: "\x1b[38;5;244m",
        bar_primary: "\x1b[38;2;7;102;120m",
        bar_background: "\x1b[38;2;180;205;211m",
    },
    Theme {
        name: "one-dark",
        bar_warning: "bright_yellow",
        bar_exhausted: "bright_red",
        bar_unknown: "bright_cyan",
        meter_color: "\x1b[38;5;114m",
        window_color: "\x1b[38;5;181m",
        reset_color: "\x1b[38;5;245m",
        bar_primary: "\x1b[38;2;152;195;121m",
        bar_background: "\x1b[38;2;70;95;60m",
    },
    Theme {
        name: "one-light",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "blue",
        meter_color: "\x1b[38;5;28m",
        window_color: "\x1b[38;5;101m",
        reset_color: "\x1b[38;5;244m",
        bar_primary: "\x1b[38;2;80;161;79m",
        bar_background: "\x1b[38;2;196;220;196m",
    },
    Theme {
        name: "nord",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "blue",
        meter_color: "\x1b[38;5;74m",
        window_color: "\x1b[38;5;104m",
        reset_color: "\x1b[38;5;244m",
        bar_primary: "\x1b[38;2;136;192;208m",
        bar_background: "\x1b[38;2;62;90;99m",
    },
    Theme {
        name: "github-dark",
        bar_warning: "yellow",
        bar_exhausted: "bright_red",
        bar_unknown: "blue",
        meter_color: "\x1b[38;5;47m",
        window_color: "\x1b[38;5;249m",
        reset_color: "\x1b[38;5;240m",
        bar_primary: "\x1b[38;2;63;185;80m",
        bar_background: "\x1b[38;2;40;86;48m",
    },
    Theme {
        name: "github-light",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "cyan",
        meter_color: "\x1b[38;5;24m",
        window_color: "\x1b[38;5;244m",
        reset_color: "\x1b[38;5;244m",
        bar_primary: "\x1b[38;2;9;105;218m",
        bar_background: "\x1b[38;2;200;216;240m",
    },
    Theme {
        name: "nord-dark",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "magenta",
        meter_color: "\x1b[38;5;117m",
        window_color: "\x1b[38;5;152m",
        reset_color: "\x1b[38;5;245m",
        bar_primary: "\x1b[38;2;94;129;172m",
        bar_background: "\x1b[38;2;52;72;97m",
    },
    Theme {
        name: "catppuccin-mocha",
        bar_warning: "yellow",
        bar_exhausted: "bright_red",
        bar_unknown: "cyan",
        meter_color: "\x1b[38;5;141m",
        window_color: "\x1b[38;5;103m",
        reset_color: "\x1b[38;5;243m",
        bar_primary: "\x1b[38;2;203;166;247m",
        bar_background: "\x1b[38;2;90;74;114m",
    },
    Theme {
        name: "tokyo-night",
        bar_warning: "yellow",
        bar_exhausted: "red",
        bar_unknown: "magenta",
        meter_color: "\x1b[38;5;110m",
        window_color: "\x1b[38;5;246m",
        reset_color: "\x1b[38;5;242m",
        bar_primary: "\x1b[38;2;122;162;247m",
        bar_background: "\x1b[38;2;58;78;116m",
    },
];

fn config_file_path() -> Option<PathBuf> {
    AppDirs::new(Some("codex-usage"), true).map(|app_dirs| app_dirs.config_dir.join("config.toml"))
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
    cfg.get_string("theme").map_err(|e| e.to_string())
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
#[derive(Clone)]
struct AuthRecord {
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
            || self.email != prior.email
    }

    fn default_oauth_client_id(&self) -> &str {
        self.oauth_client_id
            .as_deref()
            .unwrap_or("app_EMoamEEZ73f0CkXaXp7hrann")
    }
}

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

#[derive(Clone)]
struct ParsedUsage {
    plan_type: Option<String>,
    rate_limit: Option<RateLimit>,
    additional_rate_limits: Vec<AdditionalRateLimit>,
    reset_credits_available: Option<u64>,
    reset_credits: Option<Vec<ResetCredit>>,
    raw: Value,
}

#[derive(Clone)]
struct UsageItem {
    meter: String,
    window_label: String,
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

fn fetch_usage(cli: &Cli, auth: &mut AuthRecord, use_progress: bool) -> AppResult<ParsedUsage> {
    let base = normalize_base_url(&cli.base_url);
    let url = format!("{}/wham/usage", base);

    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;

    let use_progress = use_progress && output_is_interactive();
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
    };

    let request = |auth: &AuthRecord| -> AppResult<reqwest::blocking::Response> {
        client
            .get(&url)
            .headers(build_headers(auth)?)
            .send()
            .map_err(|e| format!("usage request failed: {e}"))
    };

    let first_response = request(auth)?;
    let first_status = first_response.status();

    if first_status.is_success() {
        let payload = first_response.json().map_err(|e| e.to_string())?;
        let mut usage = parse_usage_payload(payload);
        if usage.reset_credits_available.unwrap_or(0) > 0 {
            if let Some(pb) = &spinner {
                pb.set_message("Fetching banked reset credits…");
            }
            usage.reset_credits = fetch_reset_credits(&client, &base, auth);
        }
        if let Some(pb) = &spinner {
            pb.finish_and_clear();
        }
        return Ok(usage);
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
                    let payload = second_response.json().map_err(|e| e.to_string())?;
                    let mut usage = parse_usage_payload(payload);
                    if usage.reset_credits_available.unwrap_or(0) > 0 {
                        if let Some(pb) = &spinner {
                            pb.set_message("Fetching banked reset credits…");
                        }
                        usage.reset_credits = fetch_reset_credits(&client, &base, auth);
                    }
                    if let Some(pb) = &spinner {
                        pb.finish_and_clear();
                    }
                    return Ok(usage);
                }
                let second_status = second_response.status();
                let second_body = second_response
                    .text()
                    .unwrap_or_else(|_| "unable to read response body".to_string());
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
    Err(format!(
        "usage endpoint returned HTTP {}: {}",
        first_status,
        trim_body(&first_body)
    ))
}

struct TempAuthFile {
    path: PathBuf,
    remove_on_drop: bool,
}

fn build_headers(auth: &AuthRecord) -> Result<HeaderMap, String> {
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
    Ok(headers)
}

fn fetch_reset_credits(client: &Client, base: &str, auth: &AuthRecord) -> Option<Vec<ResetCredit>> {
    let url = format!("{base}/wham/rate-limit-reset-credits");
    let response = client
        .get(&url)
        .headers(build_headers(auth).ok()?)
        .timeout(Duration::from_secs(10))
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload: Value = response.json().ok()?;
    let credits = payload.get("credits")?.as_array()?;
    Some(
        credits
            .iter()
            .filter_map(|credit| {
                let obj = credit.as_object()?;
                Some(ResetCredit {
                    title: obj
                        .get("title")
                        .and_then(as_string)
                        .filter(|title| !title.trim().is_empty()),
                    expires_at_ms: obj.get("expires_at").and_then(parse_timestamp_to_ms),
                })
            })
            .collect(),
    )
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

fn sync_directory_for(path: &Path) -> AppResult<()> {
    let dir = fs::File::open(path).map_err(|e| e.to_string())?;
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

    file.write_all(formatted.as_bytes()).map_err(|e| {
        format!(
            "failed writing temporary auth file {}: {e}",
            guard.path().display()
        )
    })?;
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
        ])
        .send()
        .map_err(|e| format!("refresh request failed: {e}"))?;

    let status = response.status();
    let body = response
        .text()
        .unwrap_or_else(|_| "unable to read response body".to_string());
    if !status.is_success() {
        return Err(format!(
            "refresh request returned HTTP {}: {}",
            status,
            trim_body(&body)
        ));
    }

    let token_payload: Value = serde_json::from_str(&body).map_err(|e| {
        format!(
            "failed to parse refresh response JSON: {}: {}",
            e,
            trim_body(&body)
        )
    })?;

    let refreshed_access_token = token_payload
        .get("access_token")
        .and_then(as_string)
        .ok_or_else(|| "refresh response missing access_token".to_string())?;

    let refreshed_id_token = token_payload.get("id_token").and_then(as_string);
    let refreshed_refresh_token = token_payload.get("refresh_token").and_then(as_string);
    auth.access_token = refreshed_access_token;
    if refreshed_id_token.is_some() {
        auth.id_token = refreshed_id_token;
    }
    if refreshed_refresh_token.is_some() {
        auth.refresh_token = refreshed_refresh_token;
    }

    if let Some(id_token) = auth.id_token.as_deref() {
        auth.oauth_client_id = parse_jwt_aud(id_token).or_else(|| auth.oauth_client_id.clone());
        auth.account_id = parse_jwt_claim(id_token, JWT_AUTH_CLAIM, "chatgpt_account_id")
            .or_else(|| parse_jwt_claim(&auth.access_token, JWT_AUTH_CLAIM, "chatgpt_account_id"))
            .or_else(|| auth.account_id.clone());
        if let Some(email) = parse_jwt_claim(id_token, JWT_PROFILE_CLAIM, "email") {
            auth.email = Some(email);
        }
    } else if let Some(parsed) = parse_jwt_claim(&auth.access_token, JWT_PROFILE_CLAIM, "email") {
        auth.email = Some(parsed);
    }

    Ok(())
}

fn trim_body(body: &str) -> String {
    const LIMIT: usize = 1_000;
    if body.len() > LIMIT {
        let mut trimmed = body.chars().take(LIMIT).collect::<String>();
        trimmed.push_str("…");
        trimmed
    } else {
        body.to_string()
    }
}

fn parse_jwt_aud(token: &str) -> Option<String> {
    let claims = parse_jwt(token)?;
    if let Some(aud) = claims.get("aud") {
        if let Some(value) = aud.as_str() {
            return Some(value.to_string());
        }

        if let Some(values) = aud.as_array() {
            for value in values {
                if let Some(value) = value.as_str() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

#[derive(Clone)]
struct ResetCredit {
    title: Option<String>,
    expires_at_ms: Option<u64>,
}

#[derive(Clone, PartialEq, Eq)]
struct BankReset {
    source: String,
    expires_in: String,
}

fn output_is_interactive() -> bool {
    std::io::stdout().is_terminal() && std::io::stderr().is_terminal()
}

fn print_usage_report(usage: &ParsedUsage, auth: &AuthRecord, theme: &Theme, use_progress: bool) {
    let use_progress = use_progress && output_is_interactive();
    println!("{}", colorize(theme.meter_color, "Codex plan usage"));
    println!("{}", colorize(theme.window_color, "================"));

    println!("{}", format_account_plan_line(auth, usage, theme));
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

    print_banked_resets(&collect_banked_resets(usage, now_ms), theme, use_progress);
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

    match &payload.reset_credits {
        Some(credits) => {
            for credit in credits {
                let label = credit
                    .title
                    .clone()
                    .unwrap_or_else(|| "Reset credit".to_string());
                let expires_in = match credit.expires_at_ms {
                    Some(expires_ms) => {
                        let diff_secs =
                            (expires_ms.saturating_sub(now_ms) / 1000).clamp(0, u64::MAX);
                        format!("expires in {}", human_duration(diff_secs))
                    }
                    None => "no expiry reported".to_string(),
                };
                push_banked_reset(&mut resets, label, Some(expires_in));
            }
        }
        None => {
            if let Some(count) = payload.reset_credits_available {
                if count > 0 {
                    push_banked_reset(
                        &mut resets,
                        format!("Reset credits available: {count}"),
                        Some("expiry not reported".to_string()),
                    );
                }
            }
        }
    }

    resets
}

fn push_banked_reset(resets: &mut Vec<BankReset>, source: String, expiration: Option<String>) {
    let Some(expires_in) = expiration else {
        return;
    };
    resets.push(BankReset { source, expires_in });
}

fn print_banked_resets(resets: &[BankReset], theme: &Theme, bar_mode: bool) {
    if resets.is_empty() {
        return;
    }

    if bar_mode {
        let _ = std::io::stderr().write_all(b"\n");
    } else {
        println!();
    }
    println!("{}", colorize(theme.meter_color, "Bank resets"));
    println!("{}", colorize(theme.window_color, "------------"));
    for reset in resets {
        println!(
            "  {} {} — {}",
            colorize(theme.window_color, "•"),
            colorize(theme.meter_color, &reset.source),
            colorize(theme.reset_color, &reset.expires_in)
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
            "gpt-reserve" => "Reserve".to_string(),
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

fn render_usage_item(item: &UsageItem, theme: &Theme, use_progress: bool) {
    let (status_label, status_color) = match item.status {
        UsageStatus::Ok => (None, ""),
        UsageStatus::Warning => (Some("warning"), ansi_color_from_name(theme.bar_warning)),
        UsageStatus::Exhausted => (Some("exhausted"), ansi_color_from_name(theme.bar_exhausted)),
        UsageStatus::Unknown => (Some("unknown"), ansi_color_from_name(theme.bar_unknown)),
    };
    // Bar always draws in the theme's primary hue; the muted secondary
    // is reserved for the unfilled region. Status differences show in the
    // status word, not the bar color.
    let bar_primary = theme.bar_primary;
    let meter = fit_width(&item.meter, 12);
    let window = fit_width(&item.window_label, 11);
    let prefix = format!(
        "{ANSI_BOLD}{meter_color}{meter}{ANSI_RESET} {window_color}({window}){ANSI_RESET} ",
        meter_color = theme.meter_color,
        window_color = theme.window_color
    );

    match item.used_percent {
        Some(percent_raw) => {
            let percent = percent_raw.clamp(0.0, 100.0);
            let filled = (percent.round() as u64).min(100);
            let mut line = colorize(theme.meter_color, &format!("{percent:>5.1}%"));
            if let Some(reset_text) = &item.reset_text {
                line.push(' ');
                line.push_str(&colorize(theme.reset_color, reset_text));
            }
            if let Some(status) = status_label {
                line.push(' ');
                line.push_str(&colorize(status_color, status));
            }

            if use_progress {
                render_usage_bar(&prefix, &line, bar_primary, theme.bar_background, filled);
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
                line.push_str(&colorize(status_color, status));
            }
            if use_progress {
                println!("{: <22} {}", prefix, line);
            } else {
                println!("{: <22} {}", prefix, line);
            }
        }
    }
}

fn render_usage_bar(
    prefix: &str,
    line: &str,
    bar_primary: &str,
    bar_background: &str,
    filled: u64,
) {
    let bar = render_bar_cells(44, bar_primary, bar_background, filled);
    println!("{prefix:<22}{bar} {line}");
}

/// 4-level bar: solid/dark/medium shades in the theme primary, the
/// fractional cell interpolating ▒→▓ with the remainder, and the unfilled
/// region in the theme's muted same-hue secondary.
fn render_bar_cells(width: usize, fill_ansi: &str, background_ansi: &str, filled: u64) -> String {
    let filled_units = (filled as f64 / 100.0) * width as f64;
    let whole = (filled_units.floor() as usize).min(width);
    let frac = filled_units - whole as f64;

    let mut out = String::with_capacity(width * 4);
    out.push_str(fill_ansi);
    for _ in 0..whole {
        out.push('█');
    }

    let mut partial = 0;
    if whole < width && frac > 0.0 {
        // fraction picks among the middle chars, exactly like the tester:
        // emptier shade for small remainders, most-filled near the boundary
        let current = ['▒', '▓'];
        let idx = ((frac * current.len() as f64) as usize).min(current.len() - 1);
        out.push(current[current.len() - 1 - idx]);
        partial = 1;
    }
    out.push_str(ANSI_RESET);

    let empty_count = width - whole - partial;
    if empty_count > 0 {
        out.push_str(background_ansi);
        for _ in 0..empty_count {
            out.push('░');
        }
        out.push_str(ANSI_RESET);
    }
    out
}

fn normalize_usage_label(name: &str) -> String {
    let normalized = name.trim().to_lowercase();
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

fn ansi_color_from_name(name: &str) -> &'static str {
    match name {
        "black" => "\x1b[30m",
        "red" => "\x1b[31m",
        "green" => "\x1b[32m",
        "yellow" => "\x1b[33m",
        "blue" => "\x1b[34m",
        "magenta" => "\x1b[35m",
        "cyan" => "\x1b[36m",
        "white" => "\x1b[37m",
        "bright_black" => "\x1b[90m",
        "bright_red" => "\x1b[91m",
        "bright_green" => "\x1b[92m",
        "bright_yellow" => "\x1b[93m",
        "bright_blue" => "\x1b[94m",
        "bright_magenta" => "\x1b[95m",
        "bright_cyan" => "\x1b[96m",
        "bright_white" => "\x1b[97m",
        _ => "",
    }
}

fn colorize(ansi: &str, text: &str) -> String {
    format!("{ansi}{text}{ANSI_RESET}")
}

fn parse_usage_payload(payload: Value) -> ParsedUsage {
    let raw_obj = payload.as_object().cloned().unwrap_or_default();

    let plan_type = raw_obj.get("plan_type").and_then(as_string);

    let rate_limit = raw_obj
        .get("rate_limit")
        .and_then(Value::as_object)
        .and_then(parse_rate_limit);

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
        reset_credits: None,
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
    let rate_limit = obj
        .get("rate_limit")
        .and_then(Value::as_object)
        .and_then(parse_rate_limit);

    if limit_name.is_none() && metered_feature.is_none() && rate_limit.is_none() {
        return None;
    }

    Some(AdditionalRateLimit {
        limit_name,
        metered_feature,
        rate_limit,
    })
}

fn usage_status(
    used_percent: Option<f64>,
    explicitly_allowed: Option<bool>,
    limit_reached: Option<bool>,
) -> UsageStatus {
    let used_fraction = used_percent.and_then(|percent| {
        if !percent.is_finite() {
            return None;
        }
        Some((percent / 100.0).clamp(0.0, 1.0))
    });

    match used_fraction {
        None => UsageStatus::Unknown,
        Some(frac) if frac >= 1.0 => {
            if explicitly_allowed == Some(true) && limit_reached != Some(true) {
                UsageStatus::Warning
            } else {
                UsageStatus::Exhausted
            }
        }
        Some(frac) if frac >= 0.9 => UsageStatus::Warning,
        Some(_) => UsageStatus::Ok,
    }
}

fn additional_limit_slug(limit_name: Option<&str>, metered_feature: Option<&str>) -> String {
    let probe = format!(
        "{} {}",
        limit_name.unwrap_or(""),
        metered_feature.unwrap_or("")
    )
    .to_lowercase();
    let probe = probe.trim();
    if probe.contains("spark") || probe.contains("bengalfox") {
        return "spark".to_string();
    }

    if probe.contains("gpt-reserve") {
        return "gpt-reserve".to_string();
    }

    let source = metered_feature
        .or(limit_name)
        .unwrap_or("extra")
        .to_lowercase();
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
    }
}

fn window_label(seconds: Option<u64>) -> String {
    if let Some(total_seconds) = seconds {
        if total_seconds >= 86_400 {
            let days = (total_seconds as f64 / 86_400.0).round() as u64;
            format!("{} day{}", days, if days == 1 { "" } else { "s" })
        } else if total_seconds >= 3_600 {
            let hours = (total_seconds as f64 / 3_600.0).round() as u64;
            format!("{} hour{}", hours, if hours == 1 { "" } else { "s" })
        } else {
            let minutes = (total_seconds as f64 / 60.0).round() as u64;
            format!("{} minute{}", minutes, if minutes == 1 { "" } else { "s" })
        }
    } else {
        "unknown".to_string()
    }
}

fn slugify(input: &str) -> String {
    let normalized = input.trim().to_lowercase();
    let no_prefix = normalized
        .trim_start_matches("codex-")
        .trim_start_matches("codex_");
    let mut slug = String::with_capacity(no_prefix.len());
    let mut last_dash = false;
    for normalized_ch in no_prefix.chars() {
        let normalized_ch = normalized_ch.to_ascii_lowercase();
        if normalized_ch.is_ascii_alphanumeric() {
            last_dash = false;
            slug.push(normalized_ch);
            continue;
        }
        if normalized_ch == '-' || normalized_ch == '_' {
            if last_dash {
                continue;
            }
            last_dash = true;
            slug.push('-');
        } else {
            last_dash = false;
            slug.push(' ');
        }
    }

    slug.trim_matches('-').to_string()
}

fn title_case_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut out = String::new();
                    out.push(first.to_ascii_uppercase());
                    out.push_str(chars.as_str());
                    out
                }
                None => String::new(),
            }
        })
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
    window
        .reset_after_seconds
        .map(|after_secs| now_ms.saturating_add(after_secs.saturating_mul(1000)))
}

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
    use super::{
        additional_limit_slug, collect_banked_resets, collect_items, fit_width,
        format_account_plan_line, human_duration, now_millis, parse_timestamp_to_ms, persist_auth,
        render_bar_cells, AdditionalRateLimit, AuthRecord, ParsedUsage, RateLimit, ResetCredit,
        UsageWindow, BUILTIN_THEMES,
    };
    use std::env;
    use std::fs;

    #[test]
    fn render_bar_cells_shades_fill_and_background() {
        let bar = render_bar_cells(10, "\x1b[32m", "\x1b[38;5;59m", 42);

        let filled_cells = bar.chars().filter(|c| matches!(c, '█' | '▓' | '▒')).count();
        let empty_cells = bar.chars().filter(|c| *c == '░').count();
        assert_eq!(filled_cells + empty_cells, 10, "cells must sum to width");
        assert!(filled_cells > 0, "partial bar should include fill chars");
        assert!(filled_cells < 10, "bar should not be fully filled at 42%");
        assert!(empty_cells > 0, "unfilled area must use background color");
        assert!(
            bar.contains("\x1b[32m"),
            "fill region carries theme primary"
        );
        assert!(
            bar.contains("\x1b[38;5;59m"),
            "unfilled region carries background color"
        );
    }

    #[test]
    fn render_bar_cells_extremes_have_no_partial_cell() {
        for filled in [0u64, 100u64] {
            let bar = render_bar_cells(10, "\x1b[31m", "\x1b[38;5;59m", filled);
            assert_eq!(
                bar.chars()
                    .filter(|c| matches!(c, '█' | '▓' | '▒' | '░'))
                    .count(),
                10,
                "bar renders exactly width cells at {filled}%"
            );
        }
        let full = render_bar_cells(10, "\x1b[31m", "\x1b[38;5;59m", 100);
        assert_eq!(full.chars().filter(|c| *c == '█').count(), 10);
        let empty = render_bar_cells(10, "\x1b[31m", "\x1b[38;5;59m", 0);
        assert_eq!(empty.chars().filter(|c| *c == '░').count(), 10);
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
            reset_credits: None,
            raw: serde_json::Value::Null,
        };

        let line = format_account_plan_line(&auth, &usage, &BUILTIN_THEMES[0]);

        assert!(line.contains("john.doe@gmail.com"));
        assert!(line.contains("(prolite)"));
        assert!(!line.contains("a1b2c3d4"));
    }

    #[test]
    fn additional_limit_display_uses_real_names() {
        // spark/chat keep canonical display names via the slug mapping;
        // anything else normalizes the limit name, never underscore-slug
        let usage = ParsedUsage {
            plan_type: Some("prolite".to_string()),
            rate_limit: None,
            additional_rate_limits: vec![
                AdditionalRateLimit {
                    limit_name: Some("GPT-Reserve".to_string()),
                    metered_feature: None,
                    rate_limit: Some(RateLimit {
                        allowed: Some(true),
                        limit_reached: Some(false),
                        primary_window: Some(UsageWindow {
                            used_percent: Some(10.0),
                            limit_window_seconds: Some(604_800),
                            reset_after_seconds: Some(900),
                            reset_at: None,
                        }),
                        secondary_window: None,
                    }),
                },
                AdditionalRateLimit {
                    limit_name: None,
                    metered_feature: Some("codex_extra_feature".to_string()),
                    rate_limit: Some(RateLimit {
                        allowed: Some(true),
                        limit_reached: Some(false),
                        primary_window: Some(UsageWindow {
                            used_percent: Some(20.0),
                            limit_window_seconds: Some(86_400),
                            reset_after_seconds: Some(1_800),
                            reset_at: None,
                        }),
                        secondary_window: None,
                    }),
                },
            ],
            reset_credits_available: None,
            reset_credits: None,
            raw: serde_json::Value::Null,
        };

        let items = collect_items(&usage, 0);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].meter, "Reserve");
        assert_eq!(items[1].meter, "Extra Feature");
    }

    #[test]
    fn window_label_fits_prefix_geometry() {
        // 8-char meter + space + parens + 11-char window = 22 visible cols
        let window = fit_width("7 days", 11);
        assert_eq!(window.chars().count(), 11);
        assert_eq!(fit_width(&"x".repeat(20), 11).chars().count(), 11);
    }

    #[test]
    fn collect_banked_resets_itemizes_credits_with_expiry() {
        let usage = ParsedUsage {
            plan_type: Some("prolite".to_string()),
            rate_limit: None,
            additional_rate_limits: vec![],
            reset_credits_available: Some(2),
            reset_credits: Some(vec![
                ResetCredit {
                    title: Some("Weekly reset".to_string()),
                    expires_at_ms: Some(3_600_000),
                },
                ResetCredit {
                    title: None,
                    expires_at_ms: Some(900_000),
                },
            ]),
            raw: serde_json::Value::Null,
        };

        let resets = collect_banked_resets(&usage, 0);
        assert_eq!(resets.len(), 2);
        assert!(resets
            .iter()
            .any(|item| item.source == "Weekly reset" && item.expires_in == "expires in 1h"));
        assert!(resets
            .iter()
            .any(|item| item.source == "Reset credit" && item.expires_in == "expires in 15m"));
    }

    #[test]
    fn collect_banked_resets_keeps_identical_looking_credits_distinct() {
        let usage = ParsedUsage {
            plan_type: Some("prolite".to_string()),
            rate_limit: None,
            additional_rate_limits: vec![],
            reset_credits_available: Some(2),
            reset_credits: Some(vec![
                ResetCredit {
                    title: Some("Full reset".to_string()),
                    expires_at_ms: Some(3_600_000),
                },
                ResetCredit {
                    title: Some("Full reset".to_string()),
                    expires_at_ms: Some(3_600_000),
                },
            ]),
            raw: serde_json::Value::Null,
        };

        let resets = collect_banked_resets(&usage, 0);
        assert_eq!(
            resets.len(),
            2,
            "distinct API credits with identical title+expiry must each render"
        );
    }

    #[test]
    fn collect_banked_resets_falls_back_to_count_without_details() {
        let usage = ParsedUsage {
            plan_type: Some("prolite".to_string()),
            rate_limit: None,
            additional_rate_limits: vec![],
            reset_credits_available: Some(2),
            reset_credits: None,
            raw: serde_json::Value::Null,
        };

        let resets = collect_banked_resets(&usage, 0);
        assert_eq!(resets.len(), 1);
        assert_eq!(resets[0].source, "Reset credits available: 2");
        assert_eq!(resets[0].expires_in, "expiry not reported");
    }

    #[test]
    fn collect_banked_resets_empty_when_no_credits() {
        let usage = ParsedUsage {
            plan_type: Some("prolite".to_string()),
            rate_limit: None,
            additional_rate_limits: vec![],
            reset_credits_available: Some(0),
            reset_credits: None,
            raw: serde_json::Value::Null,
        };

        assert!(collect_banked_resets(&usage, 0).is_empty());
    }

    #[test]
    fn parse_timestamp_handles_epoch_and_rfc3339() {
        assert_eq!(
            parse_timestamp_to_ms(&serde_json::json!(1_788_295_133)),
            Some(1_788_295_133_000)
        );
        assert_eq!(
            parse_timestamp_to_ms(&serde_json::json!("1788295133")),
            Some(1_788_295_133_000)
        );
        assert_eq!(
            parse_timestamp_to_ms(&serde_json::json!("2026-09-01T00:00:00Z")),
            Some(1_788_220_800_000)
        );
        assert_eq!(
            parse_timestamp_to_ms(&serde_json::json!("not-a-time")),
            None
        );
    }

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
    let claims = parse_jwt(token)?;
    claims
        .get(top)
        .and_then(Value::as_object)
        .and_then(|obj| obj.get(inner))
        .and_then(as_string)
}

fn parse_jwt(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _sig = parts.next()?;

    let mut payload = payload.to_string();
    while !payload.len().is_multiple_of(4) {
        payload.push('=');
    }
    let bytes = URL_SAFE.decode(&payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn parse_timestamp_to_ms(value: &Value) -> Option<u64> {
    let raw_secs = match value {
        Value::Number(_) => as_u64(value)?,
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return None;
            }
            if trimmed.chars().all(|c| c.is_ascii_digit()) {
                trimmed.parse::<u64>().ok()?
            } else {
                return parse_rfc3339_to_ms(trimmed);
            }
        }
        _ => return None,
    };
    Some(if raw_secs > 1_000_000_000_000 {
        raw_secs
    } else {
        raw_secs.saturating_mul(1000)
    })
}

fn parse_rfc3339_to_ms(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let month: i64 = text.get(5..7)?.parse().ok()?;
    let day: i64 = text.get(8..10)?.parse().ok()?;
    if !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    if bytes[13] != b':' {
        return None;
    }
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    if bytes[16] != b':' {
        return None;
    }
    let second: i64 = text.get(17..19)?.parse().ok()?;

    let mut rest = &text[19..];
    let mut millis: i64 = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits: usize = fraction.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        let fraction_value: f64 = format!("0.{}", &fraction[..digits]).parse().ok()?;
        millis = (fraction_value * 1000.0).round() as i64;
        rest = &fraction[digits..];
    }

    let offset_secs: i64 = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.chars().next()? {
                '+' => 1,
                '-' => -1,
                _ => return None,
            };
            let offset = rest.get(1..6)?;
            if offset.as_bytes()[2] != b':' {
                return None;
            }
            let offset_hours: i64 = offset.get(0..2)?.parse().ok()?;
            let offset_minutes: i64 = offset.get(3..5)?.parse().ok()?;
            sign * (offset_hours * 3_600 + offset_minutes * 60)
        }
    };

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }

    let day_secs = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
        - offset_secs;
    Some(
        (day_secs as u64)
            .saturating_mul(1000)
            .saturating_add(millis as u64),
    )
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_of_year = (month + 9) % 12;
    let day_of_year = (153 * month_of_year + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn as_string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn as_bool(value: &Value) -> Option<bool> {
    value.as_bool()
}

fn as_f64(value: &Value) -> Option<f64> {
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
