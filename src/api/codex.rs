use super::{
    amount, authorized, nonnegative, percent_allowance, request_json, text, timestamp, HttpError,
};
use crate::auth::AuthRecord;
use crate::dashboard::{
    AccountUsage, AllowanceWindow, BankedReset, CreditBalance, CreditCount, CreditUnit,
    UsageCredits,
};
use crate::time::{now_millis, Millis};
use reqwest::header::HeaderValue;
use reqwest::{Client, RequestBuilder};
use serde_json::Value;

pub(super) async fn fetch(
    client: &Client,
    auth: &AuthRecord,
    base: &str,
) -> Result<AccountUsage, HttpError> {
    let payload = request_json(request(client, auth, &format!("{base}/wham/usage"))?).await?;
    if !payload.is_object() {
        return Err(HttpError::InvalidPayload);
    }
    let details = if payload
        .pointer("/rate_limit_reset_credits/available_count")
        .and_then(count)
        .is_some_and(|count| count > 0)
    {
        // Listing is read-only and optional. Never call the consume endpoint.
        request_json(request(
            client,
            auth,
            &format!("{base}/wham/rate-limit-reset-credits"),
        )?)
        .await
        .ok()
    } else {
        None
    };
    normalize(&payload, details.as_ref(), now_millis())
}

fn request(client: &Client, auth: &AuthRecord, url: &str) -> Result<RequestBuilder, HttpError> {
    let request = authorized(client.get(url), auth, super::USER_AGENT_VALUE)?;
    match auth.account_id.as_deref() {
        Some(id) => {
            let mut value = HeaderValue::from_str(id).map_err(|_| HttpError::InvalidCredential)?;
            value.set_sensitive(true);
            Ok(request.header("ChatGPT-Account-Id", value))
        }
        None => Ok(request),
    }
}

fn normalize(
    payload: &Value,
    details: Option<&Value>,
    now: Option<Millis>,
) -> Result<AccountUsage, HttpError> {
    let mut usage = AccountUsage {
        plan: text(payload, "plan_type").map(str::to_owned),
        ..AccountUsage::default()
    };
    if let Some(rate_limit) = payload.get("rate_limit") {
        add_rate_limit(&mut usage, "Codex", rate_limit, now);
    }
    if let Some(additional) = payload
        .get("additional_rate_limits")
        .and_then(Value::as_array)
    {
        for limit in additional {
            if let Some(rate_limit) = limit.get("rate_limit") {
                add_rate_limit(&mut usage, &additional_title(limit), rate_limit, now);
            }
        }
    }
    add_reset_credits(&mut usage, payload, details, now);
    // Monetary credits and saved window resets are independent balances.
    if let Some(credits) = payload.get("credits").filter(|value| value.is_object()) {
        usage.limits.balances.push(CreditBalance {
            title: "Codex credits".to_owned(),
            credits: UsageCredits {
                count: credits
                    .get("balance")
                    .and_then(amount)
                    .map_or(CreditCount::Unknown, CreditCount::Remaining),
                unit: CreditUnit::GenericCredits,
            },
            expires_at: credits.get("expires_at").and_then(timestamp),
        });
    }
    if usage.limits.allowances.is_empty()
        && usage.limits.balances.is_empty()
        && usage.limits.banked_resets.is_empty()
    {
        return Err(HttpError::InvalidPayload);
    }
    Ok(usage)
}

fn add_rate_limit(usage: &mut AccountUsage, title: &str, rate_limit: &Value, now: Option<Millis>) {
    for field in ["primary_window", "secondary_window"] {
        let Some(value) = rate_limit.get(field).filter(|value| value.is_object()) else {
            continue;
        };
        let window = value
            .get("limit_window_seconds")
            .and_then(count)
            .map(AllowanceWindow::from_seconds);
        let reset = value.get("reset_at").and_then(timestamp).or_else(|| {
            now.zip(value.get("reset_after_seconds").and_then(count))
                .map(|(now, seconds)| now.saturating_add_seconds(seconds))
        });
        usage.limits.allowances.push(percent_allowance(
            title.to_owned(),
            window,
            value.get("used_percent").and_then(nonnegative),
            reset,
        ));
    }
}

fn additional_title(value: &Value) -> String {
    let name = text(value, "limit_name");
    let feature = text(value, "metered_feature");
    let probe = format!("{} {}", name.unwrap_or(""), feature.unwrap_or("")).to_ascii_lowercase();
    if probe.contains("spark") || probe.contains("bengalfox") {
        "Spark".to_owned()
    } else if probe.contains("gpt-reserve") {
        "Reserve".to_owned()
    } else if matches!(feature, Some("chat" | "codex" | "codex-chat")) {
        "Codex".to_owned()
    } else {
        name.or(feature).unwrap_or("Codex").to_owned()
    }
}

fn add_reset_credits(
    usage: &mut AccountUsage,
    payload: &Value,
    details: Option<&Value>,
    now: Option<Millis>,
) {
    let bank = payload.get("rate_limit_reset_credits");
    let available = details
        .and_then(|value| value.get("available_count"))
        .and_then(count)
        .or_else(|| {
            bank.and_then(|value| value.get("available_count"))
                .and_then(count)
        });
    let entries = details
        .and_then(|value| value.get("credits"))
        .and_then(Value::as_array);
    let mut represented = 0_u64;
    if let Some(entries) = entries {
        for entry in entries.iter().filter(|entry| entry.is_object()) {
            if text(entry, "status").is_some_and(|status| status != "available") {
                continue;
            }
            let expires_at = entry.get("expires_at").and_then(timestamp);
            if expires_at
                .zip(now)
                .is_some_and(|(expiry, now)| expiry <= now)
            {
                continue;
            }
            if available.is_some_and(|count| represented >= count) {
                break;
            }
            usage.limits.banked_resets.push(BankedReset {
                title: text(entry, "title").unwrap_or("Codex").to_owned(),
                count: Some(1),
                expires_at,
            });
            represented += 1;
        }
    }
    let remaining = available.map(|count| count.saturating_sub(represented));
    if remaining.is_some_and(|count| count > 0)
        || (represented == 0 && (bank.is_some() || details.is_some()))
    {
        usage.limits.banked_resets.push(BankedReset {
            title: "Codex".to_owned(),
            count: remaining.or_else(|| entries.map(|_| 0)),
            expires_at: None,
        });
    }
}

fn count(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::normalize;
    use crate::dashboard::{AllowanceWindow, CreditAmount, CreditCount, CreditUnit};
    use crate::time::Millis;
    use serde_json::json;

    #[test]
    fn live_bank_count_overrides_stale_usage_and_redeemed_entries() -> eyre::Result<()> {
        let payload = json!({"rate_limit_reset_credits":{"available_count":8}});
        let details = json!({"available_count":1,"credits":[
            {"status":"redeemed","expires_at":"2099-01-01T00:00:00Z"},
            {"status":"available","title":"Saved reset","expires_at":"2099-02-01T00:00:00Z"}
        ]});
        let usage =
            normalize(&payload, Some(&details), None).map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(usage.limits.banked_resets.len(), 1);
        let credit = usage
            .limits
            .banked_resets
            .first()
            .ok_or_else(|| eyre::eyre!("missing reset"))?;
        assert_eq!(credit.count, Some(1));
        assert_eq!(credit.expires_at, Some(Millis::new(4_073_587_200_000)));
        let empty = normalize(
            &payload,
            Some(&json!({"available_count":0,"credits":[]})),
            None,
        )
        .map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(empty.limits.banked_resets.len(), 1);
        assert_eq!(empty.limits.banked_resets[0].count, Some(0));
        assert!(usage.limits.balances.is_empty());
        Ok(())
    }

    #[test]
    fn bank_preserves_duplicate_resets_count_overflow_and_unknown_count() -> eyre::Result<()> {
        let payload = json!({"rate_limit_reset_credits":{"available_count":5}});
        let details = json!({"credits":[
            {"status":"available","expires_at":"2099-01-01T00:00:00Z"},
            {"status":"available","expires_at":"2099-01-01T00:00:00Z"},
            {"status":"available","expires_at":"2020-01-01T00:00:00Z"}
        ]});
        let usage = normalize(
            &payload,
            Some(&details),
            Some(Millis::new(1_788_220_800_000)),
        )
        .map_err(|error| eyre::eyre!("{error}"))?;
        let resets = &usage.limits.banked_resets;
        assert_eq!(
            resets.iter().map(|reset| reset.count).collect::<Vec<_>>(),
            vec![Some(1), Some(1), Some(3)]
        );
        assert_eq!(resets[0].expires_at, resets[1].expires_at);
        assert_eq!(resets[2].expires_at, None);
        let unknown = normalize(&json!({"rate_limit_reset_credits":{}}), None, None)
            .map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(unknown.limits.banked_resets[0].count, None);
        let count_only = normalize(&payload, None, None).map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(count_only.limits.banked_resets.len(), 1);
        assert_eq!(count_only.limits.banked_resets[0].count, Some(5));
        Ok(())
    }

    #[test]
    fn typed_windows_keep_unknown_usage_and_absolute_reset_precedence() -> eyre::Result<()> {
        let usage = normalize(
            &json!({"rate_limit":{"allowed":false,"limit_reached":true,
            "primary_window":{"used_percent":100,"limit_window_seconds":18000,
                "reset_at":"2026-09-01T00:00:00Z","reset_after_seconds":1},
            "secondary_window":{"limit_window_seconds":604_800,"reset_after_seconds":60}
        },"credits":{"balance":9_007_199_254_740_993_u64}}),
            None,
            Some(Millis::new(1000)),
        )
        .map_err(|error| eyre::eyre!("{error}"))?;
        let windows = &usage.limits.allowances;
        assert_eq!(windows[0].window, Some(AllowanceWindow::Seconds(18_000)));
        assert_eq!(windows[0].resets_at, Some(Millis::new(1_788_220_800_000)));
        assert_eq!(windows[1].window, Some(AllowanceWindow::Weekly));
        assert_eq!(windows[1].resets_at, Some(Millis::new(61_000)));
        assert_eq!(windows[1].credits.count, CreditCount::Unknown);
        assert_eq!(windows[1].credits.allocated(), None);
        assert_eq!(
            usage.limits.balances[0].credits.count,
            CreditCount::Remaining(CreditAmount::Integer(9_007_199_254_740_993))
        );
        assert_eq!(
            usage.limits.balances[0].credits.unit,
            CreditUnit::GenericCredits
        );
        Ok(())
    }
}
