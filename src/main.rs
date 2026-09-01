use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::{Client, Response};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

const DEFAULT_AUTH_PATH: &str = "~/.codex/auth.json";
const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api";

const JWT_AUTH_CLAIM: &str = "https://api.openai.com/auth";
const JWT_PROFILE_CLAIM: &str = "https://api.openai.com/profile";

#[derive(Debug, Parser)]
#[command(name = "codex-usage", version, about = "Show Codex usage details from ~/.codex/auth.json")]
struct Cli {
    /// Path to Codex auth JSON file (defaults to ~/.codex/auth.json)
    #[arg(short, long, default_value = DEFAULT_AUTH_PATH)]
    auth_file: String,

    /// Base URL override for Codex account endpoints
    #[arg(
        short = 'b',
        long,
        default_value = DEFAULT_BASE_URL,
        help = "Accepted overrides are chatgpt.com / chat.openai.com origins; other hosts fall back to https://chatgpt.com/backend-api"
    )]
    base_url: String,

    /// Print raw JSON payload instead of formatted bars
    #[arg(short, long)]
    json: bool,

    /// Hide progress rendering (useful in scripts / logs)
    #[arg(long)]
    no_progress: bool,
}

#[derive(Clone)]
struct AuthRecord {
    access_token: String,
    account_id: Option<String>,
    email: Option<String>,
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

#[derive(Clone)]
struct ParsedUsage {
    plan_type: Option<String>,
    rate_limit: Option<RateLimit>,
    additional_rate_limits: Vec<AdditionalRateLimit>,
    reset_credits_available: Option<u64>,
    raw: Value,
}

#[derive(Clone)]
struct UsageItem {
    label: String,
    used_percent: Option<f64>,
    status: UsageStatus,
    reset_text: Option<String>,
}

#[derive(Copy, Clone, Debug)]
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

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    let auth_path = expand_path(&cli.auth_file);
    let auth = load_auth(&auth_path)?;
    let usage = fetch_usage(&cli, &auth, !cli.no_progress)?;

    if cli.json {
        let pretty = serde_json::to_string_pretty(&usage.raw)?;
        println!("{pretty}");
        return Ok(());
    }

    print_usage_report(&usage, auth.email.as_deref(), auth.account_id.as_deref());
    Ok(())
}

fn load_auth(path: &Path) -> Result<AuthRecord, Box<dyn Error>> {
    let raw = fs::read_to_string(path)?;
    let json: Value = serde_json::from_str(&raw)?;

    let access_token = json
        .pointer("/tokens/access_token")
        .and_then(as_string)
        .or_else(|| json.pointer("/OPENAI_API_KEY").and_then(as_string));

    let access_token = access_token.ok_or_else(|| {
        CliError::new("auth file does not contain tokens.access_token or OPENAI_API_KEY")
    })?;

    let account_id = json
        .pointer("/tokens/account_id")
        .and_then(as_string)
        .or_else(|| parse_jwt_claim(&access_token, JWT_AUTH_CLAIM, "chatgpt_account_id"));

    let email = json
        .pointer("/tokens/id_token")
        .and_then(as_string)
        .and_then(|token| parse_jwt_claim(&token, JWT_PROFILE_CLAIM, "email"))
        .or_else(|| parse_jwt_claim(&access_token, JWT_PROFILE_CLAIM, "email"));

    Ok(AuthRecord {
        access_token,
        account_id,
        email,
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
    }
    DEFAULT_BASE_URL.to_string()
}

fn fetch_usage(cli: &Cli, auth: &AuthRecord, use_progress: bool) -> Result<ParsedUsage, Box<dyn Error>> {
    let base = normalize_base_url(&cli.base_url);
    let usage_url = format!("{}/wham/usage", base);

    let mut headers = HeaderMap::new();
    let bearer = format!("Bearer {}", auth.access_token);
    headers.insert(AUTHORIZATION, HeaderValue::from_str(&bearer)?);
    headers.insert(USER_AGENT, HeaderValue::from_static("codex-usage-rs/0.1"));
    if let Some(account_id) = &auth.account_id {
        headers.insert("ChatGPT-Account-Id", HeaderValue::from_str(account_id)?);
    }

    let client = Client::builder().timeout(Duration::from_secs(20)).build()?;

    let spinner = if use_progress {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner} {msg}")?
                .tick_strings(&["▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"]),
        );
        pb.set_message("Fetching Codex usage…");
        pb.enable_steady_tick(Duration::from_millis(80));
        Some(pb)
    } else {
        None
    };

    let response: Response = client
        .get(usage_url)
        .headers(headers)
        .send()
        .map_err(|e| CliError::with_msg(format!("request failed: {e}")))?;

    if let Some(pb) = &spinner {
        pb.finish_and_clear();
    }

    if !response.status().is_success() {
        return Err(Box::new(CliError::new(format!(
            "usage endpoint returned HTTP {}",
            response.status()
        ))));
    }

    let value: Value = response.json()?;
    let parsed = parse_usage_payload(value);
    Ok(parsed)
}

fn print_usage_report(usage: &ParsedUsage, email: Option<&str>, account_id: Option<&str>) {
    let now_ms = now_millis();

    println!("Codex plan usage");
    println!("================");
    println!("plan: {}", usage.plan_type.as_deref().unwrap_or("unknown"));
    if let Some(email) = email {
        println!("email: {}", email);
    }
    if let Some(account_id) = account_id {
        println!("account: {}", account_id);
    }
    if let Some(credits) = usage.reset_credits_available {
        println!("reset credits: {credits}");
    }
    println!();

    let mut items = collect_items(usage, now_ms);
    if items.is_empty() {
        println!("No usage windows were reported in response.");
        return;
    }

    for item in items.drain(..) {
        render_usage_item(&item);
    }
}

fn collect_items(payload: &ParsedUsage, now_ms: u64) -> Vec<UsageItem> {
    let mut items = Vec::new();

    if let Some(rate_limit) = &payload.rate_limit {
        if let Some(primary) = &rate_limit.primary_window {
            items.push(build_item(
                "Primary",
                primary,
                payload.plan_type.as_deref(),
                "chat",
                rate_limit.allowed,
                rate_limit.limit_reached,
                now_ms,
            ));
        }
        if let Some(secondary) = &rate_limit.secondary_window {
            items.push(build_item(
                "Secondary",
                secondary,
                payload.plan_type.as_deref(),
                "chat",
                rate_limit.allowed,
                rate_limit.limit_reached,
                now_ms,
            ));
        }
    }

    for extra in &payload.additional_rate_limits {
        let slug = additional_limit_slug(extra.limit_name.as_deref(), extra.metered_feature.as_deref());
        let display = if slug == "spark" {
            "Spark".to_string()
        } else if let Some(name) = &extra.limit_name {
            name.clone()
        } else {
            title_case_slug(&slug)
        };

        if let Some(rate_limit) = &extra.rate_limit {
            if let Some(primary) = &rate_limit.primary_window {
                items.push(build_item(
                    "Primary",
                    primary,
                    Some(&display),
                    Some(&display),
                    rate_limit.allowed,
                    rate_limit.limit_reached,
                    now_ms,
                ));
            }
            if let Some(secondary) = &rate_limit.secondary_window {
                items.push(build_item(
                    "Secondary",
                    secondary,
                    Some(&display),
                    Some(&display),
                    rate_limit.allowed,
                    rate_limit.limit_reached,
                    now_ms,
                ));
            }
        }
    }

    // Keep deterministic order: chat windows first, then extras.
    items
}

fn build_item(
    key: &str,
    window: &UsageWindow,
    plan_name: Option<&str>,
    meter_name: Option<&str>,
    allowed: Option<bool>,
    limit_reached: Option<bool>,
    now_ms: u64,
) -> UsageItem {
    let label = match (meter_name, plan_name) {
        (Some(meter), Some(plan)) if meter != plan => format!("{plan} {meter} {key}"),
        (Some(meter), _) => format!("{meter} {key}"),
        _ => format!("{key}"),
    };
    let status = usage_status(window.used_percent, allowed, limit_reached);
    let reset_text = resolve_reset_text(window, now_ms);
    UsageItem {
        label: format!("{} ({})", label, window_label(window.limit_window_seconds, key)),
        used_percent: window.used_percent,
        status,
        reset_text,
    }
}

fn render_usage_item(item: &UsageItem) {
    if let Some(used_percent) = item.used_percent {
        let clamped = used_percent.clamp(0.0, 100.0);
        let bar = ProgressBar::new(100);
        let (color, status_word) = match item.status {
            UsageStatus::Ok => ("green", "ok"),
            UsageStatus::Warning => ("yellow", "warn"),
            UsageStatus::Exhausted => ("red", "exhausted"),
            UsageStatus::Unknown => ("blue", "unknown"),
        };
        let template = format!(
            "{{prefix:>24}} {{bar:44.{color}/{color}}} {{msg}} {{pos:>6.1}}%"
        );
        let style = ProgressStyle::with_template(&template).unwrap().progress_chars("█▉▊▌ ");
        bar.set_style(style);
        bar.set_prefix(&item.label);
        bar.set_position(clamped.round() as u64);
        let mut message = item.status.to_text().to_string();
        if let Some(reset_text) = &item.reset_text {
            message.push_str(" | ");
            message.push_str(reset_text);
        }
        bar.set_message(format!("{message}"));
        bar.println(format!("{used:>5.1}%", used = clamped));
        bar.finish_and_clear();
    } else {
        println!(
            "{:<24} [{}] no used_percent in response",
            item.label,
            item.status.to_text()
        );
    }
}

fn parse_usage_payload(payload: Value) -> ParsedUsage {
    let obj = payload.as_object().cloned().unwrap_or_default();
    let plan_type = obj.get("plan_type").and_then(as_string);

    let rate_limit = obj
        .get("rate_limit")
        .and_then(Value::as_object)
        .and_then(parse_rate_limit)
        .flatten();

    let additional_rate_limits = obj
        .get("additional_rate_limits")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|raw| parse_additional_rate_limit(raw))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let reset_credits_available = obj
        .get("rate_limit_reset_credits")
        .and_then(|v| v.get("available_count"))
        .and_then(as_u64);

    ParsedUsage {
        plan_type,
        rate_limit,
        additional_rate_limits,
        reset_credits_available,
        raw: payload,
    }
}

fn parse_rate_limit(raw: &std::collections::HashMap<String, Value>) -> Option<RateLimit> {
    let allowed = raw.get("allowed").and_then(as_bool);
    let limit_reached = raw.get("limit_reached").and_then(as_bool);

    let primary_window = raw
        .get("primary_window")
        .and_then(Value::as_object)
        .and_then(parse_usage_window)
        .and_then(|w| Some(w).filter(|w| is_non_empty_window(w)));

    let secondary_window = raw
        .get("secondary_window")
        .and_then(Value::as_object)
        .and_then(parse_usage_window)
        .and_then(|w| Some(w).filter(|w| is_non_empty_window(w)));

    if !is_non_empty_window_struct(allowed, limit_reached, primary_window.as_ref(), secondary_window.as_ref()) {
        None
    } else {
        Some(RateLimit {
            allowed,
            limit_reached,
            primary_window,
            secondary_window,
        })
    }
}

fn parse_usage_window(raw: &std::collections::HashMap<String, Value>) -> Option<UsageWindow> {
    let used_percent = raw.get("used_percent").and_then(as_f64);
    let limit_window_seconds = raw.get("limit_window_seconds").and_then(as_u64);
    let reset_after_seconds = raw.get("reset_after_seconds").and_then(as_u64);
    let reset_at = raw.get("reset_at").and_then(as_u64);

    if used_percent.is_none() && limit_window_seconds.is_none() && reset_after_seconds.is_none() {
        if reset_at.is_none() {
            return None;
        }
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
        .and_then(parse_rate_limit)
        .filter(|rl| is_non_empty_rate_limit(rl));

    Some(AdditionalRateLimit {
        limit_name,
        metered_feature,
        rate_limit,
    })
}

fn is_non_empty_rate_limit(rl: &RateLimit) -> bool {
    is_non_empty_rate_limit(
        rl.allowed,
        rl.limit_reached,
        rl.primary_window.as_ref(),
        rl.secondary_window.as_ref(),
    )
}

fn is_non_empty_rate_limit(
    allowed: Option<bool>,
    limit_reached: Option<bool>,
    primary_window: Option<&UsageWindow>,
    secondary_window: Option<&UsageWindow>,
) -> bool {
    allowed.is_some() || limit_reached.is_some() || primary_window.is_some() || secondary_window.is_some()
}

fn is_non_empty_window_struct(
    allowed: Option<bool>,
    limit_reached: Option<bool>,
    primary_window: Option<&UsageWindow>,
    secondary_window: Option<&UsageWindow>,
) -> bool {
    is_non_empty_rate_limit(allowed, limit_reached, primary_window, secondary_window)
}

fn is_non_empty_window(window: &UsageWindow) -> bool {
    window.used_percent.is_some()
        || window.limit_window_seconds.is_some()
        || window.reset_after_seconds.is_some()
        || window.reset_at.is_some()
}

fn usage_status(
    used_percent: Option<f64>,
    explicitly_allowed: Option<bool>,
    limit_reached: Option<bool>,
) -> UsageStatus {
    let used_fraction = used_percent.and_then(|v| {
        if !v.is_finite() {
            None
        } else {
            Some((v / 100.0).clamp(0.0, 1.0))
        }
    });

    match used_fraction {
        None => UsageStatus::Unknown,
        Some(fraction) if fraction >= 1.0 => {
            if explicitly_allowed == Some(true) && limit_reached != Some(true) {
                UsageStatus::Warning
            } else {
                UsageStatus::Exhausted
            }
        }
        Some(fraction) => {
            if fraction >= 0.9 {
                UsageStatus::Warning
            } else {
                UsageStatus::Ok
            }
        }
    }
}

impl UsageStatus {
    fn to_text(self) -> &'static str {
        match self {
            UsageStatus::Ok => "ok",
            UsageStatus::Warning => "warning",
            UsageStatus::Exhausted => "exhausted",
            UsageStatus::Unknown => "unknown",
        }
    }
}

fn window_label(seconds: Option<u64>, fallback_key: &str) -> String {
    if let Some(total_seconds) = seconds {
        if total_seconds >= 86_400 {
            let days = (total_seconds as f64 / 86_400.0).round() as u64;
            return format!("{} days", days);
        }
        let hours = ((total_seconds as f64 / 3_600.0).round() as u64).max(1);
        return format!("{} hours", hours);
    }

    match fallback_key {
        "Primary" | "primary" => "1 hour".to_string(),
        _ => "7 days".to_string(),
    }
}

fn additional_limit_slug(limit_name: Option<&str>, metered_feature: Option<&str>) -> String {
    let probe = format!("{} {}", limit_name.unwrap_or(""), metered_feature.unwrap_or(""))
        .to_lowercase();
    if probe.contains("spark") || probe.contains("bengalfox") {
        return "spark".to_string();
    }

    let source = metered_feature
        .or(limit_name)
        .unwrap_or("extra")
        .to_lowercase();

    let slug = source
        .replace("codex-", "")
        .replace("codex_", "")
        .replace(|c: char| !c.is_ascii_alphanumeric(), "-");

    slug.trim_matches('-')
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn title_case_slug(slug: &str) -> String {
    let mut out = String::new();
    for (i, part) in slug.split('-').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(ch) = chars.next() {
            out.push(ch.to_ascii_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

fn resolve_reset_text(window: &UsageWindow, now_ms: u64) -> Option<String> {
    let reset_ms = match resolve_reset_time(window, now_ms) {
        Some(ms) => ms,
        None => return None,
    };

    let diff_ms = reset_ms.saturating_sub(now_ms);
    let diff_secs = (diff_ms / 1000).max(0);
    Some(format!(
        "resets in {}",
        human_duration(diff_secs)
    ))
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
    let secs = total_seconds;
    let days = secs / 86_400;
    if days > 0 {
        return format!("{}d", days);
    }
    let hours = (secs % 86_400) / 3600;
    if hours > 0 {
        return format!("{}h", hours);
    }
    let mins = (secs % 3600) / 60;
    if mins > 0 {
        return format!("{}m", mins);
    }
    format!("{}s", secs)
}

fn parse_jwt_claim(token: &str, top: &str, inner: &str) -> Option<String> {
    let claims = parse_jwt(token)?;
    claims
        .get(top)
        .and_then(Value::as_object)
        .and_then(|obj| obj.get(inner))
        .and_then(as_string)
        .map(|s| s.to_ascii_lowercase())
}

fn parse_jwt(token: &str) -> Option<Value> {
    let mut iter = token.split('.');
    let _header = iter.next()?;
    let payload = iter.next()?;
    let _sig = iter.next()?;

    let mut payload = payload.replace('-', "+").replace('_', "/");
    while !payload.len().is_multiple_of(4) {
        payload.push('=');
    }

    let bytes = URL_SAFE.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
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

#[derive(Debug)]
struct CliError {
    msg: String,
}

impl CliError {
    fn new<S: Into<String>>(msg: S) -> Self {
        Self { msg: msg.into() }
    }

    fn with_msg(msg: String) -> Self {
        Self { msg }
    }
}

impl Display for CliError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl Error for CliError {}
