use super::{
    amount, authorized, nonnegative, nonnegative_amount, percent_allowance, request_json,
    spending_allowance, text, timestamp, HttpError, USER_AGENT_VALUE,
};
use crate::auth::{cursor_user_id, AuthRecord};
use crate::dashboard::{
    AccountUsage, AllowanceWindow, CredentialKind, CreditAmount, CreditCount, CreditUnit, Currency,
    UsageAllowance, UsageCredits,
};
use crate::time::{next_month, Millis};
use reqwest::header::{HeaderValue, ACCEPT, COOKIE, USER_AGENT};
use reqwest::{Client, StatusCode};
use serde_json::Value;

// Wire shapes and endpoint behavior:
// https://github.com/can1357/oh-my-pi/blob/5964a0f7649275bcde818f20073193fd032451f2/packages/ai/src/usage/cursor.ts
// membershipType and profile name/sub are additionally documented by CursorUsageSummary
// and CursorUserInfo in steipete/CodexBar's CursorStatusProbe.swift.
pub(super) async fn fetch(
    client: &Client,
    auth: &mut AuthRecord,
    api_base: &str,
    web_base: &str,
) -> Result<AccountUsage, HttpError> {
    let user_id = (auth.kind == CredentialKind::OAuth)
        .then(|| cursor_user_id(&auth.access_token))
        .flatten();
    let legacy = async {
        let request = authorized(
            client.get(format!("{}/auth/usage", api_base.trim_end_matches('/'))),
            auth,
            USER_AGENT_VALUE,
        )?;
        request_json(request).await
    };
    let Some(user_id) = user_id else {
        return parse_legacy(&legacy.await?);
    };
    let web = async {
        let cookie = session_cookie(&user_id, &auth.access_token)?;
        let request = |path| {
            client
                .get(format!("{}{path}", web_base.trim_end_matches('/')))
                .header(COOKIE, cookie.clone())
                .header(ACCEPT, "application/json")
                .header(USER_AGENT, USER_AGENT_VALUE)
        };
        Ok::<_, HttpError>(tokio::join!(
            request_json(request("/api/usage-summary")),
            request_json(request("/api/auth/me")),
        ))
    };
    let (legacy, web) = tokio::join!(legacy, web);
    let legacy = legacy.and_then(|payload| parse_legacy(&payload));
    let (summary, profile) = match web {
        Ok(results) => results,
        Err(error) => return legacy.map_err(|legacy_error| preferred_error(legacy_error, error)),
    };
    if let Ok(profile) = &profile {
        enrich_profile(auth, &user_id, profile);
    }
    let summary = summary.and_then(|payload| parse_summary(&payload));
    match (legacy, summary) {
        (Ok(mut legacy), Ok(summary)) => {
            legacy.limits.allowances.extend(summary.limits.allowances);
            legacy.limits.global_reset_at = summary
                .limits
                .global_reset_at
                .or(legacy.limits.global_reset_at);
            legacy.plan = summary.plan.or(legacy.plan);
            Ok(legacy)
        }
        (Ok(usage), Err(_)) | (Err(_), Ok(usage)) => Ok(usage),
        (Err(legacy), Err(summary)) => {
            let error = preferred_error(legacy, summary);
            Err(match profile {
                Ok(_) => error,
                Err(profile) => preferred_error(error, profile),
            })
        }
    }
}

fn preferred_error(first: HttpError, second: HttpError) -> HttpError {
    let auth_error = |error: &HttpError| {
        matches!(
            error,
            HttpError::InvalidCredential
                | HttpError::Status(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
        )
    };
    if (auth_error(&second) && !auth_error(&first))
        || (!auth_error(&first) && matches!(first, HttpError::InvalidPayload))
    {
        second
    } else {
        first
    }
}

fn session_cookie(user_id: &str, token: &str) -> Result<HeaderValue, HttpError> {
    // Match encodeURIComponent, not form encoding (spaces must be %20, never '+').
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut value = String::from("WorkosCursorSessionToken=");
    for byte in user_id
        .bytes()
        .chain(b"::".iter().copied())
        .chain(token.bytes())
    {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            value.push(char::from(byte));
        } else {
            value.push('%');
            value.push(char::from(HEX[usize::from(byte >> 4)]));
            value.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    let mut header = HeaderValue::from_str(&value).map_err(|_| HttpError::InvalidCredential)?;
    header.set_sensitive(true);
    Ok(header)
}

fn enrich_profile(auth: &mut AuthRecord, user_id: &str, payload: &Value) {
    if payload.get("sub").and_then(Value::as_str) != Some(user_id) {
        return;
    }
    auth.account_id = Some(user_id.to_owned());
    if let Some(email) = text(payload, "email") {
        auth.email = Some(email.trim().to_owned());
    }
    if let Some(name) = text(payload, "name") {
        auth.name = Some(name.trim().to_owned());
    }
}

fn resets_at(payload: &Value) -> Option<Millis> {
    ["billingCycleEnd", "endOfMonth", "resetsAt", "nextReset"]
        .into_iter()
        .find_map(|key| payload.get(key).and_then(timestamp))
        .or_else(|| {
            ["startOfMonth", "billingCycleStart", "startOfBillingCycle"]
                .into_iter()
                .find_map(|key| payload.get(key).and_then(timestamp).and_then(next_month))
        })
}

fn parse_legacy(payload: &Value) -> Result<AccountUsage, HttpError> {
    let buckets = payload.as_object().ok_or(HttpError::InvalidPayload)?;
    let mut usage = AccountUsage::default();
    usage.limits.global_reset_at = resets_at(payload);
    for (key, bucket) in buckets {
        let used = ["numRequests", "used", "amountUsed", "usdUsed"]
            .into_iter()
            .find_map(|field| {
                bucket
                    .get(field)
                    .and_then(nonnegative_amount)
                    .map(|used| (field, used))
            });
        let limit = ["maxRequestUsage", "limit", "amountLimit", "usdLimit"]
            .into_iter()
            .find_map(|field| {
                bucket
                    .get(field)
                    .and_then(nonnegative_amount)
                    .map(|limit| (field, limit))
            });
        if used.is_none() && limit.is_none() {
            continue;
        }
        let normalized = key.to_ascii_lowercase();
        let monetary = used.is_some_and(|(field, _)| field == "usdUsed")
            || limit.is_some_and(|(field, _)| field == "usdLimit")
            || key == "planUsage"
            || ["usd", "billing", "stripe"]
                .iter()
                .any(|part| normalized.contains(part));
        let (label, unit) = if monetary {
            (format!("{key} spend"), CreditUnit::Currency(Currency::Usd))
        } else {
            // The shared model has no request unit. Do not turn requests into generic credits.
            (format!("{key} requests"), CreditUnit::Unknown)
        };
        let mut allowance = spending_allowance(
            label,
            Some(AllowanceWindow::Monthly),
            used.map(|(_, amount)| amount),
            limit.map(|(_, amount)| amount),
            unit,
        );
        allowance.resets_at = usage.limits.global_reset_at;
        usage.limits.allowances.push(allowance);
    }
    if usage.limits.allowances.is_empty() && usage.limits.global_reset_at.is_none() {
        Err(HttpError::InvalidPayload)
    } else {
        Ok(usage)
    }
}

fn dollars(cents: CreditAmount) -> CreditAmount {
    match cents {
        CreditAmount::Integer(value) if value % 100 == 0 => CreditAmount::Integer(value / 100),
        value => CreditAmount::Decimal(value.as_f64() / 100.0),
    }
}

fn cents_bucket(bucket: &Value, title: &str, reset: Option<Millis>) -> Option<UsageAllowance> {
    if !bucket.is_object() || bucket.get("enabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let reported_used = bucket.get("used").and_then(nonnegative_amount);
    let remaining = bucket.get("remaining").and_then(amount);
    let limit = match bucket.get("limit") {
        None | Some(Value::Null) => None,
        Some(value) => Some(nonnegative_amount(value).filter(|limit| limit.as_f64() > 0.0)?),
    };
    let used = reported_used
        .filter(|used| used.as_f64() > 0.0)
        .or_else(|| {
            // Subtract in the exact integer domain before converting cents to dollars.
            let inferred = limit.zip(remaining).and_then(|(allocated, remaining)| {
                UsageCredits {
                    count: CreditCount::Full {
                        allocated,
                        consumed: remaining,
                    },
                    unit: CreditUnit::Currency(Currency::Usd),
                }
                .remaining()
            });
            inferred
                .filter(|used| used.as_f64() > 0.0)
                .or(reported_used)
                .or(inferred)
        });
    let count = match (used, limit, remaining) {
        (Some(used), Some(limit), _) => CreditCount::Full {
            allocated: dollars(limit),
            consumed: dollars(used),
        },
        (Some(used), None, _) => CreditCount::Consumed(dollars(used)),
        (None, _, Some(remaining)) => CreditCount::Remaining(dollars(remaining)),
        (None, Some(limit), None) => CreditCount::Allocated(dollars(limit)),
        (None, None, None) => return None,
    };
    Some(UsageAllowance {
        title: title.to_owned(),
        window: Some(AllowanceWindow::Monthly),
        resets_at: reset,
        credits: UsageCredits {
            count,
            unit: CreditUnit::Currency(Currency::Usd),
        },
    })
}

fn plan_percent(
    bucket: &Value,
    title: &str,
    percent: f64,
    with_limit: bool,
    reset: Option<Millis>,
) -> UsageAllowance {
    let limit = with_limit
        .then(|| bucket.get("limit").and_then(nonnegative_amount))
        .flatten()
        .filter(|limit| limit.as_f64() > 0.0)
        .map(dollars);
    if let Some(limit) = limit {
        let used = limit.as_f64() * (percent / 100.0);
        if used.is_finite() {
            let mut allowance = spending_allowance(
                title.to_owned(),
                Some(AllowanceWindow::Monthly),
                Some(CreditAmount::Decimal(used)),
                Some(limit),
                CreditUnit::Currency(Currency::Usd),
            );
            allowance.resets_at = reset;
            return allowance;
        }
    }
    percent_allowance(
        title.to_owned(),
        Some(AllowanceWindow::Monthly),
        Some(percent),
        reset,
    )
}

fn push_plan(allowances: &mut Vec<UsageAllowance>, bucket: &Value, reset: Option<Millis>) {
    if bucket.get("enabled").and_then(Value::as_bool) == Some(false) {
        return;
    }
    let auto = bucket.get("autoPercentUsed").and_then(nonnegative);
    let api = bucket.get("apiPercentUsed").and_then(nonnegative);
    for (title, percent, with_limit) in
        [("Cursor Models", auto, false), ("Other Models", api, true)]
    {
        if let Some(percent) = percent {
            allowances.push(plan_percent(bucket, title, percent, with_limit, reset));
        }
    }
    if auto.is_none() && api.is_none() {
        if let Some(percent) = bucket.get("totalPercentUsed").and_then(nonnegative) {
            allowances.push(plan_percent(bucket, "Personal Usage", percent, true, reset));
        } else if let Some(allowance) = cents_bucket(bucket, "Personal Usage", reset) {
            allowances.push(allowance);
        }
    }
}

fn parse_summary(payload: &Value) -> Result<AccountUsage, HttpError> {
    let mut usage = AccountUsage {
        plan: text(payload, "membershipType").map(|plan| plan.trim().to_owned()),
        ..AccountUsage::default()
    };
    let reset = resets_at(payload);
    usage.limits.global_reset_at = reset;
    if let Some(individual) = payload
        .get("individualUsage")
        .filter(|value| value.is_object())
    {
        let overall = individual
            .get("overall")
            .and_then(|bucket| cents_bucket(bucket, "Personal Usage", reset));
        if let Some(overall) = overall {
            usage.limits.allowances.push(overall);
        } else if let Some(plan) = individual.get("plan") {
            push_plan(&mut usage.limits.allowances, plan, reset);
        }
        if let Some(on_demand) = individual
            .get("onDemand")
            .and_then(|bucket| cents_bucket(bucket, "On-Demand Usage", reset))
        {
            usage.limits.allowances.push(on_demand);
        }
    }
    if usage.limits.allowances.is_empty()
        && usage.plan.is_none()
        && usage.limits.global_reset_at.is_none()
    {
        Err(HttpError::InvalidPayload)
    } else {
        Ok(usage)
    }
}

#[cfg(test)]
mod tests {
    use super::{fetch, parse_legacy, parse_summary, session_cookie};
    use crate::api::tests::{client, join, read_request, response, serve};
    use crate::auth::AuthRecord;
    use crate::dashboard::{CredentialKind, CreditAmount, CreditCount, CreditUnit, Currency};
    use crate::time::parse_timestamp_to_ms;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use eyre::{eyre, Result};
    use serde_json::json;
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    fn oauth() -> AuthRecord {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"workos|synthetic-user"}"#);
        AuthRecord::new(format!("header.{payload}.signature"), CredentialKind::OAuth)
    }

    #[test]
    fn reset_only_reports_preserve_wall_clock_without_inventing_usage() -> Result<()> {
        let payload = json!({"billingCycleEnd":"2026-10-01T00:00:00Z"});
        for parsed in [parse_legacy(&payload), parse_summary(&payload)] {
            let usage = parsed.map_err(|error| eyre!("{error}"))?;
            assert_eq!(
                usage.limits.global_reset_at,
                parse_timestamp_to_ms(&json!("2026-10-01T00:00:00Z"))
            );
            assert!(usage.limits.allowances.is_empty());
        }
        Ok(())
    }

    fn serve_web(
        summary: String,
        profile: String,
    ) -> Result<(String, JoinHandle<Result<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let base = format!("http://{}", listener.local_addr()?);
        listener.set_nonblocking(true)?;
        let server = thread::spawn(move || {
            let mut routes = [
                ("/api/usage-summary", Some(summary)),
                ("/api/auth/me", Some(profile)),
            ];
            let mut requests = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(10);
            for _ in 0..routes.len() {
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
                let request = read_request(&mut stream)?;
                let path = request
                    .split_ascii_whitespace()
                    .nth(1)
                    .ok_or_else(|| eyre!("fixture request lacks a path"))?;
                let reply = routes
                    .iter_mut()
                    .find(|(route, _)| *route == path)
                    .and_then(|(_, reply)| reply.take())
                    .ok_or_else(|| eyre!("unexpected or repeated fixture path"))?;
                stream.set_write_timeout(Some(Duration::from_secs(3)))?;
                stream.write_all(reply.as_bytes())?;
                requests.push(request);
            }
            Ok(requests)
        });
        Ok((base, server))
    }

    #[test]
    fn legacy_partial_usage_keeps_exact_request_counts_and_currency() -> Result<()> {
        let usage = parse_legacy(&json!({
            "gpt-4": {"numRequests": "9007199254740993", "maxRequestUsage": "9007199254740995"},
            "uncapped": {"used": 0, "limit": null},
            "planUsage": {"amountUsed": 15.5, "amountLimit": 20},
            "custom": {"usdUsed": 3, "usdLimit": 2}
        }))
        .map_err(|error| eyre!(error))?;
        let find = |title| {
            usage
                .limits
                .allowances
                .iter()
                .find(|item| item.title == title)
                .ok_or_else(|| eyre!("missing allowance {title}"))
        };
        let requests = &find("gpt-4 requests")?.credits;
        assert_eq!(requests.remaining(), Some(CreditAmount::Integer(2)));
        assert_eq!(
            requests.consumed(),
            Some(CreditAmount::Integer(9_007_199_254_740_993))
        );
        assert_eq!(requests.unit, CreditUnit::Unknown);
        let uncapped = &find("uncapped requests")?.credits;
        assert_eq!(
            uncapped.count,
            CreditCount::Consumed(CreditAmount::Integer(0))
        );
        assert_eq!(uncapped.used_percent(), None);
        assert_eq!(
            find("planUsage spend")?.credits.unit,
            CreditUnit::Currency(Currency::Usd)
        );
        assert_eq!(
            find("custom spend")?
                .credits
                .remaining()
                .map(CreditAmount::as_f64),
            Some(-1.0)
        );
        Ok(())
    }

    #[test]
    fn overall_precedes_plan_without_using_team_spending() -> Result<()> {
        let usage = parse_summary(&json!({
            "individualUsage": {
                "overall": {"used": "9000", "limit": "10000", "remaining": "1000"},
                "plan": {"autoPercentUsed": 12, "apiPercentUsed": 30, "limit": 2000}
            },
            "teamUsage": {"onDemand": {"used": 900_000, "limit": 1_000_000}}
        }))
        .map_err(|error| eyre!(error))?;
        assert_eq!(usage.limits.allowances.len(), 1);
        let credits = &usage.limits.allowances[0].credits;
        assert_eq!(credits.consumed(), Some(CreditAmount::Integer(90)));
        assert_eq!(credits.remaining(), Some(CreditAmount::Integer(10)));
        Ok(())
    }

    #[test]
    fn unusable_overall_falls_back_to_independent_plan_percentages() -> Result<()> {
        let usage = parse_summary(&json!({"individualUsage": {
            "overall": {"used": 0, "limit": 0},
            "plan": {"used": 1504, "limit": 7000, "autoPercentUsed": 1.85,
                "apiPercentUsed": 125, "totalPercentUsed": 1.63}
        }}))
        .map_err(|error| eyre!(error))?;
        assert_eq!(usage.limits.allowances.len(), 2);
        let cursor = &usage.limits.allowances[0];
        assert_eq!(cursor.title, "Cursor Models");
        assert_eq!(cursor.credits.unit, CreditUnit::Percentage);
        assert!(
            (cursor
                .credits
                .used_percent()
                .ok_or_else(|| eyre!("missing percentage"))?
                - 1.85)
                .abs()
                < 1e-10
        );
        let other = &usage.limits.allowances[1];
        assert_eq!(other.title, "Other Models");
        assert_eq!(other.credits.unit, CreditUnit::Currency(Currency::Usd));
        assert_eq!(other.credits.allocated(), Some(CreditAmount::Integer(70)));
        assert_eq!(
            other.credits.remaining().map(CreditAmount::as_f64),
            Some(-17.5)
        );
        Ok(())
    }

    #[test]
    fn unknown_dollar_allocation_retains_reported_percentage_without_inventing_spend() -> Result<()>
    {
        let usage = parse_summary(&json!({"individualUsage": {
            "plan": {"apiPercentUsed": 0, "limit": null}
        }}))
        .map_err(|error| eyre!(error))?;
        let credits = &usage.limits.allowances[0].credits;
        assert_eq!(credits.unit, CreditUnit::Percentage);
        assert_eq!(credits.used_percent(), Some(0.0));
        Ok(())
    }

    #[test]
    fn total_percentage_precedes_old_cents_and_cents_infer_nonzero_spend() -> Result<()> {
        let total = parse_summary(&json!({"individualUsage": {
            "plan": {"totalPercentUsed": 25, "used": 0, "limit": 2000}
        }}))
        .map_err(|error| eyre!(error))?;
        assert_eq!(
            total.limits.allowances[0]
                .credits
                .consumed()
                .map(CreditAmount::as_f64),
            Some(5.0)
        );
        let old = parse_summary(&json!({"individualUsage": {
            "plan": {"used": 0, "limit": 5000, "remaining": 1500}
        }}))
        .map_err(|error| eyre!(error))?;
        assert_eq!(
            old.limits.allowances[0].credits.consumed(),
            Some(CreditAmount::Integer(35))
        );
        Ok(())
    }

    #[test]
    fn legacy_limit_only_keeps_allocation_without_inventing_consumption() -> Result<()> {
        let usage =
            parse_legacy(&json!({"custom": {"usdLimit": 20}})).map_err(|error| eyre!(error))?;
        let credits = &usage.limits.allowances[0].credits;
        assert_eq!(
            credits.count,
            CreditCount::Allocated(CreditAmount::Integer(20))
        );
        assert_eq!(credits.unit, CreditUnit::Currency(Currency::Usd));
        assert_eq!(credits.consumed(), None);
        assert_eq!(credits.used_percent(), None);
        Ok(())
    }

    #[test]
    fn positive_reported_spend_precedes_inconsistent_remaining() -> Result<()> {
        let usage = parse_summary(&json!({"individualUsage": {
            "overall": {"used": 100, "limit": 1000, "remaining": 0}
        }}))
        .map_err(|error| eyre!(error))?;
        assert_eq!(
            usage.limits.allowances[0].credits.remaining(),
            Some(CreditAmount::Integer(9))
        );
        Ok(())
    }

    #[test]
    fn on_demand_retains_signed_remaining_without_a_plan_or_limit() -> Result<()> {
        let usage = parse_summary(&json!({"individualUsage": {
            "plan": {"enabled": false, "autoPercentUsed": 12},
            "onDemand": {"remaining": -1250, "limit": null}
        }}))
        .map_err(|error| eyre!(error))?;
        assert_eq!(usage.limits.allowances.len(), 1);
        let allowance = &usage.limits.allowances[0];
        assert_eq!(allowance.title, "On-Demand Usage");
        assert_eq!(
            allowance.credits.count,
            CreditCount::Remaining(CreditAmount::Decimal(-12.5))
        );
        assert_eq!(allowance.credits.consumed(), None);
        assert_eq!(allowance.credits.used_percent(), None);
        let consumed = parse_summary(&json!({"individualUsage": {
            "onDemand": {"used": 2500}
        }}))
        .map_err(|error| eyre!(error))?;
        assert_eq!(
            consumed.limits.allowances[0].credits.count,
            CreditCount::Consumed(CreditAmount::Integer(25))
        );
        Ok(())
    }

    #[test]
    fn missing_or_disabled_usage_never_becomes_zero_spending() -> Result<()> {
        for payload in [
            json!({"individualUsage": {"plan": {"enabled": false, "autoPercentUsed": 0}}}),
            json!({"individualUsage": {"overall": {"used": "bad", "limit": null}}}),
        ] {
            assert!(parse_summary(&payload).is_err());
        }
        let allocated = parse_summary(&json!({"individualUsage": {"plan": {"limit": 2000}}}))
            .map_err(|error| eyre!(error))?;
        let credits = &allocated.limits.allowances[0].credits;
        assert_eq!(
            credits.count,
            CreditCount::Allocated(CreditAmount::Integer(20))
        );
        assert_eq!(credits.consumed(), None);
        assert_eq!(credits.remaining(), None);
        assert_eq!(credits.used_percent(), None);
        Ok(())
    }

    #[test]
    fn billing_end_wins_and_start_uses_calendar_month_not_fixed_duration() -> Result<()> {
        let direct = parse_legacy(&json!({"gpt-4": {"used": 1},
            "billingCycleEnd": "2026-08-20T00:00:00Z", "startOfMonth": "2026-07-01T00:00:00Z"
        }))
        .map_err(|error| eyre!(error))?;
        assert_eq!(
            direct.limits.global_reset_at,
            parse_timestamp_to_ms(&json!("2026-08-20T00:00:00Z"))
        );
        let derived = parse_legacy(&json!({"gpt-4": {"used": 1},
            "startOfMonth": "2026-01-31T12:34:56.789Z"
        }))
        .map_err(|error| eyre!(error))?;
        assert_eq!(
            derived.limits.global_reset_at,
            parse_timestamp_to_ms(&json!("2026-03-03T12:34:56.789Z"))
        );
        Ok(())
    }

    #[test]
    fn summary_keeps_only_evidenced_membership_metadata_without_usage() -> Result<()> {
        let usage = parse_summary(&json!({"membershipType": " pro ", "plan": "invented"}))
            .map_err(|error| eyre!(error))?;
        assert_eq!(usage.plan.as_deref(), Some("pro"));
        assert!(usage.limits.allowances.is_empty());
        assert!(parse_summary(&json!({"plan": "invented", "planType": "invented"})).is_err());
        Ok(())
    }

    #[test]
    fn workos_cookie_matches_uri_component_encoding_and_is_sensitive() -> Result<()> {
        let cookie = session_cookie("u +/é", "a.b!~*'().c").map_err(|error| eyre!(error))?;
        assert!(cookie.is_sensitive());
        assert_eq!(
            cookie.to_str()?,
            "WorkosCursorSessionToken=u%20%2B%2F%C3%A9%3A%3Aa.b!~*'().c"
        );
        Ok(())
    }

    #[tokio::test]
    async fn oauth_fetch_merges_pinned_paths_and_enriches_verified_profile() -> Result<()> {
        let (api, api_server) = serve(vec![response(200, &json!({"gpt-4": {"used": 3}}))])?;
        let (web, web_server) = serve_web(
            response(
                200,
                &json!({"membershipType": "pro", "individualUsage": {
                    "overall": {"used": 2000, "limit": 10000}
                }}),
            ),
            response(
                200,
                &json!({"sub": "synthetic-user", "email": " person@example.invalid ", "name": " Person "}),
            ),
        )?;
        let mut auth = oauth();
        let usage = fetch(&client()?, &mut auth, &api, &web)
            .await
            .map_err(|error| eyre!(error))?;
        assert_eq!(usage.limits.allowances.len(), 2);
        assert_eq!(usage.plan.as_deref(), Some("pro"));
        assert_eq!(auth.email.as_deref(), Some("person@example.invalid"));
        assert_eq!(auth.name.as_deref(), Some("Person"));
        assert_eq!(auth.account_id.as_deref(), Some("synthetic-user"));
        let api_requests = join(api_server)?;
        assert!(api_requests[0].starts_with("GET /auth/usage HTTP/1.1"));
        assert!(
            api_requests[0].contains(&format!("authorization: Bearer {}\r\n", auth.access_token))
        );
        assert!(!api_requests[0].contains("cookie:"));
        let web_requests = join(web_server)?;
        assert!(web_requests
            .iter()
            .any(|request| request.starts_with("GET /api/usage-summary HTTP/1.1")));
        assert!(web_requests
            .iter()
            .any(|request| request.starts_with("GET /api/auth/me HTTP/1.1")));
        for request in web_requests {
            assert!(request.contains(&format!(
                "cookie: WorkosCursorSessionToken=synthetic-user%3A%3A{}\r\n",
                auth.access_token
            )));
            assert!(!request.contains("authorization:"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn api_keys_never_request_web_session_endpoints() -> Result<()> {
        let (api, server) = serve(vec![response(200, &json!({"gpt-4": {"used": 7}}))])?;
        let mut auth = AuthRecord::new(oauth().access_token, CredentialKind::ApiKey);
        let usage = fetch(&client()?, &mut auth, &api, "invalid-web-origin")
            .await
            .map_err(|error| eyre!(error))?;
        assert_eq!(
            usage.limits.allowances[0].credits.consumed(),
            Some(CreditAmount::Integer(7))
        );
        let requests = join(server)?;
        assert!(requests[0].starts_with("GET /auth/usage HTTP/1.1"));
        Ok(())
    }

    #[tokio::test]
    async fn optional_failures_and_mismatched_profile_do_not_hide_legacy_usage() -> Result<()> {
        let (api, api_server) = serve(vec![response(200, &json!({"gpt-4": {"used": 3}}))])?;
        let (web, web_server) = serve_web(
            response(503, &json!({})),
            response(
                200,
                &json!({
                    "sub": "other-user", "email": "other@example.invalid", "name": "Other"
                }),
            ),
        )?;
        let mut auth = oauth();
        auth.email = Some("stored@example.invalid".to_owned());
        auth.name = Some("Stored".to_owned());
        auth.account_id = Some("stored-id".to_owned());
        let usage = fetch(&client()?, &mut auth, &api, &web)
            .await
            .map_err(|error| eyre!(error))?;
        assert_eq!(
            usage.limits.allowances[0].credits.consumed(),
            Some(CreditAmount::Integer(3))
        );
        assert_eq!(auth.email.as_deref(), Some("stored@example.invalid"));
        assert_eq!(auth.name.as_deref(), Some("Stored"));
        assert_eq!(auth.account_id.as_deref(), Some("stored-id"));
        join(api_server)?;
        join(web_server)?;
        Ok(())
    }

    #[tokio::test]
    async fn summary_survives_legacy_auth_failure_and_profile_failure() -> Result<()> {
        let (api, api_server) = serve(vec![response(401, &json!({}))])?;
        let (web, web_server) = serve_web(
            response(200, &json!({"membershipType": "enterprise"})),
            response(503, &json!({})),
        )?;
        let usage = fetch(&client()?, &mut oauth(), &api, &web)
            .await
            .map_err(|error| eyre!(error))?;
        assert_eq!(usage.plan.as_deref(), Some("enterprise"));
        join(api_server)?;
        join(web_server)?;
        Ok(())
    }

    #[tokio::test]
    async fn all_usage_failures_keep_auth_error_and_verified_identity() -> Result<()> {
        let (api, api_server) = serve(vec![response(503, &json!({}))])?;
        let (web, web_server) = serve_web(
            response(401, &json!({})),
            response(200, &json!({"sub": "synthetic-user", "name": "Person"})),
        )?;
        let mut auth = oauth();
        let result = fetch(&client()?, &mut auth, &api, &web).await;
        assert!(matches!(
            result,
            Err(super::HttpError::Status(reqwest::StatusCode::UNAUTHORIZED))
        ));
        assert_eq!(auth.name.as_deref(), Some("Person"));
        assert_eq!(auth.account_id.as_deref(), Some("synthetic-user"));
        join(api_server)?;
        join(web_server)?;
        Ok(())
    }
}
