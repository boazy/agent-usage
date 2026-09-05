use chrono::{DateTime, Datelike as _, Days, Months};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

const MILLIS_EPOCH_CUTOFF: u64 = 1_000_000_000_000;
const MILLIS_PER_SECOND: u64 = 1_000;

/// A remote wall-clock timestamp expressed as Unix epoch milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Millis(u64);

impl Millis {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    pub(crate) fn saturating_add_seconds(self, seconds: u64) -> Self {
        Self(
            self.0
                .saturating_add(seconds.saturating_mul(MILLIS_PER_SECOND)),
        )
    }

    pub(crate) fn seconds_until(self, later: Self) -> u64 {
        later.0.saturating_sub(self.0) / MILLIS_PER_SECOND
    }
}

pub(crate) fn now_millis() -> Option<Millis> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .map(Millis::new)
}

pub(crate) fn parse_timestamp_to_ms(value: &Value) -> Option<Millis> {
    match value {
        Value::Number(number) => number.as_u64().map(epoch_value_to_ms),
        Value::String(text) => parse_timestamp_text(text.trim()),
        _ => None,
    }
}

fn parse_timestamp_text(text: &str) -> Option<Millis> {
    if text.is_empty() {
        return None;
    }

    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        return text.parse::<u64>().ok().map(epoch_value_to_ms);
    }

    parse_rfc3339_to_ms(text)
}

fn epoch_value_to_ms(value: u64) -> Millis {
    if value >= MILLIS_EPOCH_CUTOFF {
        Millis::new(value)
    } else {
        Millis::new(value.saturating_mul(MILLIS_PER_SECOND))
    }
}

fn parse_rfc3339_to_ms(text: &str) -> Option<Millis> {
    let timestamp = DateTime::parse_from_rfc3339(text).ok()?.timestamp_millis();
    u64::try_from(timestamp).ok().map(Millis::new)
}

/// Cursor's documented start-date fallback uses JavaScript's UTC-month rollover.
pub(crate) fn next_month(value: Millis) -> Option<Millis> {
    let date = DateTime::from_timestamp_millis(i64::try_from(value.get()).ok()?)?;
    let next = date
        .with_day(1)?
        .checked_add_months(Months::new(1))?
        .checked_add_days(Days::new(u64::from(date.day() - 1)))?;
    u64::try_from(next.timestamp_millis()).ok().map(Millis::new)
}

#[cfg(test)]
mod tests {
    use super::{parse_timestamp_to_ms, Millis};
    use serde_json::json;
    #[test]
    fn parses_epoch_milliseconds_and_rfc3339() {
        assert_eq!(
            parse_timestamp_to_ms(&json!(1_788_295_133)),
            Some(Millis::new(1_788_295_133_000))
        );
        assert_eq!(
            parse_timestamp_to_ms(&json!("1788295133000")),
            Some(Millis::new(1_788_295_133_000))
        );
        assert_eq!(
            parse_timestamp_to_ms(&json!("2026-09-01T00:00:00Z")),
            Some(Millis::new(1_788_220_800_000))
        );
        assert_eq!(
            parse_timestamp_to_ms(&json!("2026-09-01T00:00:00.123Z")),
            Some(Millis::new(1_788_220_800_123))
        );
    }

    #[test]
    fn calendar_month_rollover_matches_cursor_utc_contract() -> eyre::Result<()> {
        let start = parse_timestamp_to_ms(&json!("2026-01-31T12:34:56.789Z"))
            .ok_or_else(|| eyre::eyre!("missing start"))?;
        assert_eq!(
            super::next_month(start),
            parse_timestamp_to_ms(&json!("2026-03-03T12:34:56.789Z"))
        );
        assert_eq!(super::next_month(Millis::new(u64::MAX)), None);
        Ok(())
    }

    #[test]
    fn rejects_invalid_calendar_dates_and_lossy_numeric_forms() {
        assert_eq!(parse_timestamp_to_ms(&json!("2026-02-29T00:00:00Z")), None);
        assert_eq!(
            parse_timestamp_to_ms(&json!("2024-02-29T00:00:00Z")),
            Some(Millis::new(1_709_164_800_000))
        );
        assert_eq!(
            parse_timestamp_to_ms(&json!(1_000_000_000_000_u64)),
            Some(Millis::new(1_000_000_000_000))
        );
        assert_eq!(
            parse_timestamp_to_ms(&json!(9_007_199_254_740_993_u64)),
            Some(Millis::new(9_007_199_254_740_993))
        );
    }
}
