use crate::auth::AuthRecord;
use crate::theme::{paint, Theme, BOLD};
use crate::time::{now_millis, Millis};
use crate::usage::{
    collect_banked_resets, collect_items, human_duration, window_label, BankReset, ParsedUsage,
    UsageItem, UsageStatus,
};
use anstyle::Style;
use std::fmt::Write as _;
use std::io::{IsTerminal, Write};

const METER_WIDTH: usize = 12;
const WINDOW_WIDTH: usize = 11;
const PREFIX_WIDTH: usize = 22;
const BAR_WIDTH: usize = 44;

pub(crate) fn print_usage_report(
    usage: &ParsedUsage,
    auth: &AuthRecord,
    theme: Theme,
    interactive: bool,
) {
    let interactive = interactive && output_is_interactive();
    print_header("Codex plan usage", "================", theme);
    anstream::println!("{}", format_account_plan_line(auth, usage, theme));
    anstream::println!();

    let now = now_millis();
    let items = collect_items(usage, now);
    if items.is_empty() {
        anstream::println!("No usage windows were reported by this endpoint.");
        return;
    }
    for item in &items {
        render_usage_item(item, theme, interactive, now);
    }
    print_banked_resets(&collect_banked_resets(usage), theme, interactive, now);
}

fn output_is_interactive() -> bool {
    std::io::stdout().is_terminal() && std::io::stderr().is_terminal()
}

fn print_header(title: &str, underline: &str, theme: Theme) {
    anstream::println!("{}", paint(theme.meter, title));
    anstream::println!("{}", paint(theme.window, underline));
}

pub(crate) fn format_account_plan_line(
    auth: &AuthRecord,
    usage: &ParsedUsage,
    theme: Theme,
) -> String {
    let account = auth.email.as_deref().unwrap_or("unknown");
    let plan = usage.plan_type.as_deref().unwrap_or("unknown");
    format!(
        "{} {} {}",
        paint(theme.window, "Account:"),
        paint(theme.meter, account),
        paint(theme.reset, &format!("({plan})"))
    )
}

fn print_banked_resets(resets: &[BankReset], theme: Theme, interactive: bool, now: Option<Millis>) {
    if resets.is_empty() {
        return;
    }
    if interactive {
        let _ = anstream::stderr().write_all(b"\n");
    } else {
        anstream::println!();
    }
    print_header("Bank resets", "------------", theme);
    for reset in resets {
        anstream::println!(
            "  {} {} — {}",
            paint(theme.window, "•"),
            paint(theme.meter, &reset.source),
            paint(theme.reset, &format_banked_reset(reset, now))
        );
    }
}

fn format_banked_reset(reset: &BankReset, now: Option<Millis>) -> String {
    if let Some(count) = reset.count {
        return format!("{count} available; expiry not reported");
    }
    match (reset.expires_at, now) {
        (Some(expiry), Some(now)) => {
            format!("expires in {}", human_duration(now.seconds_until(expiry)))
        }
        (Some(_), None) => "expiry reported; current time unavailable".to_owned(),
        (None, _) => "no expiry reported".to_owned(),
    }
}

fn render_usage_item(item: &UsageItem, theme: Theme, interactive: bool, now: Option<Millis>) {
    let (status_label, status_style) = match item.status {
        UsageStatus::Healthy => (None, theme.meter),
        UsageStatus::Warning => (Some("warning"), theme.warning),
        UsageStatus::Exhausted => (Some("exhausted"), theme.exhausted),
        UsageStatus::Unknown => (Some("unknown"), theme.unknown),
    };
    let meter = fit_width(&item.meter, METER_WIDTH);
    let window = fit_width(&window_label(item.limit_window_seconds), WINDOW_WIDTH);
    let prefix = format!(
        "{} {} ",
        paint_bold(theme.meter, &meter),
        paint(theme.window, &format!("({window})"))
    );
    let mut line = item.used_percent.map_or_else(
        || "[unknown usage percentage]".to_owned(),
        |percent| paint(theme.meter, &format!("{:>5.1}%", percent.clamp(0.0, 100.0))),
    );
    if let Some(reset) = item
        .reset_at
        .zip(now)
        .map(|(reset, now)| format!("resets in {}", human_duration(now.seconds_until(reset))))
    {
        line.push(' ');
        line.push_str(&paint(theme.reset, &reset));
    }
    if let Some(status) = status_label {
        line.push(' ');
        line.push_str(&paint(status_style, status));
    }

    if interactive && item.used_percent.is_some() {
        let filled = item.used_percent.map_or(0, rounded_percent);
        render_usage_bar(&prefix, &line, theme, filled);
    } else {
        anstream::println!("{prefix:<PREFIX_WIDTH$} {line}");
    }
}

fn render_usage_bar(prefix: &str, line: &str, theme: Theme, filled: u64) {
    let bar = render_bar_cells(BAR_WIDTH, theme.bar_primary, theme.bar_background, filled);
    anstream::println!("{prefix:<PREFIX_WIDTH$}{bar} {line}");
}

/// The bar keeps solid, partial, and muted cells as distinct visual levels.
fn render_bar_cells(width: usize, fill: Style, background: Style, filled: u64) -> String {
    let filled = usize::try_from(filled.min(100)).unwrap_or(100);
    let filled_cells = filled.saturating_mul(width);
    let whole = filled_cells / 100;
    let fraction = filled_cells % 100;
    let mut output = String::with_capacity(width * 4);
    if whole > 0 {
        let _ = write!(&mut output, "{}", fill.render());
        output.extend(std::iter::repeat_n('█', whole));
        let _ = write!(&mut output, "{}", fill.render_reset());
    }
    let partial = usize::from(whole < width && fraction > 0);
    if partial == 1 {
        let cell = if fraction < 50 { '▓' } else { '▒' };
        output.push_str(&paint(fill, &cell.to_string()));
    }
    let empty = width - whole - partial;
    if empty > 0 {
        let _ = write!(&mut output, "{}", background.render());
        output.extend(std::iter::repeat_n('░', empty));
        let _ = write!(&mut output, "{}", background.render_reset());
    }
    output
}

fn paint_bold(style: Style, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    format!(
        "{}{}{text}{}",
        BOLD.render(),
        style.render(),
        style.render_reset()
    )
}

fn fit_width(value: &str, width: usize) -> String {
    let truncated: String = value.chars().take(width).collect();
    format!("{truncated:<width$}")
}

fn rounded_percent(percent: f64) -> u64 {
    if !percent.is_finite() {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the caller clamps the finite percentage to 0 through 100 before conversion"
    )]
    #[expect(
        clippy::cast_sign_loss,
        reason = "the caller clamps the finite percentage to a non-negative value before conversion"
    )]
    {
        percent.clamp(0.0, 100.0).round() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::render_bar_cells;
    use crate::theme::BUILTIN_THEMES;

    #[test]
    fn bar_geometry_has_exact_width_at_boundaries_and_partial_fill() {
        for filled in [0, 42, 100] {
            let bar = render_bar_cells(
                10,
                BUILTIN_THEMES[0].bar_primary,
                BUILTIN_THEMES[0].bar_background,
                filled,
            );
            assert_eq!(
                bar.chars()
                    .filter(|character| matches!(character, '█' | '▓' | '▒' | '░'))
                    .count(),
                10
            );
        }
    }
}
