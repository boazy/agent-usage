use super::{authorized, millis, nonnegative, now_ms, percent_metric, request_json, timestamp, HttpError};
use crate::auth::AuthRecord;
use crate::dashboard::{AccountUsage, CreditBalance};
use crate::time::now_millis;
use crate::usage::{collect_items, parse_reset_credits, parse_usage_payload, window_label, ParsedUsage};
use reqwest::header::HeaderValue;
use reqwest::{Client, RequestBuilder};
use serde_json::Value;

pub(super) async fn fetch(client: &Client, auth: &AuthRecord, base: &str)
    -> Result<AccountUsage, HttpError>
{
    let payload = request_json(request(client, auth, &format!("{base}/wham/usage"))?).await?;
    if !payload.is_object() {
        return Err(HttpError::InvalidPayload);
    }
    let parsed = parse_usage_payload(payload);
    let details = if parsed.reset_credits_available.is_some_and(|count| count > 0) {
        // Listing is read-only and optional. Never call the consume endpoint.
        request_json(request(client, auth, &format!("{base}/wham/rate-limit-reset-credits"))?)
            .await.ok()
    } else {
        None
    };
    normalize(&parsed, details.as_ref())
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

fn normalize(parsed: &ParsedUsage, details: Option<&Value>) -> Result<AccountUsage, HttpError> {
    let windows = collect_items(parsed, now_millis()).into_iter().map(|item| {
        percent_metric(
            format!("{} {}", item.meter, window_label(item.limit_window_seconds)),
            item.used_percent,
            item.reset_at.and_then(millis),
        )
    }).collect();
    let mut usage = AccountUsage { windows, credits: Vec::new(), plan: parsed.plan_type.clone() };
    add_reset_credits(&mut usage, parsed, details);
    // Wham's optional monetary credits are independent of saved window resets.
    if let Some(credits) = parsed.raw.get("credits") {
        if let Some(balance) = credits.get("balance").and_then(nonnegative) {
            usage.credits.push(CreditBalance {
                name: "Codex credits".to_owned(), balance, unit: "credits".to_owned(),
                expires_at: credits.get("expires_at").and_then(timestamp),
            });
        }
    }
    if usage.windows.is_empty() && usage.credits.is_empty() {
        return Err(HttpError::InvalidPayload);
    }
    Ok(usage)
}

fn add_reset_credits(usage: &mut AccountUsage, parsed: &ParsedUsage, details: Option<&Value>) {
    let now = now_ms();
    let detail_credits = details.and_then(parse_reset_credits).map(|mut credits| {
        credits.retain(|credit| {
            !credit.expires_at.and_then(millis).zip(now)
                .is_some_and(|(expiry, now)| expiry <= now)
        });
        credits
    });
    let mut available = details.and_then(|value| value.get("available_count"))
        .and_then(Value::as_u64).or_else(|| {
            detail_credits.as_ref().map_or(parsed.reset_credits_available, |credits| {
                u64::try_from(credits.len()).ok()
            })
        });
    if let Some(credits) = detail_credits {
        let mut remaining = available.unwrap_or(0);
        for credit in credits {
            if remaining == 0 { break; }
            usage.credits.push(CreditBalance {
                name: credit.title.unwrap_or_else(|| "Banked reset".to_owned()),
                balance: 1.0, unit: "resets".to_owned(),
                expires_at: credit.expires_at.and_then(millis),
            });
            remaining -= 1;
        }
        // Preserve a provider-reported count beyond the returned detail page.
        available = available.map(|_| remaining);
    }
    if let Some(balance) = available.and_then(|count| serde_json::Number::from(count).as_f64()) {
        if balance > 0.0 || usage.credits.is_empty() {
            usage.credits.push(CreditBalance {
                name: "Banked resets".to_owned(), balance, unit: "resets".to_owned(), expires_at: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize, parse_usage_payload};
    use serde_json::json;

    #[test]
    fn live_bank_count_overrides_stale_usage_and_redeemed_entries() -> eyre::Result<()> {
        let parsed = parse_usage_payload(json!({"rate_limit_reset_credits":{"available_count":8}}));
        let details = json!({"available_count":1,"credits":[
            {"status":"redeemed","expires_at":"2099-01-01T00:00:00Z"},
            {"status":"available","title":"Saved reset","expires_at":"2099-02-01T00:00:00Z"}
        ]});
        let usage = normalize(&parsed, Some(&details)).map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(usage.credits.len(), 1);
        let credit = usage.credits.first().ok_or_else(|| eyre::eyre!("missing reset"))?;
        assert!((credit.balance - 1.0).abs() < f64::EPSILON);
        assert_eq!(credit.expires_at, Some(4_073_587_200_000));
        let empty = normalize(&parsed, Some(&json!({"available_count":0,"credits":[]})))
            .map_err(|error| eyre::eyre!("{error}"))?;
        assert!(empty.credits.iter().all(|credit| credit.balance.abs() < f64::EPSILON));
        Ok(())
    }
}
