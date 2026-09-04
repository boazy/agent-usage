use crate::dashboard::{AccountSnapshot, UsageMetric};
use crate::theme::Theme;
use crate::time::{now_millis, Millis};
use crate::usage::human_duration;
use anstyle::{AnsiColor, Effects};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::io::{self, Write};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) const MIN_PANE_WIDTH: u16 = 44;

pub(crate) struct AccountPane {
    title: Line<'static>,
    lines: Vec<Line<'static>>,
    border: Style,
}

impl AccountPane {
    pub(crate) fn new(snapshot: &AccountSnapshot, width: u16, theme: Theme, color: bool) -> Self {
        let inner_width = width.saturating_sub(2).max(1);
        let mut lines = Vec::new();
        let muted = style(theme.window, color);
        let title = Line::from(Span::styled(
            format!(" {} · {} ", clean(&snapshot.account.label), snapshot.account.provider),
            style(theme.meter, color).add_modifier(Modifier::BOLD),
        ));
        push_wrapped(&mut lines, format!("Sources: {}", snapshot.account.sources.join(", ")), muted, inner_width);
        if let Some(id) = &snapshot.account.account_id {
            push_wrapped(&mut lines, format!("Account: {}", clean(id)), muted, inner_width);
        }
        if let Some(mask) = &snapshot.account.masked_key {
            push_wrapped(&mut lines, format!("API key: {mask}"), muted, inner_width);
        }
        if let Some(error) = &snapshot.error {
            push_wrapped(&mut lines, format!("Unavailable: {}", clean(error)), style(theme.exhausted, color), inner_width);
        } else if let Some(usage) = &snapshot.usage {
            if let Some(plan) = &usage.plan {
                push_wrapped(&mut lines, format!("Plan: {}", clean(plan)), style(theme.meter, color), inner_width);
            }
            for metric in &usage.windows {
                append_metric(&mut lines, metric, inner_width, theme, color);
            }
            for credit in &usage.credits {
                push_wrapped(&mut lines, format!("{}: {} {}", clean(&credit.name), credit.balance, clean(&credit.unit)), style(theme.meter, color), inner_width);
                if let Some(expires) = credit.expires_at {
                    push_wrapped(&mut lines, format!("  expires {}", countdown(expires)), style(theme.reset, color), inner_width);
                }
            }
            if usage.windows.is_empty() && usage.credits.is_empty() {
                push_wrapped(&mut lines, "No quota amounts reported".to_owned(), style(theme.unknown, color), inner_width);
            }
        } else {
            push_wrapped(&mut lines, "Loading…".to_owned(), style(theme.unknown, color), inner_width);
        }
        for warning in &snapshot.warnings {
            push_wrapped(&mut lines, format!("Warning: {}", clean(warning)), style(theme.warning, color), inner_width);
        }
        Self { title, lines, border: muted }
    }

    pub(crate) fn height(&self) -> u16 {
        u16::try_from(self.lines.len()).unwrap_or(u16::MAX - 2).saturating_add(2)
    }
}

impl Widget for &AccountPane {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).border_style(self.border).title(self.title.clone());
        let inner = block.inner(area);
        block.render(area, buffer);
        Paragraph::new(self.lines.clone()).render(inner, buffer);
    }
}

fn append_metric(lines: &mut Vec<Line<'static>>, metric: &UsageMetric, width: u16, theme: Theme, color: bool) {
    let value = metric.used_percent.filter(|value| value.is_finite()).map_or_else(
        || match (metric.used, metric.limit) {
            (Some(used), Some(limit)) => format!("{used:.2} / {limit:.2} {}", metric.unit.as_deref().unwrap_or("")),
            (Some(used), None) => format!("{used:.2} {}", metric.unit.as_deref().unwrap_or("")),
            (None, Some(limit)) => format!("limit {limit:.2} {}", metric.unit.as_deref().unwrap_or("")),
            (None, None) => "unknown".to_owned(),
        },
        |percent| format!("{percent:.1}%"),
    );
    push_wrapped(lines, format!("{}: {value}", clean(&metric.name)), style(theme.meter, color), width);
    if let Some(percent) = metric.used_percent.filter(|value| value.is_finite()) {
        lines.push(gauge(width, percent, style(theme.bar_primary, color), style(theme.bar_background, color)));
        if metric.used.is_some() && metric.unit.as_deref().is_some_and(|unit| unit != "percent" && unit != "%") {
            let amount = metric.used.unwrap_or(0.0);
            let text = metric.limit.map_or_else(
                || format!("  used {amount:.2} {}", metric.unit.as_deref().unwrap_or("")),
                |limit| format!("  {amount:.2} / {limit:.2} {}", metric.unit.as_deref().unwrap_or("")),
            );
            push_wrapped(lines, text, style(theme.window, color), width);
        }
    }
    if let Some(resets) = metric.resets_at {
        push_wrapped(lines, format!("  resets {}", countdown(resets)), style(theme.reset, color), width);
    }
}

fn countdown(timestamp: i64) -> String {
    let Some(then) = u64::try_from(timestamp).ok().map(Millis::new) else { return "unknown".to_owned(); };
    now_millis().map_or_else(|| "unknown".to_owned(), |now| {
        let seconds = now.seconds_until(then);
        if seconds == 0 { "now".to_owned() } else { format!("in {}", human_duration(seconds)) }
    })
}

fn push_wrapped(lines: &mut Vec<Line<'static>>, text: String, style: Style, width: u16) {
    let mut part = String::new();
    let mut columns = 0;
    for character in text.chars() {
        let advance = character.width().unwrap_or(0);
        if columns + advance > usize::from(width) && !part.is_empty() {
            lines.push(Line::from(Span::styled(std::mem::take(&mut part), style)));
            columns = 0;
        }
        part.push(character);
        columns += advance;
    }
    lines.push(Line::from(Span::styled(part, style)));
}

#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "finite usage is clamped to the pane width before conversion")]
fn gauge(width: u16, percent: f64, foreground: Style, background: Style) -> Line<'static> {
    let cells = percent.clamp(0.0, 100.0) * f64::from(width) / 100.0;
    let whole = cells.floor() as usize;
    let remainder = cells.fract();
    let partial = usize::from(whole < usize::from(width) && remainder > 0.0);
    let mut spans = vec![Span::styled("█".repeat(whole), foreground)];
    if partial != 0 {
        spans.push(Span::styled(if remainder >= 0.5 { "▓" } else { "▒" }, foreground));
    }
    spans.push(Span::styled("░".repeat(usize::from(width) - whole - partial), background));
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

pub(crate) fn layout_panes(snapshots: &[AccountSnapshot], indices: &[usize], width: u16, theme: Theme, color: bool) -> Vec<PlacedPane> {
    let width = width.max(1);
    let columns = usize::from((width / MIN_PANE_WIDTH).max(1));
    let column_count = u16::try_from(columns).unwrap_or(1);
    let base_width = width / column_count;
    let extra = width % column_count;
    let mut result = Vec::with_capacity(indices.len());
    let mut y = 0;
    for row in indices.chunks(columns) {
        let mut x = 0;
        let mut row_height = 0;
        for (column, &index) in row.iter().enumerate() {
            let pane_width = base_width + u16::from(column < usize::from(extra));
            let pane = AccountPane::new(&snapshots[index], pane_width, theme, color);
            let height = pane.height();
            row_height = row_height.max(height);
            result.push(PlacedPane { index, x, y, width: pane_width, height, pane });
            x += pane_width;
        }
        y += usize::from(row_height);
    }
    result
}

pub(crate) fn report_width(explicit: Option<u16>) -> u16 {
    explicit.or_else(|| crossterm::terminal::size().ok().map(|(width, _)| width)).unwrap_or(100).max(1)
}

pub(crate) fn color_enabled() -> bool {
    anstream::AutoStream::auto(std::io::stdout()).current_choice() != anstream::ColorChoice::Never
}

pub(crate) fn render_report(snapshots: &[AccountSnapshot], width: u16, theme: Theme) -> io::Result<()> {
    let color = color_enabled();
    let indices: Vec<_> = (0..snapshots.len()).collect();
    let panes = layout_panes(snapshots, &indices, width, theme, color);
    let mut output = anstream::AutoStream::auto(io::stdout().lock());
    if panes.is_empty() { return writeln!(output, "No accounts found. Check configured credential sources and exclusions."); }
    // Stream one grid row at a time: the complete report has no terminal-height
    // limit and never enters alternate-screen/cursor-drawing mode.
    let mut cursor = 0;
    while cursor < panes.len() {
        let y = panes[cursor].y;
        let end = cursor + panes[cursor..].iter().take_while(|pane| pane.y == y).count();
        let row = &panes[cursor..end];
        let height = row.iter().map(|pane| pane.height).max().unwrap_or(1);
        let area = Rect::new(0, 0, width.max(1), height);
        let mut buffer = Buffer::empty(area);
        for pane in row { (&pane.pane).render(Rect::new(pane.x, 0, pane.width, pane.height), &mut buffer); }
        write_buffer(&buffer, area, &mut output, color)?;
        cursor = end;
    }
    output.flush()
}

pub(crate) fn write_buffer<W: Write>(buffer: &Buffer, area: Rect, mut output: W, color: bool) -> io::Result<()> {
    for y in area.y..area.bottom() {
        let mut x = area.x;
        let mut current = Style::default();
        while x < area.right() {
            let cell = &buffer[(x, y)];
            let cell_style = cell.style();
            if color && cell_style != current {
                write!(output, "{}{}", anstyle::Reset, ansi_style(cell_style).render())?;
                current = cell_style;
            }
            write!(output, "{}", cell.symbol())?;
            let advance = u16::try_from(cell.symbol().width()).unwrap_or(1).max(1);
            x = x.saturating_add(advance);
        }
        if color { write!(output, "{}", anstyle::Reset)?; }
        writeln!(output)?;
    }
    Ok(())
}

pub(crate) fn style(value: anstyle::Style, enabled: bool) -> Style {
    if !enabled { return Style::default(); }
    let mut result = Style::default();
    if let Some(color) = value.get_fg_color() { result = result.fg(ratatui_color(color)); }
    if let Some(color) = value.get_bg_color() { result = result.bg(ratatui_color(color)); }
    if value.get_effects().contains(Effects::BOLD) { result = result.add_modifier(Modifier::BOLD); }
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
    if let Some(color) = value.fg.and_then(ansi_color) { result = result.fg_color(Some(color)); }
    if let Some(color) = value.bg.and_then(ansi_color) { result = result.bg_color(Some(color)); }
    if value.add_modifier.contains(Modifier::BOLD) { result = result.effects(Effects::BOLD); }
    result
}

fn ansi_color(color: Color) -> Option<anstyle::Color> {
    Some(match color {
        Color::Reset => return None,
        Color::Rgb(red, green, blue) => anstyle::Color::Rgb(anstyle::RgbColor(red, green, blue)),
        Color::Indexed(index) => anstyle::Color::Ansi256(anstyle::Ansi256Color(index)),
        other => anstyle::Color::Ansi(match other {
            Color::Black => AnsiColor::Black, Color::Red => AnsiColor::Red,
            Color::Green => AnsiColor::Green, Color::Yellow => AnsiColor::Yellow,
            Color::Blue => AnsiColor::Blue, Color::Magenta => AnsiColor::Magenta,
            Color::Cyan => AnsiColor::Cyan, Color::Gray => AnsiColor::White,
            Color::DarkGray => AnsiColor::BrightBlack, Color::LightRed => AnsiColor::BrightRed,
            Color::LightGreen => AnsiColor::BrightGreen, Color::LightYellow => AnsiColor::BrightYellow,
            Color::LightBlue => AnsiColor::BrightBlue, Color::LightMagenta => AnsiColor::BrightMagenta,
            Color::LightCyan => AnsiColor::BrightCyan, Color::White => AnsiColor::BrightWhite,
            _ => return None,
        }),
    })
}

fn clean(value: &str) -> String {
    value.chars().filter(|character| !character.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::{gauge, write_buffer};
    use ratatui::{buffer::Buffer, layout::Rect, style::Style, widgets::{Paragraph, Widget}};
    use eyre::Result;

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
        for (percent, expected) in [(0.0, "░░░░░░░░░░"), (42.0, "████▒░░░░░"), (46.0, "████▓░░░░░"), (100.0, "██████████")] {
            let line = gauge(10, percent, Style::default(), Style::default());
            assert_eq!(line.to_string(), expected);
        }
    }
}
