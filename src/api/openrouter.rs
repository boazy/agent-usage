use super::{authorized, nonnegative, number, request_json, spending_metric, text, timestamp, HttpError};
use crate::auth::AuthRecord;
use crate::dashboard::{AccountUsage, CreditBalance};
use reqwest::Client;
use serde_json::Value;

pub(super) async fn fetch(client: &Client, auth: &AuthRecord, base: &str)
    -> Result<AccountUsage, HttpError>
{
    let payload = request_json(authorized(client.get(format!("{base}/key")), auth, super::USER_AGENT_VALUE)?).await?;
    let mut usage = parse(&payload)?;
    // /credits is account-wide and requires a management key. Never issue it
    // for an ordinary API key, and never relabel it as key-specific spending.
    let data = payload.get("data").ok_or(HttpError::InvalidPayload)?;
    let management = data.get("is_management_key").or_else(|| data.get("is_provisioning_key"))
        .and_then(Value::as_bool) == Some(true);
    if management {
        let credits = request_json(authorized(client.get(format!("{base}/credits")), auth, super::USER_AGENT_VALUE)?).await?;
        let data = credits.get("data").ok_or(HttpError::InvalidPayload)?;
        let total = data.get("total_credits").and_then(nonnegative).ok_or(HttpError::InvalidPayload)?;
        let used = data.get("total_usage").and_then(nonnegative).ok_or(HttpError::InvalidPayload)?;
        usage.credits.push(CreditBalance {
            name: "Account-wide credits".to_owned(), balance: total - used,
            unit: "USD".to_owned(), expires_at: None,
        });
    }
    Ok(usage)
}

fn parse(payload: &Value) -> Result<AccountUsage, HttpError> {
    let data = payload.get("data").filter(|data| data.is_object()).ok_or(HttpError::InvalidPayload)?;
    let mut usage = AccountUsage::default();
    for (field, name) in [
        ("usage", "Key spending (all time)"),
        ("usage_daily", "Key spending (UTC day)"),
        ("usage_weekly", "Key spending (UTC week)"),
        ("usage_monthly", "Key spending (UTC month)"),
        ("byok_usage", "External BYOK spending (all time)"),
        ("byok_usage_daily", "External BYOK spending (UTC day)"),
        ("byok_usage_weekly", "External BYOK spending (UTC week)"),
        ("byok_usage_monthly", "External BYOK spending (UTC month)"),
    ] {
        if let Some(used) = data.get(field).and_then(nonnegative) {
            usage.windows.push(spending_metric(name.to_owned(), Some(used), None, "USD"));
        }
    }
    if let Some(limit) = data.get("limit").and_then(nonnegative) {
        let reset = text(data, "limit_reset");
        let name = match reset {
            Some("daily") => "Key budget (daily)",
            Some("weekly") => "Key budget (weekly)",
            Some("monthly") => "Key budget (monthly)",
            None => "Key budget (all time)",
            Some(_) => "Key budget (provider-defined reset)",
        };
        let remaining = data.get("limit_remaining").and_then(number);
        // Remaining is authoritative and already accounts for reset period and
        // include_byok_in_limit. Total lifetime spending is not a periodic cap.
        let used = remaining.map(|remaining| (limit - remaining).max(0.0));
        usage.windows.push(spending_metric(name.to_owned(), used, Some(limit), "USD"));
        if let Some(balance) = remaining {
            usage.credits.push(CreditBalance {
                name: "Key budget remaining".to_owned(), balance, unit: "USD".to_owned(),
                expires_at: data.get("expires_at").and_then(timestamp),
            });
        }
    }
    if usage.windows.is_empty() && usage.credits.is_empty() { return Err(HttpError::InvalidPayload); }
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::parse;
    use serde_json::json;

    #[test]
    fn budget_uses_remaining_not_lifetime_spend_or_external_byok() -> eyre::Result<()> {
        let usage = parse(&json!({"data":{"usage":800,"usage_monthly":10,
            "byok_usage":400,"include_byok_in_limit":false,
            "limit":100,"limit_remaining":75,"limit_reset":"monthly"}}))
            .map_err(|error| eyre::eyre!("{error}"))?;
        let budget = usage.windows.iter().find(|metric| metric.limit.is_some())
            .ok_or_else(|| eyre::eyre!("missing budget"))?;
        assert!(budget.used.is_some_and(|used| (used - 25.0).abs() < f64::EPSILON));
        assert!(budget.used_percent.is_some_and(|used| (used - 25.0).abs() < f64::EPSILON));
        let unlimited = parse(&json!({"data":{"usage":42,"limit":null,"limit_remaining":null}}))
            .map_err(|error| eyre::eyre!("{error}"))?;
        assert!(unlimited.windows.iter().all(|metric| metric.limit.is_none() && metric.used_percent.is_none()));
        assert!(unlimited.credits.is_empty());
        Ok(())
    }
}
