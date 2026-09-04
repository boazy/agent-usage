use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

const MILLIS_EPOCH_CUTOFF: u64 = 1_000_000_000_000;
const MILLIS_PER_SECOND: u64 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
    if value > MILLIS_EPOCH_CUTOFF {
        Millis::new(value)
    } else {
        Millis::new(value.saturating_mul(MILLIS_PER_SECOND))
    }
}

fn parse_rfc3339_to_ms(text: &str) -> Option<Millis> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 {
        return None;
    }

    let year = parse_i64(text.get(0..4)?)?;
    if bytes.get(4)? != &b'-' || bytes.get(7)? != &b'-' {
        return None;
    }
    let month = parse_i64(text.get(5..7)?)?;
    let day = parse_i64(text.get(8..10)?)?;
    if !matches!(bytes.get(10).copied(), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let hour = parse_i64(text.get(11..13)?)?;
    if bytes.get(13)? != &b':' {
        return None;
    }
    let minute = parse_i64(text.get(14..16)?)?;
    if bytes.get(16)? != &b':' {
        return None;
    }
    let second = parse_i64(text.get(17..19)?)?;

    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }

    let (millis, offset) = parse_fraction_and_offset(text.get(19..)?)?;
    let day_seconds = days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(hour.checked_mul(3_600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)?
        .checked_sub(offset)?;
    let milliseconds = day_seconds.checked_mul(1_000)?.checked_add(millis)?;
    u64::try_from(milliseconds).ok().map(Millis::new)
}

fn parse_i64(text: &str) -> Option<i64> {
    text.parse().ok()
}

fn parse_fraction_and_offset(rest: &str) -> Option<(i64, i64)> {
    let (fraction_millis, offset_text) = if let Some(fraction_and_offset) = rest.strip_prefix('.') {
        let digits = fraction_and_offset
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if digits == 0 {
            return None;
        }
        let (fraction, offset) = fraction_and_offset.split_at(digits);
        let millis = fraction
            .bytes()
            .take(3)
            .fold(0_i64, |value, digit| value * 10 + i64::from(digit - b'0'));
        let padding = 3_usize.saturating_sub(fraction.len().min(3));
        let scale = 10_i64.checked_pow(u32::try_from(padding).ok()?)?;
        (millis.checked_mul(scale)?, offset)
    } else {
        (0, rest)
    };

    let offset = match offset_text {
        "Z" | "z" => 0,
        _ if offset_text.len() == 6 => {
            let sign = match offset_text.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            if offset_text.as_bytes().get(3)? != &b':' {
                return None;
            }
            let hours = parse_i64(offset_text.get(1..3)?)?;
            let minutes = parse_i64(offset_text.get(4..6)?)?;
            if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
                return None;
            }
            sign * (hours * 3_600 + minutes * 60)
        }
        _ => return None,
    };

    Some((fraction_millis, offset))
}

const fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

const fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_of_year = (month + 9) % 12;
    let day_of_year = (153 * month_of_year + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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
    fn rejects_invalid_calendar_dates_and_lossy_numeric_forms() {
        assert_eq!(parse_timestamp_to_ms(&json!("2026-02-29T00:00:00Z")), None);
        assert_eq!(
            parse_timestamp_to_ms(&json!("2024-02-29T00:00:00Z")),
            Some(Millis::new(1_709_164_800_000))
        );
        assert_eq!(
            parse_timestamp_to_ms(&json!(1_000_000_000_000_u64)),
            Some(Millis::new(1_000_000_000_000_000))
        );
        assert_eq!(
            parse_timestamp_to_ms(&json!(9_007_199_254_740_993_u64)),
            Some(Millis::new(9_007_199_254_740_993))
        );
    }
}
