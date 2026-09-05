use crate::dashboard::{
    AccountInfo, AccountSnapshot, AccountUsage, BankedReset, CreditAmount, CreditCount, CreditUnit,
    Currency, UsageAllowance, UsageCredits,
};
use crate::theme::Theme;
use crate::time::{now_millis, Millis};
use crate::usage::human_duration;
use anstyle::{AnsiColor, Effects};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) const MIN_PANE_WIDTH: u16 = 44;
const PANE_PADDING: u16 = 1;

pub(crate) struct AccountPane {
    title: Line<'static>,
    lines: Vec<Line<'static>>,
    border: Style,
}

impl AccountPane {
    pub(crate) fn new(
        snapshot: &AccountSnapshot,
        width: u16,
        theme: &Theme,
        color: bool,
        show_account_id: bool,
    ) -> Self {
        let inner_width = width.saturating_sub(2 + 2 * PANE_PADDING).max(1);
        let mut lines = Vec::new();
        let title = Line::from(Span::styled(
            format!(
                " {} · {} ",
                account_label(&snapshot.account),
                clean(&snapshot.account.provider.to_string())
            ),
            style(theme.title, color),
        ));
        push_field(
            &mut lines,
            "Sources: ",
            snapshot.account.sources.join(", "),
            theme.label,
            theme.source,
            inner_width,
            color,
        );
        if let Some(name) = snapshot
            .account
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty() && *name != snapshot.account.label)
        {
            push_field(
                &mut lines,
                "Name: ",
                clean(name),
                theme.label,
                theme.account,
                inner_width,
                color,
            );
        }
        if let Some(id) = &snapshot.account.account_id {
            if show_account_id || !opaque_id(id) {
                push_field(
                    &mut lines,
                    "Account: ",
                    clean(id),
                    theme.label,
                    theme.account,
                    inner_width,
                    color,
                );
            }
        }
        if let Some(mask) = &snapshot.account.masked_key {
            push_field(
                &mut lines,
                "API key: ",
                clean(mask),
                theme.label,
                theme.account,
                inner_width,
                color,
            );
        }
        if let Some(error) = &snapshot.error {
            push_wrapped(
                &mut lines,
                format!("Unavailable: {}", clean(error)),
                style(theme.exhausted, color),
                inner_width,
            );
        } else if let Some(usage) = &snapshot.usage {
            append_usage(&mut lines, usage, inner_width, theme, color);
        } else {
            push_wrapped(
                &mut lines,
                "Loading…".to_owned(),
                style(theme.unknown, color),
                inner_width,
            );
        }
        for warning in &snapshot.warnings {
            push_wrapped(
                &mut lines,
                format!("Warning: {}", clean(warning)),
                style(theme.warning, color),
                inner_width,
            );
        }
        Self {
            title,
            lines,
            border: style(theme.border, color),
        }
    }

    pub(crate) fn height(&self) -> u16 {
        u16::try_from(self.lines.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2 + 2 * PANE_PADDING)
    }
}

impl Widget for &AccountPane {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.border)
            .padding(Padding::uniform(PANE_PADDING))
            .title(self.title.clone());
        let inner = block.inner(area);
        block.render(area, buffer);
        Paragraph::new(self.lines.clone()).render(inner, buffer);
    }
}

fn append_usage(
    lines: &mut Vec<Line<'static>>,
    usage: &AccountUsage,
    width: u16,
    theme: &Theme,
    color: bool,
) {
    if let Some(plan) = &usage.plan {
        push_field(
            lines,
            "Plan: ",
            clean(plan),
            theme.label,
            theme.plan,
            width,
            color,
        );
    }
    for allowance in &usage.limits.allowances {
        append_allowance(lines, allowance, width, theme, color);
    }
    for balance in &usage.limits.balances {
        push_field(
            lines,
            &format!("{}: ", clean(&balance.title)),
            credit_value(&balance.credits, balance.credits.remaining()),
            theme.meter,
            theme.account,
            width,
            color,
        );
        if let Some(expires) = balance.expires_at {
            push_reset(lines, "expires", Some(expires), width, theme, color);
        }
    }
    if !usage.limits.banked_resets.is_empty() {
        push_wrapped(
            lines,
            "Banked resets".to_owned(),
            style(theme.meter, color),
            width,
        );
        for reset in &usage.limits.banked_resets {
            append_banked_reset(lines, reset, width, theme, color);
        }
    }
    if let Some(resets) = usage.limits.global_reset_at {
        push_reset(
            lines,
            "subscription resets",
            Some(resets),
            width,
            theme,
            color,
        );
    }
    if usage.limits.allowances.is_empty()
        && usage.limits.balances.is_empty()
        && usage.limits.banked_resets.is_empty()
    {
        push_wrapped(
            lines,
            "No quota amounts reported".to_owned(),
            style(theme.unknown, color),
            width,
        );
    }
}

fn append_allowance(
    lines: &mut Vec<Line<'static>>,
    allowance: &UsageAllowance,
    width: u16,
    theme: &Theme,
    color: bool,
) {
    let mut heading = vec![Span::styled(
        clean(&allowance.title),
        style(theme.meter, color),
    )];
    if let Some(window) = &allowance.window {
        heading.push(Span::styled(
            format!(" ({})", clean(&window.to_string())),
            style(theme.window, color),
        ));
    }
    heading.push(Span::styled(
        format!(": {}", credit_value(&allowance.credits, None)),
        style(theme.account, color),
    ));
    push_styled(lines, heading, width);
    if let Some(percent) = allowance
        .credits
        .used_percent()
        .filter(|percent| percent.is_finite())
    {
        lines.push(gauge(
            width,
            percent,
            style(theme.bar_primary, color),
            style(theme.bar_background, color),
        ));
    }
    if let Some(resets) = allowance.resets_at {
        push_reset(lines, "resets", Some(resets), width, theme, color);
    }
}

fn credit_value(credits: &UsageCredits, display_remaining: Option<CreditAmount>) -> String {
    let unit = match &credits.unit {
        CreditUnit::Currency(Currency::Usd) => " USD".to_owned(),
        CreditUnit::Currency(Currency::Other(currency)) => format!(" {}", clean(currency)),
        CreditUnit::GenericCredits => " credits".to_owned(),
        CreditUnit::Percentage => "%".to_owned(),
        CreditUnit::Unknown => String::new(),
    };
    if let Some(remaining) = display_remaining {
        return format!("{}{unit} remaining", amount_value(remaining, &credits.unit));
    }
    match &credits.count {
        CreditCount::Full {
            allocated,
            consumed,
        } => {
            if credits.unit == CreditUnit::Percentage {
                if let Some(percent) = credits.used_percent() {
                    return format!("{percent:.1}%");
                }
            }
            format!(
                "{} / {}{unit}",
                amount_value(*consumed, &credits.unit),
                amount_value(*allocated, &credits.unit)
            )
        }
        CreditCount::Remaining(amount) => {
            format!("{}{unit} remaining", amount_value(*amount, &credits.unit))
        }
        CreditCount::Consumed(amount) => format!("{}{unit}", amount_value(*amount, &credits.unit)),
        CreditCount::Allocated(amount) => {
            format!("limit {}{unit}", amount_value(*amount, &credits.unit))
        }
        CreditCount::Unknown => "unknown".to_owned(),
    }
}

fn amount_value(amount: CreditAmount, unit: &CreditUnit) -> String {
    match (amount, unit) {
        (CreditAmount::Integer(value), CreditUnit::Currency(_)) => format!("{value}.00"),
        (CreditAmount::Decimal(value), CreditUnit::Currency(_)) => format!("{value:.2}"),
        _ => amount.to_string(),
    }
}

fn push_reset(
    lines: &mut Vec<Line<'static>>,
    verb: &str,
    at: Option<Millis>,
    width: u16,
    theme: &Theme,
    color: bool,
) {
    let (label, value) = reset_text(verb, at);
    push_field(
        lines,
        &label,
        value,
        theme.reset_label,
        theme.reset_time,
        width,
        color,
    );
}

fn reset_text(verb: &str, at: Option<Millis>) -> (String, String) {
    let value = at.map_or_else(|| "unknown".to_owned(), countdown);
    let label = if matches!(value.as_str(), "now" | "unknown") {
        format!("{verb} ")
    } else {
        format!("{verb} in ")
    };
    (label, value)
}

fn append_banked_reset(
    lines: &mut Vec<Line<'static>>,
    reset: &BankedReset,
    width: u16,
    theme: &Theme,
    color: bool,
) {
    let (label, value) = reset_text("expires", reset.expires_at);
    let mut available = usize::from(width);
    let value = truncate_text(&value, available);
    available = available.saturating_sub(value.width());
    let label = truncate_text(&label, available);
    available = available.saturating_sub(label.width());
    let count = match reset.count {
        Some(1) => String::new(),
        Some(count) => format!("×{count} · "),
        None => "count unknown · ".to_owned(),
    };
    let count = truncate_text(&count, available);
    available = available.saturating_sub(count.width());
    let title = if available >= 4 {
        format!("{} · ", truncate_text(&clean(&reset.title), available - 3))
    } else {
        String::new()
    };
    lines.push(Line::from(vec![
        Span::styled(format!("{title}{count}"), style(theme.account, color)),
        Span::styled(label, style(theme.reset_label, color)),
        Span::styled(value, style(theme.reset_time, color)),
    ]));
}

fn truncate_text(value: &str, width: usize) -> String {
    if value.width() <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut columns = 0;
    for character in value.chars() {
        let advance = character.width().unwrap_or(0);
        if columns + advance >= width {
            break;
        }
        result.push(character);
        columns += advance;
    }
    result.push('…');
    result
}

fn push_field(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    mut value: String,
    label_style: anstyle::Style,
    value_style: anstyle::Style,
    width: u16,
    color: bool,
) {
    value.retain(|character| !character.is_control());
    push_styled(
        lines,
        vec![
            Span::styled(label.to_owned(), style(label_style, color)),
            Span::styled(value, style(value_style, color)),
        ],
        width,
    );
}

fn countdown(then: Millis) -> String {
    now_millis().map_or_else(
        || "unknown".to_owned(),
        |now| {
            let seconds = now.seconds_until(then);
            if seconds == 0 {
                "now".to_owned()
            } else {
                human_duration(seconds)
            }
        },
    )
}

fn push_wrapped(lines: &mut Vec<Line<'static>>, text: String, style: Style, width: u16) {
    push_styled(lines, vec![Span::styled(text, style)], width);
}

fn push_styled(lines: &mut Vec<Line<'static>>, spans: Vec<Span<'static>>, width: u16) {
    let width = usize::from(width.max(1));
    let mut row = Vec::new();
    let mut columns = 0;
    for span in spans {
        let mut part = String::new();
        for character in span.content.chars() {
            let advance = character.width().unwrap_or(0);
            if columns + advance > width && columns != 0 {
                if !part.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut part), span.style));
                }
                lines.push(Line::from(std::mem::take(&mut row)));
                columns = 0;
            }
            if advance > width {
                part.push('�');
                columns += 1;
            } else {
                part.push(character);
                columns += advance;
            }
        }
        if !part.is_empty() {
            row.push(Span::styled(part, span.style));
        }
    }
    if !row.is_empty() {
        lines.push(Line::from(row));
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite usage is clamped to the pane width before conversion"
)]
fn gauge(width: u16, percent: f64, foreground: Style, background: Style) -> Line<'static> {
    let cells = percent.clamp(0.0, 100.0) * f64::from(width) / 100.0;
    let whole = cells.floor() as usize;
    let remainder = cells.fract();
    let partial = usize::from(whole < usize::from(width) && remainder > 0.0);
    let mut spans = vec![Span::styled("█".repeat(whole), foreground)];
    if partial != 0 {
        spans.push(Span::styled(
            if remainder >= 0.5 { "▓" } else { "▒" },
            foreground,
        ));
    }
    spans.push(Span::styled(
        "░".repeat(usize::from(width) - whole - partial),
        background,
    ));
    Line::from(spans)
}

pub(crate) struct PlacedPane {
    pub(crate) index: usize,
    pub(crate) x: u16,
    pub(crate) y: usize,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) pane: AccountPane,
}

pub(crate) fn layout_panes(
    snapshots: &[AccountSnapshot],
    indices: &[usize],
    width: u16,
    theme: &Theme,
    color: bool,
    show_account_id: bool,
    reserved: Option<Rect>,
) -> Vec<PlacedPane> {
    let width = width.max(1);
    let columns = usize::from((width / MIN_PANE_WIDTH).max(1));
    let column_count = u16::try_from(columns).unwrap_or(1);
    let base_width = width / column_count;
    let extra = width % column_count;
    let pane_widths: Vec<u16> = (0..columns)
        .map(|column| base_width + u16::from(column < usize::from(extra)))
        .collect();
    let pane_x: Vec<u16> = pane_widths
        .iter()
        .scan(0, |x, pane_width| {
            let result = *x;
            *x += *pane_width;
            Some(result)
        })
        .collect();
    let mut column_heights = vec![0usize; columns];
    let mut scores = vec![0usize; columns];
    if let Some(reserved) = reserved {
        for column in 0..columns {
            let right = pane_x[column] + pane_widths[column];
            if pane_x[column] < reserved.right() && right > reserved.x {
                scores[column] = usize::from(reserved.height);
            }
        }
    }
    let mut result = Vec::with_capacity(indices.len());
    for &index in indices {
        let column = scores
            .iter()
            .enumerate()
            .min_by_key(|&(column, score)| (*score, column))
            .map_or(0, |(column, _)| column);
        let pane = AccountPane::new(
            &snapshots[index],
            pane_widths[column],
            theme,
            color,
            show_account_id && indices.len() == 1,
        );
        let height = pane.height();
        let y = column_heights[column];
        column_heights[column] = y.saturating_add(usize::from(height));
        scores[column] = scores[column].saturating_add(usize::from(height));
        result.push(PlacedPane {
            index,
            x: pane_x[column],
            y,
            width: pane_widths[column],
            height,
            pane,
        });
    }
    result
}

pub(crate) fn report_width(explicit: Option<u16>) -> u16 {
    explicit
        .or_else(|| {
            if io::stdout().is_terminal() {
                crossterm::terminal::size().ok().map(|(width, _)| width)
            } else {
                None
            }
        })
        .unwrap_or(100)
        .max(1)
}

pub(crate) fn color_enabled() -> bool {
    anstream::AutoStream::auto(std::io::stdout()).current_choice() != anstream::ColorChoice::Never
}

pub(crate) fn render_report(
    snapshots: &[AccountSnapshot],
    width: u16,
    theme: &Theme,
) -> io::Result<()> {
    let color = color_enabled();
    let indices: Vec<_> = (0..snapshots.len()).collect();
    let panes = layout_panes(snapshots, &indices, width, theme, color, false, None);
    let mut output = anstream::AutoStream::auto(io::stdout().lock());
    if panes.is_empty() {
        return writeln!(
            output,
            "No accounts found. Check configured credential sources and exclusions."
        );
    }
    write_panes(&panes, width, &mut output, color)?;
    output.flush()
}

fn write_panes<W: Write>(
    panes: &[PlacedPane],
    width: u16,
    mut output: W,
    color: bool,
) -> io::Result<()> {
    let mut boundaries = BTreeSet::new();
    boundaries.insert(0usize);
    for pane in panes {
        boundaries.insert(pane.y);
        boundaries.insert(pane.y.saturating_add(usize::from(pane.height)));
    }
    let mut columns: Vec<Vec<usize>> = Vec::new();
    let mut starts = Vec::new();
    for (pane_index, pane) in panes.iter().enumerate() {
        if let Some(column) = starts.iter().position(|&x| x == pane.x) {
            columns[column].push(pane_index);
        } else {
            starts.push(pane.x);
            columns.push(vec![pane_index]);
        }
    }
    let mut cursors = vec![0usize; columns.len()];
    let mut buffers: Vec<Option<Buffer>> = (0..columns.len()).map(|_| None).collect();
    let boundaries: Vec<_> = boundaries.into_iter().collect();
    for pair in boundaries.windows(2) {
        let band_start = pair[0];
        let band_height = pair[1].saturating_sub(band_start);
        if band_height == 0 {
            continue;
        }
        let band_height = u16::try_from(band_height)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "pane band exceeds u16"))?;
        let area = Rect::new(0, 0, width.max(1), band_height);
        let mut buffer = Buffer::empty(area);
        for column in 0..columns.len() {
            while cursors[column] < columns[column].len() {
                let pane = &panes[columns[column][cursors[column]]];
                if pane.y.saturating_add(usize::from(pane.height)) <= band_start {
                    cursors[column] += 1;
                    buffers[column] = None;
                } else {
                    break;
                }
            }
            let Some(&pane_index) = columns[column].get(cursors[column]) else {
                continue;
            };
            let pane = &panes[pane_index];
            if pane.y > band_start || pane.y >= band_start + usize::from(band_height) {
                continue;
            }
            let pane_buffer = buffers[column].get_or_insert_with(|| {
                let mut pane_buffer = Buffer::empty(Rect::new(0, 0, pane.width, pane.height));
                (&pane.pane).render(pane_buffer.area, &mut pane_buffer);
                pane_buffer
            });
            let row_start = band_start.saturating_sub(pane.y);
            let row_end =
                (band_start + usize::from(band_height) - pane.y).min(usize::from(pane.height));
            for row in row_start..row_end {
                let dst_y = u16::try_from(pane.y + row - band_start).unwrap_or(0);
                for x in 0..pane.width {
                    buffer[(pane.x + x, dst_y)] =
                        pane_buffer[(x, u16::try_from(row).unwrap_or(0))].clone();
                }
            }
        }
        write_buffer(&buffer, area, &mut output, color)?;
    }
    Ok(())
}

pub(crate) fn write_buffer<W: Write>(
    buffer: &Buffer,
    area: Rect,
    mut output: W,
    color: bool,
) -> io::Result<()> {
    for y in area.y..area.bottom() {
        let mut x = area.x;
        let mut current = Style::default();
        while x < area.right() {
            let cell = &buffer[(x, y)];
            let cell_style = cell.style();
            if color && cell_style != current {
                write!(
                    output,
                    "{}{}",
                    anstyle::Reset,
                    ansi_style(cell_style).render()
                )?;
                current = cell_style;
            }
            write!(output, "{}", cell.symbol())?;
            let advance = u16::try_from(cell.symbol().width()).unwrap_or(1).max(1);
            x = x.saturating_add(advance);
        }
        if color {
            write!(output, "{}", anstyle::Reset)?;
        }
        writeln!(output)?;
    }
    Ok(())
}

pub(crate) fn style(value: anstyle::Style, enabled: bool) -> Style {
    if !enabled {
        return Style::default();
    }
    let mut result = Style::default();
    if let Some(color) = value.get_fg_color() {
        result = result.fg(ratatui_color(color));
    }
    if let Some(color) = value.get_bg_color() {
        result = result.bg(ratatui_color(color));
    }
    if value.get_effects().contains(Effects::BOLD) {
        result = result.add_modifier(Modifier::BOLD);
    }
    if value.get_effects().contains(Effects::ITALIC) {
        result = result.add_modifier(Modifier::ITALIC);
    }
    if value.get_effects().contains(Effects::UNDERLINE) {
        result = result.add_modifier(Modifier::UNDERLINED);
    }
    result
}

fn ratatui_color(value: anstyle::Color) -> Color {
    match value {
        anstyle::Color::Rgb(anstyle::RgbColor(red, green, blue)) => Color::Rgb(red, green, blue),
        anstyle::Color::Ansi256(anstyle::Ansi256Color(index)) => Color::Indexed(index),
        anstyle::Color::Ansi(color) => Color::Indexed(color as u8),
    }
}

fn ansi_style(value: Style) -> anstyle::Style {
    let mut result = anstyle::Style::new();
    if let Some(color) = value.fg.and_then(ansi_color) {
        result = result.fg_color(Some(color));
    }
    if let Some(color) = value.bg.and_then(ansi_color) {
        result = result.bg_color(Some(color));
    }
    if value.add_modifier.contains(Modifier::BOLD) {
        result = result.bold();
    }
    if value.add_modifier.contains(Modifier::ITALIC) {
        result = result.italic();
    }
    if value.add_modifier.contains(Modifier::UNDERLINED) {
        result = result.underline();
    }
    result
}

fn ansi_color(color: Color) -> Option<anstyle::Color> {
    Some(match color {
        Color::Reset => return None,
        Color::Rgb(red, green, blue) => anstyle::Color::Rgb(anstyle::RgbColor(red, green, blue)),
        Color::Indexed(index) => anstyle::Color::Ansi256(anstyle::Ansi256Color(index)),
        other => anstyle::Color::Ansi(match other {
            Color::Black => AnsiColor::Black,
            Color::Red => AnsiColor::Red,
            Color::Green => AnsiColor::Green,
            Color::Yellow => AnsiColor::Yellow,
            Color::Blue => AnsiColor::Blue,
            Color::Magenta => AnsiColor::Magenta,
            Color::Cyan => AnsiColor::Cyan,
            Color::Gray => AnsiColor::White,
            Color::DarkGray => AnsiColor::BrightBlack,
            Color::LightRed => AnsiColor::BrightRed,
            Color::LightGreen => AnsiColor::BrightGreen,
            Color::LightYellow => AnsiColor::BrightYellow,
            Color::LightBlue => AnsiColor::BrightBlue,
            Color::LightMagenta => AnsiColor::BrightMagenta,
            Color::LightCyan => AnsiColor::BrightCyan,
            Color::White => AnsiColor::BrightWhite,
            _ => return None,
        }),
    })
}

pub(crate) fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

pub(crate) fn opaque_id(value: &str) -> bool {
    let value = value.trim();
    let compact_len = value.bytes().filter(|byte| *byte != b'-').count();
    (compact_len >= 6
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-'))
        || (compact_len >= 16
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-'))
        || (value.len() >= 16
            && !value.contains(char::is_whitespace)
            && !value.contains('@')
            && value.bytes().any(|byte| byte.is_ascii_digit())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
}

pub(crate) fn short_account_id(account: &AccountInfo) -> String {
    let digest = Sha256::digest(account.id.as_bytes());
    format!("{:02x}{:02x}{:02x}", digest[0], digest[1], digest[2])
}

pub(crate) fn account_label(account: &AccountInfo) -> String {
    let mut label = clean(&account.label);
    if let Some(id) = account.account_id.as_deref().filter(|id| opaque_id(id)) {
        label
            .replace(id, "")
            .trim_matches([' ', '·', '-', '(', ')'])
            .clone_into(&mut label);
    }
    if label.is_empty() || opaque_id(&label) {
        return account
            .email
            .as_deref()
            .map(clean)
            .or_else(|| account.masked_key.as_deref().map(clean))
            .unwrap_or_else(|| format!("Account #{}", short_account_id(account)));
    }
    label
}

#[cfg(test)]
mod tests {
    use super::{gauge, write_buffer};
    use eyre::Result;
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::Style,
        widgets::{Paragraph, Widget},
    };

    #[test]
    fn wide_glyph_serializes_once_and_report_never_moves_cursor() -> Result<()> {
        let area = Rect::new(0, 0, 6, 1);
        let mut buffer = Buffer::empty(area);
        Paragraph::new("界x").render(area, &mut buffer);
        let mut output = Vec::new();
        write_buffer(&buffer, area, &mut output, false)?;
        assert_eq!(String::from_utf8(output)?, "界x   \n");
        Ok(())
    }

    #[test]
    fn gauge_has_exact_cell_width_and_four_distinct_levels() {
        for (percent, expected) in [
            (0.0, "░░░░░░░░░░"),
            (42.0, "████▒░░░░░"),
            (46.0, "████▓░░░░░"),
            (100.0, "██████████"),
        ] {
            let line = gauge(10, percent, Style::default(), Style::default());
            assert_eq!(line.to_string(), expected);
        }
    }

    #[test]
    fn gauge_preserves_theme_pair_at_all_usage_levels_and_unknown_has_no_bar() {
        use crate::dashboard::{
            CreditAmount, CreditCount, CreditUnit, UsageAllowance, UsageCredits,
        };
        for theme in crate::theme::BUILTIN_THEMES {
            let mut allowance = UsageAllowance {
                title: "Weekly".to_owned(),
                window: None,
                credits: UsageCredits {
                    count: CreditCount::Unknown,
                    unit: CreditUnit::Percentage,
                },
                resets_at: None,
            };
            for percent in [0.0, 42.0, 95.0, 100.0] {
                allowance.credits.count = CreditCount::Full {
                    allocated: CreditAmount::Integer(100),
                    consumed: CreditAmount::Decimal(percent),
                };
                let mut lines = Vec::new();
                super::append_allowance(&mut lines, &allowance, 40, theme, true);
                let bar = &lines[1];
                for span in bar.spans.iter().filter(|span| !span.content.is_empty()) {
                    let expected = if span.content.contains('░') {
                        theme.bar_background
                    } else {
                        theme.bar_primary
                    };
                    assert_eq!(span.style, super::style(expected, true));
                }
            }
            allowance.credits.count = CreditCount::Unknown;
            let mut lines = Vec::new();
            super::append_allowance(&mut lines, &allowance, 40, theme, false);
            assert!(!lines
                .iter()
                .any(|line| line.to_string().contains(['█', '▓', '▒', '░'])));
            assert!(lines
                .iter()
                .flat_map(|line| &line.spans)
                .all(|span| span.style == Style::default()));
        }
    }

    fn snapshot() -> crate::dashboard::AccountSnapshot {
        use crate::dashboard::{
            AccountInfo, AccountSnapshot, AccountUsage, CredentialKind, Provider,
            SubscriptionLimits,
        };
        AccountSnapshot {
            account: AccountInfo {
                id: "synthetic-identity".to_owned(),
                provider: Provider::Codex,
                label: "Example".to_owned(),
                name: None,
                email: Some("example@example.test".to_owned()),
                account_id: Some("93e4ca21-fb93-4d73-bfdc-688bfd73a012".to_owned()),
                sources: vec!["fixture".to_owned()],
                credential_kind: CredentialKind::OAuth,
                masked_key: None,
            },
            usage: Some(AccountUsage {
                limits: SubscriptionLimits::default(),
                plan: Some("Pro".to_owned()),
            }),
            error: None,
            warnings: Vec::new(),
            fetched_at: None,
        }
    }

    fn pane_text(
        snapshot: &crate::dashboard::AccountSnapshot,
        width: u16,
        color: bool,
        details: bool,
    ) -> Result<(String, Buffer)> {
        let pane = super::AccountPane::new(
            snapshot,
            width,
            &crate::theme::BUILTIN_THEMES[0],
            color,
            details,
        );
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, pane.height()));
        (&pane).render(buffer.area, &mut buffer);
        let mut output = Vec::new();
        write_buffer(&buffer, buffer.area, &mut output, color)?;
        Ok((String::from_utf8(output)?, buffer))
    }

    #[test]
    fn report_hides_opaque_identity_even_for_one_account_and_styles_semantic_fields() -> Result<()>
    {
        let mut snapshot = snapshot();
        for id in ["93e4ca21-fb93-4d73-bfdc-688bfd73a012", "491827495821746289"] {
            snapshot.account.account_id = Some(id.to_owned());
            snapshot.account.label = id.to_owned();
            let (overview, _) = pane_text(&snapshot, 100, false, false)?;
            assert!(!overview.contains(id));
            assert!(overview.contains("example@example.test"));
            assert!(!overview.contains('\u{1b}'));
            assert!(pane_text(&snapshot, 100, false, true)?.0.contains(id));
        }
        let (colored, buffer) = pane_text(&snapshot, 100, true, false)?;
        assert!(colored.contains("\u{1b}[38;2;"));
        assert_ne!(buffer[(2, 2)].fg, buffer[(11, 2)].fg);
        assert_ne!(buffer[(2, 3)].fg, buffer[(8, 3)].fg);
        for escape in colored.split('\u{1b}').skip(1) {
            let command = escape.chars().find(char::is_ascii_alphabetic);
            assert_eq!(
                command,
                Some('m'),
                "reports may style text but never move the cursor"
            );
        }
        Ok(())
    }

    #[test]
    fn banked_resets_group_known_unknown_and_aggregate_expiries_without_inventing_counts(
    ) -> Result<()> {
        use crate::dashboard::BankedReset;
        let mut snapshot = snapshot();
        snapshot
            .usage
            .as_mut()
            .ok_or_else(|| eyre::eyre!("fixture usage missing"))?
            .limits
            .banked_resets = vec![
            BankedReset {
                title: "Weekly".to_owned(),
                count: Some(1),
                expires_at: Some(crate::time::Millis::new(0)),
            },
            BankedReset {
                title: "Weekly".to_owned(),
                count: Some(4),
                expires_at: None,
            },
            BankedReset {
                title: "Monthly".to_owned(),
                count: None,
                expires_at: None,
            },
            BankedReset {
                title: "A deliberately long allowance title that must fit one line".to_owned(),
                count: Some(1),
                expires_at: super::now_millis()
                    .map(|now| crate::time::Millis::new(now.get() + 86_400_000)),
            },
        ];
        let (text, _) = pane_text(&snapshot, 44, false, false)?;
        assert_eq!(text.matches("Banked resets").count(), 1);
        assert!(text.contains("Weekly · expires now"));
        assert!(text.contains("Weekly · ×4 · expires unknown"));
        assert!(text.contains("count unknown · expires unknown"));
        assert_eq!(
            text.lines().filter(|line| line.contains("expires")).count(),
            4
        );
        assert_eq!(text.lines().count(), 11);
        assert!(text.contains("… · expires in "));
        let (_, buffer) = pane_text(&snapshot, 44, true, false)?;
        let row: String = (0..44).map(|x| buffer[(x, 8)].symbol()).collect();
        let offset = row
            .find("expires in ")
            .ok_or_else(|| eyre::eyre!("expiry label missing"))?;
        let x = u16::try_from(unicode_width::UnicodeWidthStr::width(&row[..offset]))?;
        assert_eq!(buffer[(x, 8)].fg, buffer[(x + 8, 8)].fg);
        assert_ne!(buffer[(x + 8, 8)].fg, buffer[(x + 11, 8)].fg);
        Ok(())
    }

    #[test]
    fn window_and_reset_styles_survive_wrapping_and_missing_usage_never_gets_a_bar() -> Result<()> {
        use crate::dashboard::{
            AllowanceWindow, CreditAmount, CreditCount, CreditUnit, UsageAllowance, UsageCredits,
        };
        use ratatui::style::Modifier;
        let mut snapshot = snapshot();
        snapshot
            .usage
            .as_mut()
            .ok_or_else(|| eyre::eyre!("fixture usage missing"))?
            .limits
            .allowances = vec![UsageAllowance {
            title: "Spent".to_owned(),
            window: Some(AllowanceWindow::Monthly),
            resets_at: Some(crate::time::Millis::new(0)),
            credits: UsageCredits {
                count: CreditCount::Allocated(CreditAmount::Integer(100)),
                unit: CreditUnit::GenericCredits,
            },
        }];
        let (text, buffer) = pane_text(&snapshot, 100, true, false)?;
        let plain: String = buffer
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(plain.contains("Spent (monthly): limit 100 credits"));
        assert!(!plain.contains(['█', '▓', '▒', '░']));
        assert!(buffer[(8, 4)].modifier.contains(Modifier::ITALIC));
        assert_ne!(buffer[(2, 5)].fg, buffer[(10, 5)].fg);
        assert!(text.contains("\u{1b}[3"));
        for width in [5, 12, 44] {
            let (text, buffer) = pane_text(&snapshot, width, false, true)?;
            assert!(text
                .lines()
                .all(|line| unicode_width::UnicodeWidthStr::width(line) == usize::from(width)));
            for row in 1..buffer.area.height - 1 {
                assert_eq!(buffer[(1, row)].symbol(), " ");
                assert_eq!(buffer[(width - 2, row)].symbol(), " ");
            }
        }
        Ok(())
    }
    #[test]
    fn masonry_packing_has_bounds_nonoverlap_and_beats_equal_row_height() {
        let mut snapshots = Vec::new();
        for warning_count in [0, 20, 0, 0] {
            let mut account = snapshot();
            account.warnings = (0..warning_count)
                .map(|index| format!("warning-{index}"))
                .collect();
            snapshots.push(account);
        }
        let width = 88;
        let indices: Vec<_> = (0..snapshots.len()).collect();
        let panes = super::layout_panes(
            &snapshots,
            &indices,
            width,
            &crate::theme::BUILTIN_THEMES[0],
            false,
            false,
            None,
        );
        for left in &panes {
            assert!(left.x + left.width <= width);
            for right in &panes {
                if std::ptr::eq(left, right) {
                    continue;
                }
                let x_overlap = left.x < right.x + right.width && right.x < left.x + left.width;
                let y_overlap = left.y < right.y + usize::from(right.height)
                    && right.y < left.y + usize::from(left.height);
                assert!(!(x_overlap && y_overlap));
            }
        }
        let packed_height = panes
            .iter()
            .map(|pane| pane.y + usize::from(pane.height))
            .max()
            .unwrap_or(0);
        let row_height = panes
            .chunks(2)
            .map(|row| {
                row.iter()
                    .map(|pane| usize::from(pane.height))
                    .max()
                    .unwrap_or(0)
            })
            .sum::<usize>();
        assert!(packed_height < row_height);
    }

    #[test]
    fn reserved_columns_seed_masonry_score_without_moving_pane_coordinates() {
        let snapshots = vec![snapshot(), snapshot()];
        let indices = vec![0, 1];
        let baseline = super::layout_panes(
            &snapshots,
            &indices,
            88,
            &crate::theme::BUILTIN_THEMES[0],
            false,
            false,
            None,
        );
        let reserved = super::layout_panes(
            &snapshots,
            &indices,
            88,
            &crate::theme::BUILTIN_THEMES[0],
            false,
            false,
            Some(Rect::new(44, 0, 44, 100)),
        );
        assert_eq!(reserved[0].x, 0);
        assert_eq!(reserved[0].y, 0);
        assert_eq!(baseline[1].x, 44);
        assert_eq!(reserved[1].x, 0);
        assert_ne!(reserved[1].x, baseline[1].x);
    }
    #[test]
    fn masonry_report_serializes_interleaved_panes_once_without_cursor_controls() -> Result<()> {
        let mut snapshots = Vec::new();
        for (index, warning_count) in [20, 0, 8, 1].into_iter().enumerate() {
            let mut account = snapshot();
            account.account.label = if index == 0 {
                "界-A".to_owned()
            } else {
                format!("account-{index}")
            };
            account.warnings = (0..warning_count)
                .map(|warning| format!("unique-{index}-{warning}!"))
                .collect();
            snapshots.push(account);
        }
        let indices: Vec<_> = (0..snapshots.len()).collect();
        let panes = super::layout_panes(
            &snapshots,
            &indices,
            88,
            &crate::theme::BUILTIN_THEMES[0],
            false,
            false,
            None,
        );
        let mut output = Vec::new();
        super::write_panes(&panes, 88, &mut output, false)?;
        let text = String::from_utf8(output)?;
        assert!(!text.contains('\u{1b}'));
        assert_eq!(text.matches("界-A").count(), 1);
        for index in 1..snapshots.len() {
            assert_eq!(text.matches(&format!("account-{index}")).count(), 1);
        }
        for marker in ["unique-0-1!", "unique-0-2!", "unique-2-7!", "unique-3-0!"] {
            assert_eq!(
                text.matches(marker).count(),
                1,
                "missing or duplicated {marker}"
            );
        }
        let mut colored = Vec::new();
        super::write_panes(&panes, 88, &mut colored, true)?;
        let colored = String::from_utf8(colored)?;
        for marker in ["unique-0-1!", "unique-2-7!", "unique-3-0!"] {
            assert_eq!(colored.matches(marker).count(), 1, "colored {marker}");
        }
        let expected_height = panes
            .iter()
            .map(|pane| pane.y + usize::from(pane.height))
            .max()
            .unwrap_or(0);
        assert_eq!(text.lines().count(), expected_height);
        Ok(())
    }
}
