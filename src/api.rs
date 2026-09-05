use crate::auth::{apply_refresh_payload, token_expiry, AuthRecord};
use crate::dashboard::{
    AccountUsage, AllowanceWindow, CredentialKind, CreditAmount, CreditCount, CreditUnit, Provider,
    UsageAllowance, UsageCredits,
};
use crate::time::{now_millis, parse_timestamp_to_ms, Millis};
use eyre::{eyre, Result};
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::{json, Value};
use std::fmt;
use std::time::Duration;
use url::Url;

mod antigravity;
mod claude;
mod codex;
mod cursor;
mod openrouter;

#[cfg(test)]
mod tests;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE_BYTES: usize = 1_048_576;
const USER_AGENT_VALUE: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
// Public installed-app OAuth credentials from OMP's google-antigravity.kdl,
// not account secrets. Google requires both on the refresh-token grant.
const ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const ANTIGRAVITY_CLIENT_SECRET: &str = "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";
const CLAUDE_USER_AGENT: &str = "claude-cli/2.1.257 (external, cli)";
const ANTIGRAVITY_USER_AGENT: &str =
    "antigravity/hub/2.8.0 (aidev_client; os_type=darwin; arch=arm64; cl=963137146)";

// Only this module can inject endpoints. Production callers cannot redirect
// OAuth credentials through a provider configuration or token response.
struct Endpoints<'a> {
    codex: &'a str,
    claude: &'a str,
    openrouter: &'a str,
    antigravity: &'a str,
    cursor_api: &'a str,
    cursor_web: &'a str,
    cursor_token: &'a str,
    codex_token: &'a str,
    claude_token: &'a str,
    antigravity_token: &'a str,
}

impl<'a> Endpoints<'a> {
    const fn production(codex: &'a str) -> Self {
        Self {
            codex,
            claude: "https://api.anthropic.com/api/oauth/usage",
            openrouter: "https://openrouter.ai/api/v1",
            antigravity: "https://daily-cloudcode-pa.googleapis.com",
            cursor_api: "https://api2.cursor.sh",
            cursor_web: "https://cursor.com",
            cursor_token: "https://api2.cursor.sh/auth/exchange_user_api_key",
            codex_token: "https://auth.openai.com/oauth/token",
            claude_token: "https://api.anthropic.com/v1/oauth/token",
            antigravity_token: "https://oauth2.googleapis.com/token",
        }
    }
}

pub(crate) async fn fetch_account(
    client: &Client,
    provider: &Provider,
    auth: &mut AuthRecord,
    codex_base_url: &str,
) -> Result<AccountUsage> {
    ensure_supported(provider, auth)?;
    let base = if *provider == Provider::Codex {
        normalize_base_url(codex_base_url)?
    } else {
        String::new()
    };
    fetch_with_endpoints(client, provider, auth, &Endpoints::production(&base)).await
}

pub(crate) fn normalize_base_url(input: &str) -> Result<String> {
    let url = Url::parse(input.trim()).map_err(|_| eyre!("invalid Codex base URL"))?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("chatgpt.com" | "chat.openai.com"))
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path().trim_end_matches('/'), "" | "/backend-api")
    {
        return Err(eyre!(
            "Codex credentials require an official HTTPS ChatGPT backend URL"
        ));
    }
    Ok(format!(
        "{}/backend-api",
        url.origin().ascii_serialization()
    ))
}

fn ensure_supported(provider: &Provider, auth: &AuthRecord) -> Result<()> {
    match (provider, auth.kind) {
        (Provider::Codex | Provider::Claude | Provider::Antigravity, CredentialKind::OAuth)
        | (Provider::OpenRouter, CredentialKind::ApiKey)
        | (Provider::Cursor, _) => {}
        (Provider::Codex, CredentialKind::ApiKey) => {
            return Err(eyre!(
                "unsupported: Codex API keys do not expose subscription usage"
            ));
        }
        (Provider::Claude, CredentialKind::ApiKey) => {
            return Err(eyre!(
                "unsupported: ordinary Claude API keys do not expose subscription usage"
            ));
        }
        (Provider::Antigravity, CredentialKind::ApiKey) => {
            return Err(eyre!(
                "unsupported: Antigravity quota requires OAuth credentials"
            ));
        }
        (Provider::OpenRouter, CredentialKind::OAuth) => {
            return Err(eyre!("unsupported: OpenRouter usage requires an API key"));
        }
        (Provider::Other(_), _) => return Err(eyre!("unsupported billing provider")),
    }
    if auth.access_token.is_empty() || auth.access_token == "__remote__" {
        return Err(eyre!(
            "no local access credential; sign in through the owning source"
        ));
    }
    if *provider == Provider::Antigravity
        && auth
            .project_id
            .as_deref()
            .is_none_or(|id| id.trim().is_empty())
    {
        return Err(eyre!(
            "Antigravity OAuth credentials require a stored project ID"
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) async fn fetch_account_at(
    client: &Client,
    provider: &Provider,
    auth: &mut AuthRecord,
    base_url: &str,
) -> Result<AccountUsage> {
    let claude = format!("{base_url}/api/oauth/usage");
    let token = format!("{base_url}/oauth/token");
    let cursor_token = format!("{base_url}/auth/exchange_user_api_key");
    let endpoints = Endpoints {
        codex: base_url,
        claude: &claude,
        openrouter: base_url,
        antigravity: base_url,
        cursor_api: base_url,
        cursor_web: base_url,
        cursor_token: &cursor_token,
        codex_token: &token,
        claude_token: &token,
        antigravity_token: &token,
    };
    fetch_with_endpoints(client, provider, auth, &endpoints).await
}

async fn fetch_with_endpoints(
    client: &Client,
    provider: &Provider,
    auth: &mut AuthRecord,
    endpoints: &Endpoints<'_>,
) -> Result<AccountUsage> {
    ensure_supported(provider, auth)?;
    let expired = auth.kind == CredentialKind::OAuth
        && auth
            .expires_at
            .zip(now_ms())
            .is_some_and(|(expiry, now)| expiry <= now);
    if expired {
        refresh(client, provider, auth, endpoints).await?;
    }
    let first = fetch_once(client, provider, auth, endpoints).await;
    match first {
        Err(HttpError::Status(status))
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
                && auth.kind == CredentialKind::OAuth
                && !expired
                && refresh_token(auth).is_some() =>
        {
            refresh(client, provider, auth, endpoints).await?;
            // Mutations intentionally survive a failing usage request: the caller
            // persists a rotated refresh token even when usage is unavailable.
            fetch_once(client, provider, auth, endpoints)
                .await
                .map_err(|error| eyre!("usage after token refresh: {error}"))
        }
        result => result.map_err(|error| eyre!("usage request: {error}")),
    }
}

async fn fetch_once(
    client: &Client,
    provider: &Provider,
    auth: &mut AuthRecord,
    endpoints: &Endpoints<'_>,
) -> std::result::Result<AccountUsage, HttpError> {
    match provider {
        Provider::Codex => codex::fetch(client, auth, endpoints.codex).await,
        Provider::Claude => claude::fetch(client, auth, endpoints.claude).await,
        Provider::OpenRouter => openrouter::fetch(client, auth, endpoints.openrouter).await,
        Provider::Antigravity => antigravity::fetch(client, auth, endpoints.antigravity).await,
        Provider::Cursor => {
            cursor::fetch(client, auth, endpoints.cursor_api, endpoints.cursor_web).await
        }
        Provider::Other(_) => Err(HttpError::InvalidPayload),
    }
}

fn refresh_token(auth: &AuthRecord) -> Option<&str> {
    auth.refresh_token
        .as_deref()
        .filter(|token| !token.is_empty() && *token != "__remote__")
}

async fn refresh(
    client: &Client,
    provider: &Provider,
    auth: &mut AuthRecord,
    endpoints: &Endpoints<'_>,
) -> Result<()> {
    let token = refresh_token(auth).ok_or_else(|| {
        eyre!("OAuth token needs refresh but no local refresh token is available")
    })?;
    let request = match provider {
        Provider::Codex => client.post(endpoints.codex_token).form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", token),
            ("client_id", auth.default_oauth_client_id()),
        ]),
        Provider::Claude => client
            .post(endpoints.claude_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header(
                USER_AGENT,
                "anthropic-sdk-typescript/0.112.1 userOAuthProvider",
            )
            .json(&json!({
                "grant_type": "refresh_token", "refresh_token": token,
                "client_id": CLAUDE_CLIENT_ID,
            })),
        Provider::Antigravity => client.post(endpoints.antigravity_token).form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", token),
            ("client_id", ANTIGRAVITY_CLIENT_ID),
            ("client_secret", ANTIGRAVITY_CLIENT_SECRET),
        ]),
        Provider::Cursor => {
            let mut bearer = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| eyre!("invalid OAuth refresh credential"))?;
            bearer.set_sensitive(true);
            client
                .post(endpoints.cursor_token)
                .header(AUTHORIZATION, bearer)
                .json(&json!({}))
        }
        Provider::OpenRouter | Provider::Other(_) => {
            return Err(eyre!("unsupported OAuth refresh provider"))
        }
    };
    let payload = request_json(request)
        .await
        .map_err(|error| eyre!("OAuth refresh: {error}"))?;
    let access_key = if *provider == Provider::Cursor {
        "accessToken"
    } else {
        "access_token"
    };
    let access = text(&payload, access_key)
        .filter(|value| *value != "__remote__")
        .ok_or_else(|| eyre!("OAuth refresh returned no usable access token"))?;
    if *provider == Provider::Codex {
        apply_refresh_payload(auth, &payload)
            .map_err(|_| eyre!("OAuth refresh returned incomplete credentials"))?;
    } else {
        access.clone_into(&mut auth.access_token);
        let refresh_key = if *provider == Provider::Cursor {
            "refreshToken"
        } else {
            "refresh_token"
        };
        if let Some(token) = text(&payload, refresh_key).filter(|value| *value != "__remote__") {
            auth.refresh_token = Some(token.to_owned());
        }
        if let Some(token) = text(&payload, "id_token") {
            auth.id_token = Some(token.to_owned());
        }
    }
    // Do not retain the expired timestamp when a refresh omits its lifetime.
    auth.expires_at = if *provider == Provider::Cursor {
        token_expiry(&auth.access_token)
    } else {
        payload
            .get("expires_in")
            .and_then(integer)
            .filter(|seconds| *seconds >= 0)
            .and_then(|seconds| now_ms()?.checked_add(seconds.checked_mul(1_000)?))
    };
    auth.enrich_from_tokens(provider);
    Ok(())
}

#[derive(Debug)]
enum HttpError {
    Status(StatusCode),
    Timeout,
    Transport,
    InvalidCredential,
    Oversized,
    InvalidJson,
    InvalidPayload,
}

impl fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => write!(formatter, "endpoint returned HTTP {}", status.as_u16()),
            Self::Timeout => formatter.write_str("request timed out"),
            Self::Transport => formatter.write_str("could not contact endpoint"),
            Self::InvalidCredential => {
                formatter.write_str("credential cannot be used as an HTTP header")
            }
            Self::Oversized => formatter.write_str("response exceeded the size limit"),
            Self::InvalidJson => formatter.write_str("endpoint returned invalid JSON"),
            Self::InvalidPayload => {
                formatter.write_str("endpoint did not report recognized usage data")
            }
        }
    }
}

fn transport_error(error: &reqwest::Error) -> HttpError {
    if error.is_timeout() {
        HttpError::Timeout
    } else {
        HttpError::Transport
    }
}

fn authorized(
    request: RequestBuilder,
    auth: &AuthRecord,
    user_agent: &'static str,
) -> std::result::Result<RequestBuilder, HttpError> {
    let mut bearer = HeaderValue::from_str(&format!("Bearer {}", auth.access_token))
        .map_err(|_| HttpError::InvalidCredential)?;
    bearer.set_sensitive(true);
    Ok(request
        .header(AUTHORIZATION, bearer)
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, user_agent))
}

async fn request_json(request: RequestBuilder) -> std::result::Result<Value, HttpError> {
    tokio::time::timeout(REQUEST_TIMEOUT, async {
        let mut response = request
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| transport_error(&error))?;
        if !response.status().is_success() {
            return Err(HttpError::Status(response.status()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > 1_048_576)
        {
            return Err(HttpError::Oversized);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| transport_error(&error))?
        {
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err(HttpError::Oversized);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| HttpError::InvalidJson)
    })
    .await
    .map_err(|_| HttpError::Timeout)?
}

fn now_ms() -> Option<i64> {
    now_millis().and_then(millis)
}

fn millis(value: Millis) -> Option<i64> {
    i64::try_from(value.get()).ok()
}

fn timestamp(value: &Value) -> Option<Millis> {
    parse_timestamp_to_ms(value)
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|number| number.is_finite())
}

fn nonnegative(value: &Value) -> Option<f64> {
    number(value).filter(|number| *number >= 0.0)
}

fn amount(value: &Value) -> Option<CreditAmount> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())
        .map(CreditAmount::Integer)
        .or_else(|| number(value).map(CreditAmount::Decimal))
}

fn nonnegative_amount(value: &Value) -> Option<CreditAmount> {
    amount(value).filter(|amount| amount.as_f64() >= 0.0)
}

fn integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)?
        .as_str()
        .filter(|text| !text.trim().is_empty())
}

fn percent_allowance(
    title: String,
    window: Option<AllowanceWindow>,
    percent: Option<f64>,
    resets_at: Option<Millis>,
) -> UsageAllowance {
    let count = percent
        .filter(|percent| percent.is_finite() && *percent >= 0.0)
        .map_or(CreditCount::Unknown, |consumed| CreditCount::Full {
            allocated: CreditAmount::Integer(100),
            consumed: CreditAmount::Decimal(consumed),
        });
    UsageAllowance {
        title,
        window,
        resets_at,
        credits: UsageCredits {
            count,
            unit: CreditUnit::Percentage,
        },
    }
}

fn spending_allowance(
    title: String,
    window: Option<AllowanceWindow>,
    used: Option<CreditAmount>,
    limit: Option<CreditAmount>,
    unit: CreditUnit,
) -> UsageAllowance {
    let count = match (limit, used) {
        (Some(allocated), Some(consumed)) => CreditCount::Full {
            allocated,
            consumed,
        },
        (Some(allocated), None) => CreditCount::Allocated(allocated),
        (_, Some(consumed)) => CreditCount::Consumed(consumed),
        _ => CreditCount::Unknown,
    };
    UsageAllowance {
        title,
        window,
        resets_at: None,
        credits: UsageCredits { count, unit },
    }
}
