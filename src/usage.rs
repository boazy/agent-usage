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
