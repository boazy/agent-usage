use super::{
    authorized, integer, nonnegative, percent_metric, request_json, spending_metric, text,
    timestamp, HttpError, CLAUDE_USER_AGENT,
};
use crate::auth::AuthRecord;
use crate::dashboard::{AccountUsage, UsageMetric};
use reqwest::Client;
use serde_json::Value;

// Mirrors the actual OAuth usage client, not the ordinary inference API-key header.
const BETA: &str = concat!(
    "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,",
    "redact-thinking-2026-02-12,context-management-2025-06-27,prompt-caching-scope-2026-01-05,",
    "mid-conversation-system-2026-04-07,advanced-tool-use-2025-11-20,effort-2025-11-24,",
    "extended-cache-ttl-2025-04-11"
);

pub(super) async fn fetch(
    client: &Client,
    auth: &AuthRecord,
    endpoint: &str,
) -> Result<AccountUsage, HttpError> {
    let request =
        authorized(client.get(endpoint), auth, CLAUDE_USER_AGENT)?.header("anthropic-beta", BETA);
    parse(&request_json(request).await?)
}

fn bucket(value: &Value, name: &str, field: &str) -> Option<UsageMetric> {
    let percent = value.get(field).and_then(nonnegative);
    let resets_at = value.get("resets_at").and_then(timestamp);
    if percent.is_none() && resets_at.is_none() {
        return None;
    }
    Some(percent_metric(name.to_owned(), percent, resets_at))
}

fn parse(payload: &Value) -> Result<AccountUsage, HttpError> {
    let mut usage = AccountUsage::default();
    let entries = payload.get("limits").and_then(Value::as_array);
    for (key, kind, label) in [
        ("five_hour", "session", "Claude 5 hour"),
        ("seven_day", "weekly_all", "Claude 7 day"),
        ("seven_day_opus", "", "Claude 7 day (Opus)"),
        ("seven_day_sonnet", "", "Claude 7 day (Sonnet)"),
    ] {
        let legacy = payload
            .get(key)
            .and_then(|value| bucket(value, label, "utilization"));
        let current = || {
            entries?.iter().find_map(|entry| {
                (text(entry, "kind") == Some(kind) && !kind.is_empty())
                    .then(|| bucket(entry, label, "percent"))
                    .flatten()
            })
        };
        if let Some(metric) = legacy.or_else(current) {
            usage.windows.push(metric);
        }
    }
    if let Some(entries) = entries {
        for entry in entries {
            // is_active identifies the binding bucket, not whether a quota exists.
            if text(entry, "kind") != Some("weekly_scoped") {
                continue;
            }
            let Some(display) = entry
                .pointer("/scope/model/display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let name = format!("Claude 7 day ({display})");
            if usage
                .windows
                .iter()
                .any(|metric| metric.name.eq_ignore_ascii_case(&name))
            {
                continue;
            }
            if let Some(metric) = bucket(entry, &name, "percent") {
                usage.windows.push(metric);
            }
        }
    }
    // A present modern spend record is authoritative, even when disabled or
    // invalid. Do not resurrect stale legacy extra_usage in that case.
    let extra = payload
        .get("spend")
        .filter(|value| !value.is_null())
        .map_or_else(
            || payload.get("extra_usage").and_then(legacy_spend),
            modern_spend,
        );
    if let Some(metric) = extra {
        usage.windows.push(metric);
    }
    if usage.windows.is_empty() {
        return Err(HttpError::InvalidPayload);
    }
    Ok(usage)
}

fn dollars(
    amount: &Value,
    exponent: &Value,
    currency: Option<&Value>,
    required: bool,
) -> Option<f64> {
    let minor =
        integer(amount).filter(|amount| *amount >= 0 && *amount <= 9_007_199_254_740_991)?;
    let exponent = i32::try_from(integer(exponent)?)
        .ok()
        .filter(|exponent| *exponent >= 0)?;
    match currency {
        Some(value)
            if value
                .as_str()
                .is_some_and(|currency| currency.eq_ignore_ascii_case("USD")) => {}
        None if !required => {}
        _ => return None,
    }
    let divisor = 10_f64.powi(exponent);
    if !divisor.is_finite() {
        return None;
    }
    serde_json::Number::from(minor)
        .as_f64()
        .map(|amount| amount / divisor)
}

fn modern_money(value: &Value) -> Option<f64> {
    dollars(
        value.get("amount_minor")?,
        value.get("exponent")?,
        value.get("currency"),
        true,
    )
}

fn modern_spend(value: &Value) -> Option<UsageMetric> {
    if value.get("enabled").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let used = modern_money(value.get("used")?)?;
    let limit = match value.get("limit")? {
        Value::Null => None,
        limit => Some(modern_money(limit).filter(|amount| *amount > 0.0)?),
    };
    Some(spending_metric(
        "Claude extra usage".to_owned(),
        Some(used),
        limit,
        "USD",
    ))
}

fn legacy_spend(value: &Value) -> Option<UsageMetric> {
    if value.get("is_enabled").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let default_exponent = Value::from(2);
    let exponent = value.get("decimal_places").unwrap_or(&default_exponent);
    let used = dollars(
        value.get("used_credits")?,
        exponent,
        value.get("currency"),
        false,
    )?;
    let limit = match value.get("monthly_limit")? {
        Value::Null => None,
        limit => Some(
            dollars(limit, exponent, value.get("currency"), false)
                .filter(|amount| *amount > 0.0)?,
        ),
    };
    Some(spending_metric(
        "Claude extra usage".to_owned(),
        Some(used),
        limit,
        "USD",
    ))
}

#[cfg(test)]
mod tests {
    use super::parse;
    use serde_json::json;

    #[test]
    fn inactive_scoped_limits_and_minor_units_are_preserved() -> eyre::Result<()> {
        let usage = parse(&json!({"limits":[
            {"kind":"weekly_all","percent":77,"is_active":false},
            {"kind":"weekly_scoped","percent":5,"is_active":false,
             "scope":{"model":{"display_name":"Fable"}},"resets_at":"2026-09-01T00:00:00Z"}
        ],"spend":{"enabled":true,"used":{"amount_minor":1234,"currency":"USD","exponent":2},
            "limit":{"amount_minor":5000,"currency":"USD","exponent":2}}}))
        .map_err(|error| eyre::eyre!("{error}"))?;
        let scoped = usage
            .windows
            .iter()
            .find(|metric| metric.name.contains("Fable"))
            .ok_or_else(|| eyre::eyre!("missing scoped quota"))?;
        assert_eq!(scoped.resets_at, Some(1_788_220_800_000));
        assert!(scoped
            .used_percent
            .is_some_and(|value| (value - 5.0).abs() < f64::EPSILON));
        let extra = usage
            .windows
            .last()
            .ok_or_else(|| eyre::eyre!("missing spend"))?;
        assert!(extra
            .used
            .is_some_and(|value| (value - 12.34).abs() < 0.0001));
        assert!(extra
            .limit
            .is_some_and(|value| (value - 50.0).abs() < f64::EPSILON));
        Ok(())
    }

    #[test]
    fn disabled_modern_spend_does_not_resurrect_legacy_budget() {
        assert!(parse(&json!({"spend":{"enabled":false},
            "extra_usage":{"is_enabled":true,"used_credits":100,"monthly_limit":1000}}))
        .is_err());
        assert!(parse(&json!({"spend":{"enabled":true,
            "used":{"amount_minor":100,"currency":"EUR","exponent":2},"limit":null}}))
        .is_err());
    }
}
