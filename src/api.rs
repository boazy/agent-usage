use crate::auth::{apply_refresh_payload, AuthRecord};
use crate::cli::Cli;
use crate::usage::{parse_reset_credits, parse_usage_payload, ParsedUsage};
use eyre::{eyre, Result, WrapErr};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde_json::Value;
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::time::Duration;
use url::Url;

const USER_AGENT_VALUE: &str = "codex-usage-rs/0.1.0";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const RESET_CREDITS_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn fetch_usage(
    cli: &Cli,
    auth: &mut AuthRecord,
    use_progress: bool,
) -> Result<ParsedUsage> {
    let base = normalize_base_url(&cli.base_url)?;
    let usage_url = format!("{base}/wham/usage");
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .wrap_err("failed to build usage HTTP client")?;
    let spinner = create_spinner(use_progress)?;
    let request = |auth: &AuthRecord| -> Result<Response> {
        client
            .get(&usage_url)
            .headers(build_headers(auth)?)
            .send()
            .wrap_err("usage request failed")
    };

    let first_response = request(auth).inspect_err(|_| finish_spinner(spinner.as_ref()))?;
    if first_response.status().is_success() {
        return finish_success(first_response, &client, &base, auth, spinner.as_ref());
    }
    let first_status = first_response.status();

    if (first_status == 401 || first_status == 403) && auth.refresh_token.is_some() {
        if let Some(spinner) = &spinner {
            spinner.set_message("Refreshing Codex access token…");
        }
        if let Err(error) = refresh_access_token(auth) {
            finish_spinner(spinner.as_ref());
            return Err(error.wrap_err(format!(
                "usage endpoint returned HTTP {first_status}; token refresh failed"
            )));
        }
        let second_response = request(auth).inspect_err(|_| finish_spinner(spinner.as_ref()))?;
        if second_response.status().is_success() {
            return finish_success(second_response, &client, &base, auth, spinner.as_ref());
        }
        finish_spinner(spinner.as_ref());
        return Err(eyre!(
            "usage endpoint returned HTTP {} after refresh",
            second_response.status()
        ));
    }

    finish_spinner(spinner.as_ref());
    Err(eyre!("usage endpoint returned HTTP {first_status}"))
}

pub(crate) fn normalize_base_url(input: &str) -> Result<String> {
    let trimmed = input.trim().trim_end_matches('/');
    let url = Url::parse(trimmed).wrap_err("base URL must be an absolute HTTP(S) URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(eyre!(
            "base URL must be an absolute HTTP(S) URL without credentials"
        ));
    }
    let host = url
        .host_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| eyre!("base URL must include a host"))?;
    if host == "chatgpt.com" || host == "chat.openai.com" {
        let mut origin = format!("{}://{host}", url.scheme());
        if let Some(port) = url.port() {
            let _ = write!(&mut origin, ":{port}");
        }
        return Ok(format!("{origin}/backend-api"));
    }
    Ok(trimmed.to_owned())
}

fn create_spinner(use_progress: bool) -> Result<Option<ProgressBar>> {
    if !use_progress || !std::io::stdout().is_terminal() || !std::io::stderr().is_terminal() {
        return Ok(None);
    }
    let spinner = ProgressBar::new_spinner();
    let style = ProgressStyle::with_template("{spinner} {msg}")
        .wrap_err("failed to configure usage spinner")?
        .tick_strings(&["◒", "◐", "◓", "◑"]);
    spinner.set_style(style);
    spinner.set_message("Fetching Codex usage…");
    spinner.enable_steady_tick(Duration::from_millis(80));
    Ok(Some(spinner))
}

fn finish_success(
    response: Response,
    client: &Client,
    base: &str,
    auth: &AuthRecord,
    spinner: Option<&ProgressBar>,
) -> Result<ParsedUsage> {
    let payload: Value = match response.json() {
        Ok(payload) => payload,
        Err(error) => {
            if let Some(spinner) = spinner {
                spinner.finish_and_clear();
            }
            return Err(error).wrap_err("usage endpoint returned unreadable JSON");
        }
    };
    let mut usage = parse_usage_payload(payload);
    if usage.reset_credits_available.unwrap_or(0) > 0 {
        if let Some(spinner) = spinner {
            spinner.set_message("Fetching banked reset credits…");
        }
        usage.reset_credits = fetch_reset_credits(client, base, auth);
    }
    if let Some(spinner) = spinner {
        spinner.finish_and_clear();
    }
    Ok(usage)
}

fn finish_spinner(spinner: Option<&ProgressBar>) {
    if let Some(spinner) = spinner {
        spinner.finish_and_clear();
    }
}

fn build_headers(auth: &AuthRecord) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    let bearer = format!("Bearer {}", auth.access_token);
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&bearer).wrap_err("access token cannot be used as an HTTP header")?,
    );
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    if let Some(account_id) = &auth.account_id {
        headers.insert(
            "ChatGPT-Account-Id",
            HeaderValue::from_str(account_id)
                .wrap_err("account ID cannot be used as an HTTP header")?,
        );
    }
    Ok(headers)
}

fn fetch_reset_credits(
    client: &Client,
    base: &str,
    auth: &AuthRecord,
) -> Option<Vec<crate::usage::ResetCredit>> {
    let response = client
        .get(format!("{base}/wham/rate-limit-reset-credits"))
        .headers(build_headers(auth).ok()?)
        .timeout(RESET_CREDITS_TIMEOUT)
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload: Value = response.json().ok()?;
    parse_reset_credits(&payload)
}

fn refresh_access_token(auth: &mut AuthRecord) -> Result<()> {
    let refresh_token = auth
        .refresh_token
        .as_deref()
        .ok_or_else(|| eyre!("authentication file is missing tokens.refresh_token"))?;
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .wrap_err("failed to build refresh HTTP client")?;
    let response = client
        .post("https://auth.openai.com/oauth/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", auth.default_oauth_client_id()),
        ])
        .send()
        .wrap_err("token refresh request failed")?;
    if !response.status().is_success() {
        return Err(eyre!(
            "token refresh endpoint returned HTTP {}",
            response.status()
        ));
    }
    let payload: Value = response
        .json()
        .wrap_err("token refresh endpoint returned unreadable JSON")?;
    apply_refresh_payload(auth, &payload).wrap_err("token refresh response was incomplete")
}

#[cfg(test)]
mod tests {
    use super::normalize_base_url;

    #[test]
    fn normalizes_official_hosts_without_losing_port() {
        assert_eq!(
            normalize_base_url("https://chatgpt.com/example")
                .unwrap_or_else(|error| panic!("unexpected URL error: {error}")),
            "https://chatgpt.com/backend-api"
        );
        assert_eq!(
            normalize_base_url("https://chat.openai.com:8443/")
                .unwrap_or_else(|error| panic!("unexpected URL error: {error}")),
            "https://chat.openai.com:8443/backend-api"
        );
    }

    #[test]
    fn rejects_invalid_or_credentialed_base_urls() {
        assert!(normalize_base_url("not a URL").is_err());
        assert!(normalize_base_url("https://user:password@example.test").is_err());
    }
}
