use super::{
    amount, authorized, nonnegative_amount, request_json, spending_allowance, text, HttpError,
};
use crate::auth::AuthRecord;
use crate::dashboard::{
    AccountUsage, AllowanceWindow, CreditBalance, CreditCount, CreditUnit, Currency,
    UsageAllowance, UsageCredits,
};
use reqwest::Client;
use serde_json::Value;

pub(super) async fn fetch(
    client: &Client,
    auth: &AuthRecord,
    base: &str,
) -> Result<AccountUsage, HttpError> {
    let payload = request_json(authorized(
        client.get(format!("{base}/key")),
        auth,
        super::USER_AGENT_VALUE,
    )?)
    .await?;
    let mut usage = parse(&payload)?;
    // /credits is account-wide and requires a management key. Never issue it
    // for an ordinary API key, and never relabel it as key-specific spending.
    let data = payload.get("data").ok_or(HttpError::InvalidPayload)?;
    let management = data
        .get("is_management_key")
        .or_else(|| data.get("is_provisioning_key"))
        .and_then(Value::as_bool)
        == Some(true);
    if management {
        let credits = request_json(authorized(
            client.get(format!("{base}/credits")),
            auth,
            super::USER_AGENT_VALUE,
        )?)
        .await?;
        usage.limits.balances.push(account_credits(&credits)?);
    }
    Ok(usage)
}

fn account_credits(payload: &Value) -> Result<CreditBalance, HttpError> {
    let data = payload.get("data").ok_or(HttpError::InvalidPayload)?;
    let allocated = data
        .get("total_credits")
        .and_then(nonnegative_amount)
        .ok_or(HttpError::InvalidPayload)?;
    let consumed = data
        .get("total_usage")
        .and_then(nonnegative_amount)
        .ok_or(HttpError::InvalidPayload)?;
    Ok(CreditBalance {
        title: "Account-wide credits".to_owned(),
        credits: UsageCredits {
            count: CreditCount::Full {
                allocated,
                consumed,
            },
            unit: CreditUnit::Currency(Currency::Usd),
        },
        expires_at: None,
    })
}

fn parse(payload: &Value) -> Result<AccountUsage, HttpError> {
    let data = payload
        .get("data")
        .filter(|data| data.is_object())
        .ok_or(HttpError::InvalidPayload)?;
    let mut usage = AccountUsage::default();
    for (field, title, window) in [
        ("usage", "Spent", None),
        ("usage_daily", "Spent", Some("day")),
        ("usage_weekly", "Spent", Some("week")),
        ("usage_monthly", "Spent", Some("month")),
        ("byok_usage", "BYOK Spent", None),
        ("byok_usage_daily", "BYOK Spent", Some("day")),
        ("byok_usage_weekly", "BYOK Spent", Some("week")),
        ("byok_usage_monthly", "BYOK Spent", Some("month")),
    ] {
        if let Some(used) = data.get(field).and_then(nonnegative_amount) {
            usage.limits.allowances.push(spending_allowance(
                title.to_owned(),
                Some(window.map_or(AllowanceWindow::AllTime, |window| {
                    AllowanceWindow::Named(window.to_owned())
                })),
                Some(used),
                None,
                CreditUnit::Currency(Currency::Usd),
            ));
        }
    }
    let allocated = data.get("limit").and_then(nonnegative_amount);
    let remaining = data.get("limit_remaining").and_then(amount);
    if allocated.is_some() || remaining.is_some() {
        let window = match text(data, "limit_reset") {
            Some("daily") => AllowanceWindow::Daily,
            Some("weekly") => AllowanceWindow::Weekly,
            Some("monthly") => AllowanceWindow::Monthly,
            None => AllowanceWindow::AllTime,
            Some(other) => AllowanceWindow::Named(other.to_owned()),
        };
        // Remaining is authoritative for the current budget period and BYOK
        // policy. Lifetime usage is an independent consumed-only allowance.
        let count = match (allocated, remaining) {
            (Some(allocated), Some(remaining)) => CreditCount::Full {
                allocated,
                consumed: allocated.subtract(remaining),
            },
            (None, Some(remaining)) => CreditCount::Remaining(remaining),
            (Some(allocated), None) => CreditCount::Allocated(allocated),
            (None, None) => CreditCount::Unknown,
        };
        usage.limits.allowances.push(UsageAllowance {
            title: "Key budget".to_owned(),
            window: Some(window),
            resets_at: None,
            credits: UsageCredits {
                count,
                unit: CreditUnit::Currency(Currency::Usd),
            },
        });
    }
    if usage.limits.allowances.is_empty() {
        return Err(HttpError::InvalidPayload);
    }
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::{account_credits, parse};
    use crate::dashboard::{AllowanceWindow, CreditAmount, CreditCount, CreditUnit, Currency};
    use serde_json::json;

    #[test]
    fn budget_uses_remaining_not_lifetime_spend_or_external_byok() -> eyre::Result<()> {
        let usage = parse(&json!({"data":{"usage":800,"usage_monthly":10,
            "byok_usage":400,"include_byok_in_limit":false,
            "limit":100,"limit_remaining":75,"limit_reset":"monthly"}}))
        .map_err(|error| eyre::eyre!("{error}"))?;
        let budget = usage
            .limits
            .allowances
            .iter()
            .find(|allowance| allowance.credits.allocated().is_some())
            .ok_or_else(|| eyre::eyre!("missing budget"))?;
        assert_eq!(budget.credits.consumed(), Some(CreditAmount::Integer(25)));
        assert_eq!(budget.credits.remaining(), Some(CreditAmount::Integer(75)));
        assert_eq!(budget.window, Some(AllowanceWindow::Monthly));
        assert_eq!(budget.credits.unit, CreditUnit::Currency(Currency::Usd));
        assert!(usage.limits.balances.is_empty());
        let unlimited = parse(&json!({"data":{"usage":42,"limit":null,"limit_remaining":null}}))
            .map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(
            unlimited.limits.allowances[0].credits.count,
            CreditCount::Consumed(CreditAmount::Integer(42))
        );
        assert_eq!(unlimited.limits.allowances[0].credits.used_percent(), None);
        Ok(())
    }

    #[test]
    fn budget_retains_signed_overdraw_and_large_integer_differences() -> eyre::Result<()> {
        let usage = parse(
            &json!({"data":{"limit":9_007_199_254_740_993_u64,"limit_remaining":9_007_199_254_740_992_u64}}),
        )
        .map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(
            usage.limits.allowances[0].credits.consumed(),
            Some(CreditAmount::Integer(1))
        );
        assert_eq!(
            usage.limits.allowances[0].credits.remaining(),
            Some(CreditAmount::Integer(9_007_199_254_740_992))
        );
        let overdraw = parse(&json!({"data":{"limit":100,"limit_remaining":-5}}))
            .map_err(|error| eyre::eyre!("{error}"))?;
        assert!(overdraw.limits.allowances[0]
            .credits
            .remaining()
            .is_some_and(|value| (value.as_f64() + 5.0).abs() < f64::EPSILON));
        assert!(overdraw.limits.allowances[0]
            .credits
            .used_percent()
            .is_some_and(|percent| (percent - 105.0).abs() < f64::EPSILON));
        Ok(())
    }

    #[test]
    fn remaining_only_has_no_invented_allocation_or_percentage() -> eyre::Result<()> {
        let usage =
            parse(&json!({"data":{"limit":null,"limit_remaining":-2.5,"limit_reset":"daily"}}))
                .map_err(|error| eyre::eyre!("{error}"))?;
        let budget = &usage.limits.allowances[0];
        assert_eq!(
            budget.credits.count,
            CreditCount::Remaining(CreditAmount::Decimal(-2.5))
        );
        assert_eq!(budget.credits.allocated(), None);
        assert_eq!(budget.credits.consumed(), None);
        assert_eq!(budget.credits.used_percent(), None);
        assert_eq!(budget.credits.unit, CreditUnit::Currency(Currency::Usd));
        assert_eq!(budget.window, Some(AllowanceWindow::Daily));
        Ok(())
    }

    #[test]
    fn allocated_only_budget_does_not_infer_spending_from_lifetime_usage() -> eyre::Result<()> {
        let usage = parse(&json!({"data":{"usage":500,"limit":100,"limit_remaining":null}}))
            .map_err(|error| eyre::eyre!("{error}"))?;
        let budget = usage
            .limits
            .allowances
            .last()
            .ok_or_else(|| eyre::eyre!("missing budget"))?;
        assert_eq!(
            budget.credits.count,
            CreditCount::Allocated(CreditAmount::Integer(100))
        );
        assert_eq!(budget.credits.allocated(), Some(CreditAmount::Integer(100)));
        assert_eq!(budget.credits.consumed(), None);
        assert_eq!(budget.credits.remaining(), None);
        assert_eq!(budget.credits.used_percent(), None);
        Ok(())
    }

    #[test]
    fn account_balance_preserves_exact_remainder_and_overdraw() -> eyre::Result<()> {
        let balance = account_credits(&json!({"data":{"total_credits":9_007_199_254_740_993_u64,"total_usage":9_007_199_254_740_992_u64}}))
            .map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(balance.credits.remaining(), Some(CreditAmount::Integer(1)));
        let negative = account_credits(&json!({"data":{"total_credits":10,"total_usage":12}}))
            .map_err(|error| eyre::eyre!("{error}"))?;
        assert_eq!(
            negative.credits.remaining(),
            Some(CreditAmount::Decimal(-2.0))
        );
        Ok(())
    }
}
