use super::{
    authorized, integer, nonnegative, nonnegative_amount, percent_allowance, request_json,
    spending_allowance, text, timestamp, HttpError, CLAUDE_USER_AGENT,
};
use crate::auth::AuthRecord;
use crate::dashboard::{
    AccountUsage, AllowanceWindow, CreditAmount, CreditUnit, Currency, UsageAllowance,
};
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

fn bucket(
    value: &Value,
    title: &str,
    window: AllowanceWindow,
    field: &str,
) -> Option<UsageAllowance> {
    value.is_object().then(|| {
        percent_allowance(
            title.to_owned(),
            Some(window),
            value.get(field).and_then(nonnegative),
            value.get("resets_at").and_then(timestamp),
        )
    })
}

fn parse(payload: &Value) -> Result<AccountUsage, HttpError> {
    let mut usage = AccountUsage::default();
    let entries = payload.get("limits").and_then(Value::as_array);
    for (key, kind, title, window) in [
        (
            "five_hour",
            "session",
            "Claude",
            AllowanceWindow::Seconds(18_000),
        ),
        ("seven_day", "weekly_all", "Claude", AllowanceWindow::Weekly),
        (
            "seven_day_opus",
            "",
            "Claude (Opus)",
            AllowanceWindow::Weekly,
        ),
        (
            "seven_day_sonnet",
            "",
            "Claude (Sonnet)",
            AllowanceWindow::Weekly,
        ),
    ] {
        let legacy = payload
            .get(key)
            .and_then(|value| bucket(value, title, window.clone(), "utilization"));
        let current = || {
            entries?.iter().find_map(|entry| {
                (text(entry, "kind") == Some(kind) && !kind.is_empty())
                    .then(|| bucket(entry, title, window.clone(), "percent"))
                    .flatten()
            })
        };
        if let Some(allowance) = legacy.or_else(current) {
            usage.limits.allowances.push(allowance);
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
            let title = format!("Claude ({display})");
            if usage.limits.allowances.iter().any(|allowance| {
                allowance.title.eq_ignore_ascii_case(&title)
                    && allowance.window == Some(AllowanceWindow::Weekly)
            }) {
                continue;
            }
            if let Some(allowance) = bucket(entry, &title, AllowanceWindow::Weekly, "percent") {
                usage.limits.allowances.push(allowance);
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
    if let Some(allowance) = extra {
        usage.limits.allowances.push(allowance);
    }
    if usage.limits.allowances.is_empty() {
        return Err(HttpError::InvalidPayload);
    }
    Ok(usage)
}

fn major_units(amount: &Value, exponent: &Value) -> Option<CreditAmount> {
    let CreditAmount::Integer(minor) = nonnegative_amount(amount)? else {
        return None;
    };
    let exponent = u32::try_from(integer(exponent)?).ok()?;
    if let Some(divisor) = 10_u64.checked_pow(exponent) {
        if minor % divisor == 0 {
            return Some(CreditAmount::Integer(minor / divisor));
        }
    }
    let divisor = 10_f64.powi(i32::try_from(exponent).ok()?);
    divisor
        .is_finite()
        .then(|| CreditAmount::Decimal(CreditAmount::Integer(minor).as_f64() / divisor))
}

fn currency(value: Option<&Value>) -> CreditUnit {
    match value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) if value.eq_ignore_ascii_case("USD") => CreditUnit::Currency(Currency::Usd),
        Some(value) => CreditUnit::Currency(Currency::Other(value.to_ascii_uppercase())),
        None => CreditUnit::Unknown,
    }
}

fn modern_money(value: &Value) -> Option<(CreditAmount, CreditUnit)> {
    Some((
        major_units(value.get("amount_minor")?, value.get("exponent")?)?,
        currency(value.get("currency")),
    ))
}

fn modern_spend(value: &Value) -> Option<UsageAllowance> {
    if value.get("enabled").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let used = value.get("used").and_then(modern_money);
    let limit = value.get("limit").and_then(modern_money);
    if used
        .as_ref()
        .zip(limit.as_ref())
        .is_some_and(|(used, limit)| used.1 != limit.1)
    {
        return None;
    }
    let unit = used.as_ref().or(limit.as_ref()).map_or_else(
        || {
            currency(
                value
                    .pointer("/used/currency")
                    .or_else(|| value.pointer("/limit/currency")),
            )
        },
        |(_, unit)| unit.clone(),
    );
    Some(spending_allowance(
        "Claude extra usage".to_owned(),
        Some(AllowanceWindow::Monthly),
        used.map(|(amount, _)| amount),
        limit.map(|(amount, _)| amount),
        unit,
    ))
}

fn legacy_spend(value: &Value) -> Option<UsageAllowance> {
    if value.get("is_enabled").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let default_exponent = Value::from(2);
    let exponent = value.get("decimal_places").unwrap_or(&default_exponent);
    // Legacy extra_usage is denominated in USD cents unless it declares otherwise.
    let unit = value
        .get("currency")
        .map_or(CreditUnit::Currency(Currency::Usd), |value| {
            currency(Some(value))
        });
    Some(spending_allowance(
        "Claude extra usage".to_owned(),
        Some(AllowanceWindow::Monthly),
        value
            .get("used_credits")
            .and_then(|amount| major_units(amount, exponent)),
        value
            .get("monthly_limit")
            .and_then(|amount| major_units(amount, exponent)),
        unit,
    ))
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::dashboard::{AllowanceWindow, CreditAmount, CreditCount, CreditUnit, Currency};
    use crate::time::Millis;
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
            .limits
            .allowances
            .iter()
            .find(|allowance| allowance.title.contains("Fable"))
            .ok_or_else(|| eyre::eyre!("missing scoped quota"))?;
        assert_eq!(scoped.resets_at, Some(Millis::new(1_788_220_800_000)));
        assert_eq!(scoped.window, Some(AllowanceWindow::Weekly));
        assert!(scoped
            .credits
            .used_percent()
            .is_some_and(|value| (value - 5.0).abs() < f64::EPSILON));
        let extra = usage
            .limits
            .allowances
            .last()
            .ok_or_else(|| eyre::eyre!("missing spend"))?;
        assert!(extra
            .credits
            .consumed()
            .is_some_and(|value| (value.as_f64() - 12.34).abs() < 0.0001));
        assert_eq!(extra.credits.allocated(), Some(CreditAmount::Integer(50)));
        assert_eq!(extra.credits.unit, CreditUnit::Currency(Currency::Usd));
        Ok(())
    }

    #[test]
    fn disabled_or_incompatible_modern_spend_does_not_resurrect_legacy_budget() {
        assert!(parse(&json!({"spend":{"enabled":false},
            "extra_usage":{"is_enabled":true,"used_credits":100,"monthly_limit":1000}}))
        .is_err());
        assert!(parse(&json!({"spend":{"enabled":true,
            "used":{"amount_minor":100,"currency":"EUR","exponent":2},
            "limit":{"amount_minor":1000,"currency":"USD","exponent":2}},
            "extra_usage":{"is_enabled":true,"used_credits":100,"monthly_limit":1000}}))
        .is_err());
    }

    #[test]
    fn partial_spending_and_missing_percent_do_not_invent_allocation() -> eyre::Result<()> {
        let usage = parse(&json!({"seven_day":{"resets_at":"2026-09-01T00:00:00Z"},
            "spend":{"enabled":true,"used":{"amount_minor":1234,"exponent":2},"limit":null}}))
        .map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(
            usage.limits.allowances[0].credits.count,
            CreditCount::Unknown
        );
        assert_eq!(usage.limits.allowances[0].credits.allocated(), None);
        let spend = &usage.limits.allowances[1].credits;
        assert_eq!(spend.unit, CreditUnit::Unknown);
        assert_eq!(spend.allocated(), None);
        assert_eq!(spend.remaining(), None);
        assert_eq!(spend.used_percent(), None);
        assert!(spend
            .consumed()
            .is_some_and(|amount| (amount.as_f64() - 12.34).abs() < 0.0001));
        Ok(())
    }

    #[test]
    fn integral_money_above_float_precision_keeps_exact_remaining() -> eyre::Result<()> {
        let usage = parse(&json!({"spend":{"enabled":true,
            "used":{"amount_minor":9_007_199_254_740_992_u64,"currency":"USD","exponent":0},
            "limit":{"amount_minor":9_007_199_254_740_993_u64,"currency":"USD","exponent":0}}}))
        .map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(
            usage.limits.allowances[0].credits.remaining(),
            Some(CreditAmount::Integer(1))
        );
        Ok(())
    }
}
