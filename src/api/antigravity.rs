use super::{
    amount, authorized, nonnegative_amount, number, request_json, text, timestamp, HttpError,
    ANTIGRAVITY_USER_AGENT,
};
use crate::auth::AuthRecord;
use crate::dashboard::{
    AccountUsage, AllowanceWindow, CreditAmount, CreditCount, CreditUnit, Currency, UsageAllowance,
    UsageCredits,
};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) async fn fetch(
    client: &Client,
    auth: &AuthRecord,
    base: &str,
) -> Result<AccountUsage, HttpError> {
    let summary = request_json(request(
        client,
        auth,
        &format!("{base}/v1internal:retrieveUserQuotaSummary"),
    )?)
    .await;
    match summary {
        Ok(payload) => {
            if let Some(usage) = parse_summary(&payload) {
                return Ok(usage);
            }
        }
        Err(HttpError::Status(
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED,
        )) => {}
        Err(error) => return Err(error),
    }
    // Read-only compatibility path; never onboard a user, provision a project,
    // follow validation links, or treat a rejected token as missing RPC support.
    let legacy = request_json(request(
        client,
        auth,
        &format!("{base}/v1internal:fetchAvailableModels"),
    )?)
    .await?;
    parse_models(&legacy)
}

fn request(client: &Client, auth: &AuthRecord, url: &str) -> Result<RequestBuilder, HttpError> {
    Ok(authorized(client.post(url), auth, ANTIGRAVITY_USER_AGENT)?
        .json(&json!({"project":auth.project_id})))
}

#[derive(Clone, Copy)]
enum Family {
    Gemini,
    ThirdParty,
}

impl Family {
    const fn title(self) -> &'static str {
        match self {
            Self::Gemini => "Gemini",
            Self::ThirdParty => "Claude/GPT",
        }
    }

    const fn window(self) -> AllowanceWindow {
        match self {
            Self::Gemini => AllowanceWindow::Seconds(18_000),
            Self::ThirdParty => AllowanceWindow::Weekly,
        }
    }
}

fn family(value: &str) -> Option<Family> {
    let value = value.to_ascii_lowercase();
    if value.contains("gemini") || value.contains("google") {
        Some(Family::Gemini)
    } else if [
        "claude",
        "gpt",
        "anthropic",
        "openai",
        "third party",
        "third_party",
        "third-party",
    ]
    .iter()
    .any(|name| value.contains(*name))
        || value.starts_with("3p")
    {
        Some(Family::ThirdParty)
    } else {
        None
    }
}

fn parse_summary(payload: &Value) -> Option<AccountUsage> {
    let mut usage = AccountUsage::default();
    let groups = payload.get("groups").and_then(Value::as_array);
    let grouped = groups.is_some_and(|groups| {
        groups.iter().any(|group| {
            group
                .get("buckets")
                .and_then(Value::as_array)
                .is_some_and(|buckets| !buckets.is_empty())
        })
    });
    if grouped {
        for group in groups? {
            if let Some(buckets) = group.get("buckets").and_then(Value::as_array) {
                for bucket in buckets {
                    add_bucket(&mut usage, bucket, text(group, "displayName"));
                }
            }
        }
    } else if let Some(buckets) = payload.get("buckets").and_then(Value::as_array) {
        for bucket in buckets {
            add_bucket(&mut usage, bucket, None);
        }
    }
    (!usage.limits.allowances.is_empty()).then_some(usage)
}

fn add_bucket(usage: &mut AccountUsage, bucket: &Value, group: Option<&str>) {
    if bucket.get("disabled").and_then(Value::as_bool) == Some(true) {
        return;
    }
    let label = text(bucket, "displayName")
        .or_else(|| text(bucket, "bucketId"))
        .unwrap_or("Quota");
    let family = family(label).or_else(|| group.and_then(family));
    let title = family.map_or_else(
        || group.map_or_else(|| label.to_owned(), |group| format!("{group}: {label}")),
        |family| family.title().to_owned(),
    );
    if let Some(allowance) = quota_allowance(
        bucket,
        title,
        quota_window(bucket).or_else(|| family.map(Family::window)),
    ) {
        usage.limits.allowances.push(allowance);
    }
}

fn window(value: &str) -> AllowanceWindow {
    match value.to_ascii_lowercase().as_str() {
        "window_weekly" | "weekly" | "7d" => AllowanceWindow::Weekly,
        "window_daily" | "daily" | "24h" => AllowanceWindow::Daily,
        "window_monthly" | "monthly" => AllowanceWindow::Monthly,
        "window_five_hours" | "5h" | "five_hour" | "5 hours" | "5 hour" => {
            AllowanceWindow::Seconds(18_000)
        }
        "all_time" | "all time" => AllowanceWindow::AllTime,
        _ => AllowanceWindow::Named(value.to_owned()),
    }
}

fn quota_window(value: &Value) -> Option<AllowanceWindow> {
    text(value, "window")
        .or_else(|| text(value, "windowLabel"))
        .or_else(|| text(value, "windowId"))
        .map(window)
        .or_else(|| {
            value
                .get("windowSeconds")
                .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
                .map(AllowanceWindow::from_seconds)
        })
}

fn unit(value: &Value) -> CreditUnit {
    match text(value, "unit")
        .or_else(|| text(value, "quotaUnit"))
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("usd") => CreditUnit::Currency(Currency::Usd),
        Some("credits" | "credit") => CreditUnit::GenericCredits,
        Some("percentage" | "percent" | "%") => CreditUnit::Percentage,
        _ => CreditUnit::Unknown,
    }
}

fn quota_allowance(
    value: &Value,
    title: String,
    window: Option<AllowanceWindow>,
) -> Option<UsageAllowance> {
    let allocated = value
        .get("allocatedAmount")
        .or_else(|| value.get("limitAmount"))
        .and_then(nonnegative_amount);
    let consumed = value
        .get("usedAmount")
        .or_else(|| value.get("consumedAmount"))
        .and_then(nonnegative_amount);
    let remaining = value.get("remainingAmount").and_then(amount);
    let resets_at = value.get("resetTime").and_then(timestamp);
    let fraction = value.get("remainingFraction").and_then(number);
    let credits = if allocated.is_some() || consumed.is_some() || remaining.is_some() {
        let count = match (allocated, consumed, remaining) {
            (Some(allocated), Some(consumed), _) => CreditCount::Full {
                allocated,
                consumed,
            },
            (Some(allocated), None, Some(remaining)) => CreditCount::Full {
                allocated,
                consumed: allocated.subtract(remaining),
            },
            (None, Some(consumed), _) => CreditCount::Consumed(consumed),
            (None, None, Some(remaining)) => CreditCount::Remaining(remaining),
            (Some(allocated), None, None) => CreditCount::Allocated(allocated),
            (None, None, None) => CreditCount::Unknown,
        };
        UsageCredits {
            count,
            unit: unit(value),
        }
    } else if let Some(percent) = fraction
        .map(|fraction| (1.0 - fraction) * 100.0)
        .filter(|percent| percent.is_finite())
    {
        UsageCredits {
            count: CreditCount::Full {
                allocated: CreditAmount::Integer(100),
                consumed: CreditAmount::Decimal(percent),
            },
            unit: CreditUnit::Percentage,
        }
    } else {
        if resets_at.is_none()
            && ![
                "remainingAmount",
                "remainingFraction",
                "allocatedAmount",
                "limitAmount",
                "usedAmount",
                "consumedAmount",
            ]
            .iter()
            .any(|field| value.get(*field).is_some())
        {
            return None;
        }
        UsageCredits {
            count: CreditCount::Unknown,
            unit: unit(value),
        }
    };
    Some(UsageAllowance {
        title,
        window,
        resets_at,
        credits,
    })
}

struct Quota<'a> {
    value: &'a Value,
    window: Option<&'a str>,
    tier: Option<&'a str>,
}

fn add_values<'a>(
    quotas: &mut Vec<Quota<'a>>,
    value: &'a Value,
    window: Option<&'a str>,
    tier: Option<&'a str>,
) {
    match value {
        Value::Array(entries) => {
            for entry in entries.iter().filter(|entry| entry.is_object()) {
                quotas.push(Quota {
                    value: entry,
                    window,
                    tier,
                });
            }
        }
        Value::Object(_) => quotas.push(Quota {
            value,
            window,
            tier,
        }),
        _ => {}
    }
}

fn model_quotas(model: &Value) -> Vec<Quota<'_>> {
    let mut quotas = Vec::new();
    for (key, window) in [
        ("quotaInfo", None),
        ("quotaInfos", None),
        ("dailyQuotaInfo", Some("Daily")),
        ("dailyQuotaInfos", Some("Daily")),
        ("weeklyQuotaInfo", Some("Weekly")),
        ("weeklyQuotaInfos", Some("Weekly")),
    ] {
        if let Some(value) = model.get(key) {
            add_values(&mut quotas, value, window, None);
        }
    }
    if let Some(tiers) = model.get("quotaInfoByTier").and_then(Value::as_object) {
        for (tier, value) in tiers {
            add_values(&mut quotas, value, None, Some(tier));
        }
    }
    for key in ["quotaInfoByWindow", "quotaInfosByWindow"] {
        if let Some(windows) = model.get(key).and_then(Value::as_object) {
            for (window, value) in windows {
                add_values(&mut quotas, value, Some(window), None);
            }
        }
    }
    quotas
}

fn parse_models(payload: &Value) -> Result<AccountUsage, HttpError> {
    let models = payload
        .get("models")
        .and_then(Value::as_object)
        .ok_or(HttpError::InvalidPayload)?;
    let mut usage = AccountUsage::default();
    let mut shared: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for (model_id, model) in models {
        for quota in model_quotas(model) {
            let provider = text(quota.value, "modelProvider")
                .or_else(|| text(quota.value, "apiProvider"))
                .or_else(|| text(model, "modelProvider"))
                .or_else(|| text(model, "apiProvider"));
            let label = text(model, "displayName").unwrap_or(model_id);
            let family = provider
                .and_then(family)
                .or_else(|| family(label))
                .or_else(|| family(model_id));
            let title = family.map_or(label, |family| family.title());
            let tier = quota
                .tier
                .or_else(|| text(quota.value, "tier"))
                .unwrap_or("");
            let title = if tier.is_empty() {
                title.to_owned()
            } else {
                format!("{title} ({tier})")
            };
            let window = quota_window(quota.value)
                .or_else(|| quota.window.map(window))
                .or_else(|| quota_window(model))
                .or_else(|| family.map(Family::window));
            let Some(allowance) = quota_allowance(quota.value, title, window) else {
                continue;
            };
            let counter = ["sharedQuotaId", "quotaGroupId", "bucketId", "quotaId"]
                .into_iter()
                .find_map(|field| text(quota.value, field));
            if let Some(counter) = counter {
                // Provider labels and matching percentages alone do not establish
                // a shared counter. Keep distinct windows and conflicting reports.
                let entries = shared.entry((counter, tier)).or_default();
                if entries
                    .iter()
                    .filter_map(|index| usage.limits.allowances.get(*index))
                    .any(|prior| {
                        prior.title == allowance.title
                            && prior.window == allowance.window
                            && prior.resets_at == allowance.resets_at
                            && prior.credits == allowance.credits
                    })
                {
                    continue;
                }
                entries.push(usage.limits.allowances.len());
            }
            usage.limits.allowances.push(allowance);
        }
    }
    if usage.limits.allowances.is_empty() {
        return Err(HttpError::InvalidPayload);
    }
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::{parse_models, parse_summary};
    use crate::dashboard::{AllowanceWindow, CreditAmount, CreditCount, CreditUnit};
    use crate::time::Millis;
    use serde_json::json;

    #[test]
    fn summary_keeps_shared_third_party_bucket_single_and_unknown_units_unknown() -> eyre::Result<()>
    {
        let usage = parse_summary(&json!({"groups":[{"displayName":"Third party","buckets":[
            {"bucketId":"3p-weekly","remainingFraction":0.25,"window":"WINDOW_WEEKLY"},
            {"bucketId":"3p-amount","remainingAmount":"9007199254740993"},
            {"bucketId":"disabled","remainingFraction":0,"disabled":true}
        ]}]}))
        .ok_or_else(|| eyre::eyre!("missing summary"))?;
        assert_eq!(usage.limits.allowances.len(), 2);
        assert!(usage.limits.allowances[0]
            .credits
            .used_percent()
            .is_some_and(|value| (value - 75.0).abs() < f64::EPSILON));
        let remaining = &usage.limits.allowances[1];
        assert_eq!(remaining.window, Some(AllowanceWindow::Weekly));
        assert_eq!(
            remaining.credits.count,
            CreditCount::Remaining(CreditAmount::Integer(9_007_199_254_740_993))
        );
        assert_eq!(remaining.credits.unit, CreditUnit::Unknown);
        assert_eq!(remaining.credits.allocated(), None);
        assert_eq!(remaining.credits.used_percent(), None);
        assert!(usage.limits.balances.is_empty());
        Ok(())
    }

    #[test]
    fn explicit_windows_override_family_defaults_and_reset_only_stays_unknown() -> eyre::Result<()>
    {
        let usage = parse_models(&json!({"models":{
            "gemini":{"modelProvider":"MODEL_PROVIDER_GOOGLE",
                "weeklyQuotaInfo":{"resetTime":"2026-09-01T00:00:00Z"}},
            "claude":{"modelProvider":"MODEL_PROVIDER_ANTHROPIC",
                "quotaInfo":{"remainingFraction":0.8,"windowLabel":"5h"}},
            "gemini-default":{"quotaInfo":{"remainingFraction":0.7}},
            "gpt-default":{"quotaInfo":{"remainingFraction":0.5}}
        }}))
        .map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(usage.limits.allowances.len(), 4);
        let google = usage
            .limits
            .allowances
            .iter()
            .find(|allowance| allowance.resets_at.is_some())
            .ok_or_else(|| eyre::eyre!("missing reset-only counter"))?;
        assert_eq!(google.window, Some(AllowanceWindow::Weekly));
        assert_eq!(google.credits.count, CreditCount::Unknown);
        assert_eq!(google.credits.unit, CreditUnit::Unknown);
        assert_eq!(google.credits.allocated(), None);
        assert_eq!(google.resets_at, Some(Millis::new(1_788_220_800_000)));
        assert!(usage
            .limits
            .allowances
            .iter()
            .any(|allowance| allowance.title == "Claude/GPT"
                && allowance.window == Some(AllowanceWindow::Seconds(18_000))));
        assert!(usage
            .limits
            .allowances
            .iter()
            .any(|allowance| allowance.title == "Gemini"
                && allowance.window == Some(AllowanceWindow::Seconds(18_000))));
        assert!(usage
            .limits
            .allowances
            .iter()
            .any(|allowance| allowance.title == "Claude/GPT"
                && allowance.window == Some(AllowanceWindow::Weekly)));
        Ok(())
    }

    #[test]
    fn only_explicit_identical_shared_counters_are_collapsed() -> eyre::Result<()> {
        let usage = parse_models(&json!({"models":{
            "claude-a":{"quotaInfo":{"remainingFraction":0.5,"bucketId":"shared"}},
            "gpt-b":{"quotaInfo":{"remainingFraction":0.5,"bucketId":"shared"}},
            "claude-independent":{"quotaInfo":{"remainingFraction":0.5}},
            "claude-other-reset":{"quotaInfo":{"remainingFraction":0.5,"bucketId":"shared","resetTime":"2026-09-01T00:00:00Z"}},
            "gpt-other-window":{"dailyQuotaInfo":{"remainingFraction":0.5,"bucketId":"shared"}}
        }})).map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(usage.limits.allowances.len(), 4);
        assert_eq!(
            usage
                .limits
                .allowances
                .iter()
                .filter(
                    |allowance| allowance.window == Some(AllowanceWindow::Weekly)
                        && allowance.resets_at.is_none()
                )
                .count(),
            2
        );
        assert_eq!(
            usage
                .limits
                .allowances
                .iter()
                .filter(|allowance| allowance.window == Some(AllowanceWindow::Daily))
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn amount_quotas_keep_exact_allocation_and_signed_remaining() -> eyre::Result<()> {
        let usage = parse_summary(&json!({"buckets":[
            {"bucketId":"gemini","allocatedAmount":9_007_199_254_740_993_u64,"remainingAmount":9_007_199_254_740_992_u64,"unit":"credits"},
            {"bucketId":"3p","remainingAmount":-2.5},
            {"bucketId":"unknown","remainingAmount":null,"resetTime":"2026-09-01T00:00:00Z"}
        ]})).ok_or_else(|| eyre::eyre!("missing summary"))?;
        assert_eq!(
            usage.limits.allowances[0].credits.consumed(),
            Some(CreditAmount::Integer(1))
        );
        assert_eq!(
            usage.limits.allowances[0].credits.unit,
            CreditUnit::GenericCredits
        );
        assert_eq!(
            usage.limits.allowances[1].credits.count,
            CreditCount::Remaining(CreditAmount::Decimal(-2.5))
        );
        assert_eq!(usage.limits.allowances[1].credits.used_percent(), None);
        assert_eq!(
            usage.limits.allowances[2].credits.count,
            CreditCount::Unknown
        );
        assert_eq!(usage.limits.allowances[2].credits.unit, CreditUnit::Unknown);
        Ok(())
    }
}
