use super::{fetch_account, fetch_account_at, request_json, HttpError, MAX_RESPONSE_BYTES};
use crate::auth::AuthRecord;
use crate::dashboard::{CredentialKind, Provider};
use eyre::{eyre, Result};
use reqwest::Client;
use serde_json::{json, Value};
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

fn client() -> Result<Client> {
    Ok(Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

fn auth(kind: CredentialKind) -> AuthRecord {
    AuthRecord::new("synthetic-access".to_owned(), kind)
}

fn response(status: u16, body: &Value) -> String {
    let body = body.to_string();
    format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

fn read_request(stream: &mut TcpStream) -> Result<String> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 4_096];
    loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Err(eyre!("fixture request ended prematurely"));
        }
        bytes.extend_from_slice(
            chunk
                .get(..count)
                .ok_or_else(|| eyre!("invalid read length"))?,
        );
        if bytes.len() > 65_536 {
            return Err(eyre!("fixture request too large"));
        }
        let text = String::from_utf8_lossy(&bytes);
        if let Some((headers, body)) = text.split_once("\r\n\r\n") {
            let length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map_or(Ok(0), |(_, value)| value.trim().parse::<usize>())?;
            if body.len() >= length {
                return Ok(text.into_owned());
            }
        }
    }
}

fn serve(replies: Vec<String>) -> Result<(String, JoinHandle<Result<Vec<String>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let base = format!("http://{}", listener.local_addr()?);
    listener.set_nonblocking(true)?;
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        for reply in replies {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            requests.push(read_request(&mut stream)?);
            stream.set_write_timeout(Some(Duration::from_secs(3)))?;
            stream.write_all(reply.as_bytes())?;
        }
        Ok(requests)
    });
    Ok((base, server))
}

fn join(server: JoinHandle<Result<Vec<String>>>) -> Result<Vec<String>> {
    server
        .join()
        .map_err(|_| eyre!("fixture server panicked"))?
}

#[tokio::test]
async fn unsupported_credentials_and_remote_refresh_never_contact_endpoint() -> Result<()> {
    let client = client()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let base = format!("http://{}", listener.local_addr()?);
    for (provider, kind) in [
        (Provider::Codex, CredentialKind::ApiKey),
        (Provider::Claude, CredentialKind::ApiKey),
        (Provider::Antigravity, CredentialKind::ApiKey),
        (Provider::OpenRouter, CredentialKind::OAuth),
        (
            Provider::Other("unknown".to_owned()),
            CredentialKind::ApiKey,
        ),
    ] {
        let error = fetch_account_at(&client, &provider, &mut auth(kind), &base)
            .await
            .err()
            .ok_or_else(|| eyre!("unsupported credential was accepted"))?;
        assert!(error.to_string().starts_with("unsupported"));
    }
    let mut remote = auth(CredentialKind::OAuth);
    remote.expires_at = Some(0);
    remote.refresh_token = Some("__remote__".to_owned());
    assert!(
        fetch_account_at(&client, &Provider::Claude, &mut remote, &base)
            .await
            .is_err()
    );
    remote.access_token = "__remote__".to_owned();
    assert!(
        fetch_account_at(&client, &Provider::Codex, &mut remote, &base)
            .await
            .is_err()
    );
    assert!(fetch_account_at(
        &client,
        &Provider::Antigravity,
        &mut auth(CredentialKind::OAuth),
        &base
    )
    .await
    .is_err());
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    Ok(())
}

#[tokio::test]
async fn rejected_claude_token_refreshes_once_and_rotation_survives_usage_failure() -> Result<()> {
    let (base, server) = serve(vec![
        response(401, &json!({"error":"synthetic-private-response"})),
        response(
            200,
            &json!({"access_token":"synthetic-rotated-access","refresh_token":"synthetic-rotated-refresh","expires_in":3600}),
        ),
        response(401, &json!({"error":"synthetic-private-response"})),
    ])?;
    let mut auth = auth(CredentialKind::OAuth);
    auth.refresh_token = Some("synthetic-refresh".to_owned());
    let prior = auth.clone();
    let error = fetch_account_at(&client()?, &Provider::Claude, &mut auth, &base)
        .await
        .err()
        .ok_or_else(|| eyre!("failed account unexpectedly succeeded"))?;
    assert!(error.to_string().contains("HTTP 401"));
    assert!(!format!("{error:?}").contains("synthetic"));
    assert_eq!(auth.access_token, "synthetic-rotated-access");
    assert_eq!(
        auth.refresh_token.as_deref(),
        Some("synthetic-rotated-refresh")
    );
    assert!(auth.needs_persisted_refresh(&prior));
    let requests = join(server)?;
    let first = requests
        .first()
        .ok_or_else(|| eyre!("missing usage probe"))?;
    assert!(first.starts_with("GET /api/oauth/usage "));
    assert!(first.contains("oauth-2025-04-20"));
    let refresh = requests.get(1).ok_or_else(|| eyre!("missing refresh"))?;
    assert!(refresh.starts_with("POST /oauth/token "));
    let payload: Value = serde_json::from_str(
        refresh
            .split_once("\r\n\r\n")
            .ok_or_else(|| eyre!("missing body"))?
            .1,
    )?;
    assert_eq!(
        payload.get("client_id"),
        Some(&json!(super::CLAUDE_CLIENT_ID))
    );
    assert_eq!(payload.get("grant_type"), Some(&json!("refresh_token")));
    assert!(requests
        .last()
        .is_some_and(|request| request.contains("Bearer synthetic-rotated-access")));
    Ok(())
}

#[tokio::test]
async fn expired_codex_token_refreshes_before_usage_and_lists_resets_without_consuming(
) -> Result<()> {
    let (base, server) = serve(vec![
        response(
            200,
            &json!({"access_token":"synthetic-new","refresh_token":"synthetic-new-refresh","expires_in":3600}),
        ),
        response(
            200,
            &json!({"rate_limit":{"primary_window":{"used_percent":13,"limit_window_seconds":18000}},
            "rate_limit_reset_credits":{"available_count":3}}),
        ),
        response(
            200,
            &json!({"available_count":1,"credits":[{"status":"available","expires_at":"2099-02-01T00:00:00Z"}]}),
        ),
    ])?;
    let mut auth = auth(CredentialKind::OAuth);
    auth.refresh_token = Some("synthetic-refresh".to_owned());
    auth.account_id = Some("synthetic-workspace".to_owned());
    auth.expires_at = Some(0);
    let usage = fetch_account_at(&client()?, &Provider::Codex, &mut auth, &base).await?;
    assert!(usage
        .windows
        .first()
        .and_then(|metric| metric.used_percent)
        .is_some_and(|percent| (percent - 13.0).abs() < f64::EPSILON));
    assert_eq!(usage.credits.len(), 1);
    assert!(usage
        .credits
        .first()
        .is_some_and(|credit| credit.expires_at.is_some()));
    let requests = join(server)?;
    assert!(requests
        .first()
        .is_some_and(|request| request.starts_with("POST /oauth/token ")
            && request.contains("grant_type=refresh_token")
            && request.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann")));
    assert!(requests
        .get(1)
        .is_some_and(|request| request.starts_with("GET /wham/usage ")
            && request
                .to_ascii_lowercase()
                .contains("chatgpt-account-id: synthetic-workspace")));
    assert!(requests
        .last()
        .is_some_and(|request| request.starts_with("GET /wham/rate-limit-reset-credits ")));
    Ok(())
}

#[tokio::test]
async fn antigravity_compatibility_fallback_preserves_project_and_refreshes_expiry() -> Result<()> {
    let (base, server) = serve(vec![
        response(
            200,
            &json!({"access_token":"synthetic-google-new","expires_in":3600}),
        ),
        response(404, &json!({})),
        response(
            200,
            &json!({"models":{"gemini":{"modelProvider":"MODEL_PROVIDER_GOOGLE",
            "quotaInfo":{"remainingFraction":0.25,"resetTime":"2026-09-01T00:00:00Z"}}}}),
        ),
    ])?;
    let mut auth = auth(CredentialKind::OAuth);
    auth.project_id = Some("synthetic-project".to_owned());
    auth.refresh_token = Some("synthetic-google-refresh".to_owned());
    auth.expires_at = Some(0);
    let usage = fetch_account_at(&client()?, &Provider::Antigravity, &mut auth, &base).await?;
    assert!(usage
        .windows
        .first()
        .and_then(|metric| metric.used_percent)
        .is_some_and(|percent| (percent - 75.0).abs() < f64::EPSILON));
    assert_eq!(
        auth.refresh_token.as_deref(),
        Some("synthetic-google-refresh")
    );
    let requests = join(server)?;
    assert!(requests
        .first()
        .is_some_and(|request| request.contains("client_secret=")
            && request.contains("grant_type=refresh_token")));
    for request in requests.iter().skip(1) {
        assert!(request.starts_with("POST /v1internal:"));
        let body: Value = serde_json::from_str(
            request
                .split_once("\r\n\r\n")
                .ok_or_else(|| eyre!("missing body"))?
                .1,
        )?;
        assert_eq!(body, json!({"project":"synthetic-project"}));
    }
    assert!(requests
        .last()
        .is_some_and(|request| request.starts_with("POST /v1internal:fetchAvailableModels ")));
    Ok(())
}

#[tokio::test]
async fn openrouter_only_requests_account_credits_for_management_keys() -> Result<()> {
    let (base, server) = serve(vec![response(
        200,
        &json!({"data":{"usage":15,"limit":null,"is_management_key":false}}),
    )])?;
    let usage = fetch_account_at(
        &client()?,
        &Provider::OpenRouter,
        &mut auth(CredentialKind::ApiKey),
        &base,
    )
    .await?;
    assert!(usage.credits.is_empty());
    assert_eq!(join(server)?.len(), 1);
    let (base, server) = serve(vec![
        response(200, &json!({"data":{"usage":0,"is_management_key":true}})),
        response(200, &json!({"data":{"total_credits":100,"total_usage":25}})),
    ])?;
    let usage = fetch_account_at(
        &client()?,
        &Provider::OpenRouter,
        &mut auth(CredentialKind::ApiKey),
        &base,
    )
    .await?;
    assert!(usage
        .credits
        .first()
        .is_some_and(|credit| credit.name == "Account-wide credits"
            && (credit.balance - 75.0).abs() < f64::EPSILON));
    assert!(join(server)?
        .last()
        .is_some_and(|request| request.starts_with("GET /credits ")));
    Ok(())
}

#[tokio::test]
async fn oversized_and_malformed_responses_are_rejected_without_echoing_body() -> Result<()> {
    let client = client()?;
    let (base, server) = serve(vec![format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        MAX_RESPONSE_BYTES + 1
    )])?;
    assert!(matches!(
        request_json(client.get(&base)).await,
        Err(HttpError::Oversized)
    ));
    join(server)?;
    // An unknown/chunked length must be bounded while reading, not only by headers.
    let body = "x".repeat(MAX_RESPONSE_BYTES + 1);
    let (base, server) = serve(vec![format!(
        "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}"
    )])?;
    assert!(matches!(
        request_json(client.get(&base)).await,
        Err(HttpError::Oversized)
    ));
    join(server)?;
    let (base, server) = serve(vec![
        "HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\nprivate-not-json!x"
            .to_owned(),
    ])?;
    let error = request_json(client.get(&base))
        .await
        .err()
        .ok_or_else(|| eyre!("invalid JSON accepted"))?;
    assert!(!format!("{error:?}: {error}").contains("private"));
    assert!(matches!(error, HttpError::InvalidJson));
    join(server)?;
    Ok(())
}

#[tokio::test]
async fn redirects_and_untrusted_codex_destinations_never_receive_credentials() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let target = format!("http://{}", listener.local_addr()?);
    let (base, server) = serve(vec![format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")])?;
    assert!(fetch_account_at(
        &client()?,
        &Provider::Claude,
        &mut auth(CredentialKind::OAuth),
        &base
    )
    .await
    .is_err());
    join(server)?;
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    assert!(fetch_account(
        &client()?,
        &Provider::Codex,
        &mut auth(CredentialKind::OAuth),
        &target
    )
    .await
    .is_err());
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    assert!(super::normalize_base_url("https://chatgpt.com.evil.invalid/backend-api").is_err());
    assert!(super::normalize_base_url("https://chatgpt.com/backend-api?token=synthetic").is_err());
    assert_eq!(
        super::normalize_base_url("https://chatgpt.com/")?,
        "https://chatgpt.com/backend-api"
    );
    Ok(())
}
