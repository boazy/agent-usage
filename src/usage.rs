use crate::time::{parse_timestamp_to_ms, Millis};
use serde_json::{Map, Value};

const WARNING_THRESHOLD: f64 = 0.9;

#[derive(Clone)]
pub(crate) struct UsageWindow {
    pub(crate) used_percent: Option<f64>,
    pub(crate) limit_window_seconds: Option<u64>,
    pub(crate) reset_after_seconds: Option<u64>,
    pub(crate) reset_at: Option<Millis>,
}

#[derive(Clone)]
pub(crate) struct RateLimit {
    pub(crate) allowed: Option<bool>,
    pub(crate) limit_reached: Option<bool>,
    pub(crate) primary_window: Option<UsageWindow>,
    pub(crate) secondary_window: Option<UsageWindow>,
}

#[derive(Clone)]
pub(crate) struct AdditionalRateLimit {
    pub(crate) limit_name: Option<String>,
    pub(crate) metered_feature: Option<String>,
    pub(crate) rate_limit: Option<RateLimit>,
}

#[derive(Clone)]
pub(crate) struct ResetCredit {
    pub(crate) title: Option<String>,
    pub(crate) expires_at: Option<Millis>,
}

#[derive(Clone)]
pub(crate) struct ParsedUsage {
    pub(crate) plan_type: Option<String>,
    pub(crate) rate_limit: Option<RateLimit>,
    pub(crate) additional_rate_limits: Vec<AdditionalRateLimit>,
    pub(crate) reset_credits_available: Option<u64>,
    pub(crate) reset_credits: Option<Vec<ResetCredit>>,
    pub(crate) raw: Value,
}

#[derive(Clone)]
pub(crate) struct UsageItem {
    pub(crate) meter: String,
    pub(crate) limit_window_seconds: Option<u64>,
    pub(crate) used_percent: Option<f64>,
    pub(crate) status: UsageStatus,
    pub(crate) reset_at: Option<Millis>,
}

#[derive(Clone)]
pub(crate) struct BankReset {
    pub(crate) source: String,
    pub(crate) expires_at: Option<Millis>,
    pub(crate) count: Option<u64>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum UsageStatus {
    Healthy,
    Warning,
    Exhausted,
    Unknown,
}

pub(crate) fn parse_usage_payload(payload: Value) -> ParsedUsage {
    let raw_object = payload.as_object();
    let plan_type = raw_object
        .and_then(|object| object.get("plan_type"))
        .and_then(as_string);
    let rate_limit = raw_object
        .and_then(|object| object.get("rate_limit"))
        .and_then(Value::as_object)
        .and_then(parse_rate_limit);
    let additional_rate_limits = raw_object
        .and_then(|object| object.get("additional_rate_limits"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(parse_additional_rate_limit)
                .collect()
        })
        .unwrap_or_default();
    let reset_credits_available = raw_object
        .and_then(|object| object.get("rate_limit_reset_credits"))
        .and_then(Value::as_object)
        .and_then(|object| object.get("available_count"))
        .and_then(as_u64);
    ParsedUsage {
        plan_type,
        rate_limit,
        additional_rate_limits,
        reset_credits_available,
        reset_credits: None,
        raw: payload,
    }
}

pub(crate) fn parse_reset_credits(payload: &Value) -> Option<Vec<ResetCredit>> {
    payload.get("credits")?.as_array().map(|credits| {
        credits
            .iter()
            .filter_map(|credit| {
                let object = credit.as_object()?;
                if object
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|status| status != "available")
                {
                    return None;
                }
                Some(ResetCredit {
                    title: object
                        .get("title")
                        .and_then(as_string)
                        .filter(|title| !title.trim().is_empty()),
                    expires_at: object.get("expires_at").and_then(parse_timestamp_to_ms),
                })
            })
            .collect()
    })
}

pub(crate) fn collect_items(payload: &ParsedUsage, now: Option<Millis>) -> Vec<UsageItem> {
    let mut items = Vec::new();
    if let Some(rate_limit) = &payload.rate_limit {
        collect_rate_limit_items(&mut items, "Codex", rate_limit, now);
    }
    for additional in &payload.additional_rate_limits {
        let slug = additional_limit_slug(
            additional.limit_name.as_deref(),
            additional.metered_feature.as_deref(),
        );
        let display_name = match slug.as_str() {
            "spark" => "Spark".to_owned(),
            "chat" => "Codex".to_owned(),
            "gpt-reserve" => "Reserve".to_owned(),
            _ => additional
                .limit_name
                .as_deref()
                .map_or_else(|| title_case_slug(&slug), normalize_usage_label),
        };
        if let Some(rate_limit) = &additional.rate_limit {
            collect_rate_limit_items(&mut items, &display_name, rate_limit, now);
        }
    }
    items
}

fn collect_rate_limit_items(
    items: &mut Vec<UsageItem>,
    meter: &str,
    rate_limit: &RateLimit,
    now: Option<Millis>,
) {
    for window in [&rate_limit.primary_window, &rate_limit.secondary_window]
        .into_iter()
        .flatten()
    {
        items.push(build_usage_item(
            meter,
            window,
            rate_limit.allowed,
            rate_limit.limit_reached,
            now,
        ));
    }
}

pub(crate) fn collect_banked_resets(payload: &ParsedUsage) -> Vec<BankReset> {
    payload.reset_credits.as_ref().map_or_else(
        || {
            payload
                .reset_credits_available
                .filter(|count| *count > 0)
                .map(|count| {
                    vec![BankReset {
                        source: "Reset credits available".to_owned(),
                        expires_at: None,
                        count: Some(count),
                    }]
                })
                .unwrap_or_default()
        },
        |credits| {
            credits
                .iter()
                .map(|credit| BankReset {
                    source: credit
                        .title
                        .clone()
                        .unwrap_or_else(|| "Reset credit".to_owned()),
                    expires_at: credit.expires_at,
                    count: None,
                })
                .collect()
        },
    )
}

fn build_usage_item(
    meter: &str,
    window: &UsageWindow,
    allowed: Option<bool>,
    limit_reached: Option<bool>,
    now: Option<Millis>,
) -> UsageItem {
    UsageItem {
        meter: meter.to_owned(),
        limit_window_seconds: window.limit_window_seconds,
        used_percent: window.used_percent,
        status: usage_status(window.used_percent, allowed, limit_reached),
        reset_at: resolve_reset_time(window, now),
    }
}

fn parse_rate_limit(raw: &Map<String, Value>) -> Option<RateLimit> {
    let allowed = raw.get("allowed").and_then(Value::as_bool);
    let limit_reached = raw.get("limit_reached").and_then(Value::as_bool);
    let primary_window = raw
        .get("primary_window")
        .and_then(Value::as_object)
        .and_then(parse_usage_window);
    let secondary_window = raw
        .get("secondary_window")
        .and_then(Value::as_object)
        .and_then(parse_usage_window);
    (allowed.is_some()
        || limit_reached.is_some()
        || primary_window.is_some()
        || secondary_window.is_some())
    .then_some(RateLimit {
        allowed,
        limit_reached,
        primary_window,
        secondary_window,
    })
}

fn parse_usage_window(raw: &Map<String, Value>) -> Option<UsageWindow> {
    let used_percent = raw.get("used_percent").and_then(as_f64);
    let limit_window_seconds = raw.get("limit_window_seconds").and_then(as_u64);
    let reset_after_seconds = raw.get("reset_after_seconds").and_then(as_u64);
    let reset_at = raw.get("reset_at").and_then(parse_timestamp_to_ms);
    (used_percent.is_some()
        || limit_window_seconds.is_some()
        || reset_after_seconds.is_some()
        || reset_at.is_some())
    .then_some(UsageWindow {
        used_percent,
        limit_window_seconds,
        reset_after_seconds,
        reset_at,
    })
}

fn parse_additional_rate_limit(raw: &Value) -> Option<AdditionalRateLimit> {
    let object = raw.as_object()?;
    let limit_name = object.get("limit_name").and_then(as_string);
    let metered_feature = object.get("metered_feature").and_then(as_string);
    let rate_limit = object
        .get("rate_limit")
        .and_then(Value::as_object)
        .and_then(parse_rate_limit);
    (limit_name.is_some() || metered_feature.is_some() || rate_limit.is_some()).then_some(
        AdditionalRateLimit {
            limit_name,
            metered_feature,
            rate_limit,
        },
    )
}

pub(crate) fn usage_status(
    used_percent: Option<f64>,
    explicitly_allowed: Option<bool>,
    limit_reached: Option<bool>,
) -> UsageStatus {
    let used_fraction = used_percent
        .filter(|percent| percent.is_finite())
        .map(|percent| (percent / 100.0).clamp(0.0, 1.0));
    match used_fraction {
        None => UsageStatus::Unknown,
        Some(fraction)
            if fraction >= 1.0
                && explicitly_allowed == Some(true)
                && limit_reached != Some(true) =>
        {
            UsageStatus::Warning
        }
        Some(fraction) if fraction >= 1.0 => UsageStatus::Exhausted,
        Some(fraction) if fraction >= WARNING_THRESHOLD => UsageStatus::Warning,
        Some(_) => UsageStatus::Healthy,
    }
}

fn additional_limit_slug(limit_name: Option<&str>, metered_feature: Option<&str>) -> String {
    let probe = format!(
        "{} {}",
        limit_name.unwrap_or(""),
        metered_feature.unwrap_or("")
    )
    .to_ascii_lowercase();
    if probe.contains("spark") || probe.contains("bengalfox") {
        return "spark".to_owned();
    }
    if probe.contains("gpt-reserve") {
        return "gpt-reserve".to_owned();
    }
    let slug = slugify(metered_feature.or(limit_name).unwrap_or("extra"));
    if slug.is_empty() {
        "extra".to_owned()
    } else {
        slug
    }
}

fn normalize_usage_label(name: &str) -> String {
    title_case_slug(&slugify(name))
}

fn slugify(input: &str) -> String {
    let normalized = input.trim().to_ascii_lowercase();
    let input = normalized
        .trim_start_matches("codex-")
        .trim_start_matches("codex_");
    let mut slug = String::with_capacity(input.len());
    let mut previous_was_dash = false;
    for character in input.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_was_dash = false;
        } else if matches!(character, '-' | '_') {
            if !previous_was_dash {
                slug.push('-');
            }
            previous_was_dash = true;
        } else {
            slug.push(' ');
            previous_was_dash = false;
        }
    }
    slug.trim_matches('-').to_owned()
}

fn title_case_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters
                .next()
                .map(|first| format!("{}{}", first.to_ascii_uppercase(), characters.as_str()))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn resolve_reset_time(window: &UsageWindow, now: Option<Millis>) -> Option<Millis> {
    window.reset_at.or_else(|| {
        now.zip(window.reset_after_seconds)
            .map(|(now, seconds)| now.saturating_add_seconds(seconds))
    })
}

pub(crate) fn window_label(seconds: Option<u64>) -> String {
    match seconds {
        Some(seconds) if seconds >= 86_400 => {
            plural_duration(rounded_units(seconds, 86_400), "day")
        }
        Some(seconds) if seconds >= 3_600 => plural_duration(rounded_units(seconds, 3_600), "hour"),
        Some(seconds) => plural_duration(rounded_units(seconds, 60), "minute"),
        None => "unknown".to_owned(),
    }
}

fn rounded_units(seconds: u64, unit: u64) -> u64 {
    seconds / unit + u64::from(seconds % unit >= unit / 2)
}

fn plural_duration(value: u64, unit: &str) -> String {
    format!("{value} {unit}{}", if value == 1 { "" } else { "s" })
}

pub(crate) fn human_duration(total_seconds: u64) -> String {
    if total_seconds == 0 {
        return "0s".to_owned();
    }
    let mut parts = Vec::new();
    for (value, suffix) in [
        (total_seconds / 86_400, "d"),
        ((total_seconds % 86_400) / 3_600, "h"),
        ((total_seconds % 3_600) / 60, "m"),
        (total_seconds % 60, "s"),
    ] {
        if value > 0 {
            parts.push(format!("{value}{suffix}"));
        }
    }
    parts.into_iter().take(3).collect::<Vec<_>>().join(" ")
}

fn as_string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}
fn as_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}
fn as_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::{parse_usage_payload, usage_status, UsageStatus};
    use serde_json::json;

    #[test]
    fn window_labels_round_to_nearest_unit() {
        assert_eq!(super::window_label(Some(5_399)), "1 hour");
        assert_eq!(super::window_label(Some(5_400)), "2 hours");
        assert_eq!(super::window_label(Some(59)), "1 minute");
        assert_eq!(super::window_label(Some(129_600)), "2 days");
        assert_eq!(super::rounded_units(u64::MAX, 60), u64::MAX / 60);
    }

    #[test]
    fn status_respects_endpoint_flags_at_full_usage() {
        assert_eq!(
            usage_status(Some(89.9), Some(false), Some(false)),
            UsageStatus::Healthy
        );
        assert_eq!(
            usage_status(Some(90.0), Some(false), Some(false)),
            UsageStatus::Warning
        );
        assert_eq!(
            usage_status(Some(100.0), Some(true), Some(false)),
            UsageStatus::Warning
        );
        assert_eq!(
            usage_status(Some(100.0), Some(true), Some(true)),
            UsageStatus::Exhausted
        );
        assert_eq!(
            usage_status(Some(100.0), None, None),
            UsageStatus::Exhausted
        );
        assert_eq!(
            usage_status(None, Some(true), Some(false)),
            UsageStatus::Unknown
        );
    }

    #[test]
    fn usage_window_accepts_rfc3339_reset_timestamp() {
        let usage = parse_usage_payload(
            json!({"rate_limit":{"primary_window":{"used_percent":5,"reset_at":"2026-09-01T00:00:00Z"}}}),
        );
        assert_eq!(
            usage
                .rate_limit
                .unwrap_or_else(|| panic!("missing rate limit"))
                .primary_window
                .unwrap_or_else(|| panic!("missing window"))
                .reset_at
                .map(super::super::time::Millis::get),
            Some(1_788_220_800_000)
        );
    }
}
