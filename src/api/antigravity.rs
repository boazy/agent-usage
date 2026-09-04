use super::{authorized, nonnegative, percent_metric, request_json, text, timestamp, HttpError, ANTIGRAVITY_USER_AGENT};
use crate::auth::AuthRecord;
use crate::dashboard::{AccountUsage, CreditBalance};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) async fn fetch(client: &Client, auth: &AuthRecord, base: &str)
    -> Result<AccountUsage, HttpError>
{
    let summary = request_json(request(client, auth, &format!("{base}/v1internal:retrieveUserQuotaSummary"))?).await;
    match summary {
        Ok(payload) => {
            if let Some(usage) = parse_summary(&payload) { return Ok(usage); }
        }
        Err(HttpError::Status(StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED)) => {}
        Err(error) => return Err(error),
    }
    // Read-only compatibility path; never onboard a user, provision a project,
    // follow validation links, or treat a rejected token as missing RPC support.
    let legacy = request_json(request(client, auth, &format!("{base}/v1internal:fetchAvailableModels"))?).await?;
    parse_models(&legacy)
}

fn request(client: &Client, auth: &AuthRecord, url: &str) -> Result<RequestBuilder, HttpError> {
    Ok(authorized(client.post(url), auth, ANTIGRAVITY_USER_AGENT)?
        .json(&json!({"project":auth.project_id})))
}

fn parse_summary(payload: &Value) -> Option<AccountUsage> {
    let mut usage = AccountUsage::default();
    let groups = payload.get("groups").and_then(Value::as_array);
    let grouped = groups.is_some_and(|groups| groups.iter().any(|group| {
        group.get("buckets").and_then(Value::as_array).is_some_and(|buckets| !buckets.is_empty())
    }));
    if grouped {
        for group in groups? {
            if let Some(buckets) = group.get("buckets").and_then(Value::as_array) {
                for bucket in buckets { add_bucket(&mut usage, bucket, text(group, "displayName")); }
            }
        }
    } else if let Some(buckets) = payload.get("buckets").and_then(Value::as_array) {
        for bucket in buckets { add_bucket(&mut usage, bucket, None); }
    }
    (!usage.windows.is_empty() || !usage.credits.is_empty()).then_some(usage)
}

fn add_bucket(usage: &mut AccountUsage, bucket: &Value, group: Option<&str>) {
    if bucket.get("disabled").and_then(Value::as_bool) == Some(true) { return; }
    let label = text(bucket, "displayName").or_else(|| text(bucket, "bucketId")).unwrap_or("Quota");
    let name = group.map_or_else(|| label.to_owned(), |group| format!("{group}: {label}"));
    let window = text(bucket, "window").map(window_name);
    let name = window.map_or_else(|| name.clone(), |window| format!("{name} ({window})"));
    let percent = bucket.get("remainingFraction").and_then(nonnegative)
        .map(|fraction| (1.0 - fraction.clamp(0.0, 1.0)) * 100.0);
    let resets_at = bucket.get("resetTime").and_then(timestamp);
    if percent.is_some() || resets_at.is_some() {
        usage.windows.push(percent_metric(name.clone(), percent, resets_at));
    }
    if percent.is_none() {
        if let Some(balance) = bucket.get("remainingAmount").and_then(nonnegative) {
            usage.credits.push(CreditBalance {
                name: format!("{name} remaining"), balance,
                unit: "quota units (unspecified)".to_owned(), expires_at: None,
            });
        }
    }
}

fn window_name(window: &str) -> &str {
    match window {
        "WINDOW_WEEKLY" | "weekly" | "7d" => "Weekly",
        "WINDOW_DAILY" | "daily" | "24h" => "Daily",
        "WINDOW_FIVE_HOURS" | "5h" | "five_hour" => "5 hour",
        other => other,
    }
}

struct Quota<'a> {
    value: &'a Value,
    window: Option<&'a str>,
    tier: Option<&'a str>,
}

fn add_values<'a>(quotas: &mut Vec<Quota<'a>>, value: &'a Value, window: Option<&'a str>, tier: Option<&'a str>) {
    match value {
        Value::Array(entries) => {
            for entry in entries.iter().filter(|entry| entry.is_object()) {
                quotas.push(Quota { value: entry, window, tier });
            }
        }
        Value::Object(_) => quotas.push(Quota { value, window, tier }),
        _ => {}
    }
}

fn model_quotas(model: &Value) -> Vec<Quota<'_>> {
    let mut quotas = Vec::new();
    for (key, window) in [
        ("quotaInfo", None), ("quotaInfos", None),
        ("dailyQuotaInfo", Some("Daily")), ("dailyQuotaInfos", Some("Daily")),
        ("weeklyQuotaInfo", Some("Weekly")), ("weeklyQuotaInfos", Some("Weekly")),
    ] {
        if let Some(value) = model.get(key) { add_values(&mut quotas, value, window, None); }
    }
    if let Some(tiers) = model.get("quotaInfoByTier").and_then(Value::as_object) {
        for (tier, value) in tiers { add_values(&mut quotas, value, None, Some(tier)); }
    }
    for key in ["quotaInfoByWindow", "quotaInfosByWindow"] {
        if let Some(windows) = model.get(key).and_then(Value::as_object) {
            for (window, value) in windows { add_values(&mut quotas, value, Some(window), None); }
        }
    }
    quotas
}

fn counter<'a>(quota: &'a Value, model: &'a Value, model_id: &'a str) -> &'a str {
    let provider = text(quota, "modelProvider").or_else(|| text(quota, "apiProvider"))
        .or_else(|| text(model, "modelProvider")).or_else(|| text(model, "apiProvider"));
    match provider {
        Some("MODEL_PROVIDER_GOOGLE" | "API_PROVIDER_GOOGLE_GEMINI") => "Google backend",
        Some("MODEL_PROVIDER_ANTHROPIC" | "API_PROVIDER_ANTHROPIC_VERTEX") => "Anthropic backend",
        Some("MODEL_PROVIDER_OPENAI" | "API_PROVIDER_OPENAI_VERTEX") => "OpenAI backend",
        Some(other) => other,
        None => text(model, "displayName").unwrap_or(model_id),
    }
}

fn parse_models(payload: &Value) -> Result<AccountUsage, HttpError> {
    let models = payload.get("models").and_then(Value::as_object).ok_or(HttpError::InvalidPayload)?;
    let mut metrics = BTreeMap::new();
    for (model_id, model) in models {
        for quota in model_quotas(model) {
            let percent = quota.value.get("remainingFraction").and_then(nonnegative)
                .map(|fraction| (1.0 - fraction.clamp(0.0, 1.0)) * 100.0);
            let reset = quota.value.get("resetTime").and_then(timestamp);
            // A reset timestamp alone does not quantitatively report exhaustion.
            if percent.is_none() && reset.is_none() { continue; }
            let counter = counter(quota.value, model, model_id);
            let window = text(quota.value, "windowLabel").or_else(|| text(quota.value, "windowId"))
                .or(quota.window).map_or("Quota", window_name);
            let tier = quota.tier.or_else(|| text(quota.value, "tier")).unwrap_or("");
            let name = if tier.is_empty() { format!("{counter}: {window}") }
                else { format!("{counter}: {window} ({tier})") };
            // Equal backend/tier/window/reset rows represent the same counter.
            // Do not merge unknown backends or distinct reset windows by guesswork.
            let key = (counter.to_owned(), tier.to_owned(), window.to_owned(), reset);
            let metric = metrics.entry(key).or_insert_with(|| percent_metric(name, percent, reset));
            if percent.is_some_and(|value| metric.used_percent.is_none_or(|prior| value > prior)) {
                *metric = percent_metric(metric.name.clone(), percent, reset);
            }
        }
    }
    if metrics.is_empty() { return Err(HttpError::InvalidPayload); }
    Ok(AccountUsage { windows: metrics.into_values().collect(), ..AccountUsage::default() })
}

#[cfg(test)]
mod tests {
    use super::{parse_models, parse_summary};
    use serde_json::json;

    #[test]
    fn summary_keeps_shared_third_party_bucket_single_and_unknown_units_unknown() -> eyre::Result<()> {
        let usage = parse_summary(&json!({"groups":[{"displayName":"Third party","buckets":[
            {"bucketId":"3p-weekly","remainingFraction":0.25,"window":"WINDOW_WEEKLY"},
            {"bucketId":"3p-amount","remainingAmount":"42"},
            {"bucketId":"disabled","remainingFraction":0,"disabled":true}
        ]}]})).ok_or_else(|| eyre::eyre!("missing summary"))?;
        assert_eq!(usage.windows.len(), 1);
        assert!(usage.windows.first().and_then(|metric| metric.used_percent)
            .is_some_and(|value| (value - 75.0).abs() < f64::EPSILON));
        assert_eq!(usage.credits.len(), 1);
        assert!(usage.credits.first().is_some_and(|credit| (credit.balance - 42.0).abs() < f64::EPSILON));
        Ok(())
    }

    #[test]
    fn legacy_reset_only_is_unknown_and_backend_windows_are_not_combined() -> eyre::Result<()> {
        let usage = parse_models(&json!({"models":{
            "gemini":{"modelProvider":"MODEL_PROVIDER_GOOGLE",
                "weeklyQuotaInfo":{"resetTime":"2026-09-01T00:00:00Z"}},
            "claude":{"modelProvider":"MODEL_PROVIDER_ANTHROPIC",
                "quotaInfoByWindow":{"weekly":{"remainingFraction":0.8,"resetTime":"2026-09-01T00:00:00Z"}}}
        }})).map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(usage.windows.len(), 2);
        let google = usage.windows.iter().find(|metric| metric.name.starts_with("Google"))
            .ok_or_else(|| eyre::eyre!("missing Google counter"))?;
        assert!(google.used_percent.is_none());
        assert!(google.limit.is_none());
        assert_eq!(google.resets_at, Some(1_788_220_800_000));
        Ok(())
    }
}
