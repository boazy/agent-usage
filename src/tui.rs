use crate::dashboard::{AccountSnapshot, Dashboard};
use crate::render::{
    account_label, clean, color_enabled, layout_panes, short_account_id, style, PlacedPane,
    MIN_PANE_WIDTH,
};
use crate::theme::{Theme, BUILTIN_THEMES};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{execute, terminal};
use eyre::{eyre, Result, WrapErr};
use futures_util::StreamExt;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Widget,
};
use std::collections::BTreeSet;
use std::io::{self, IsTerminal};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
enum PickerKind {
    Provider,
    Account,
    Theme,
}

enum Mode {
    Browse,
    Filter,
    Picker {
        kind: PickerKind,
        selected: usize,
        offset: usize,
        query: String,
    },
}

struct Choice {
    label: String,
    value: Option<String>,
    search: String,
}

pub(crate) struct DashboardView {
    snapshots: Vec<AccountSnapshot>,
    completed: Vec<bool>,
    pending: BTreeSet<usize>,
    refresh_completed: usize,
    refresh_failures: usize,
    loading_scroll: usize,
    filter: String,
    provider: Option<String>,
    account: Option<String>,
    theme: Theme,
    color: bool,
    scroll: usize,
    mode: Mode,
    status: String,
}

enum Action {
    None,
    Quit,
    Refresh(Vec<String>),
}

impl DashboardView {
    fn new(dashboard: &Dashboard, theme: &Theme) -> Self {
        let snapshots = dashboard
            .accounts()
            .iter()
            .map(|account| AccountSnapshot {
                account: account.clone(),
                usage: None,
                error: None,
                fetched_at: None,
                warnings: Vec::new(),
            })
            .collect();
        Self {
            snapshots,
            completed: vec![false; dashboard.accounts().len()],
            pending: BTreeSet::new(),
            refresh_completed: 0,
            refresh_failures: 0,
            loading_scroll: 0,
            filter: String::new(),
            provider: None,
            account: None,
            theme: *theme,
            color: color_enabled(),
            scroll: 0,
            mode: Mode::Browse,
            status: "Loading account usage…".to_owned(),
        }
    }

    fn indices(&self) -> Vec<usize> {
        let query = self.filter.to_lowercase();
        self.snapshots
            .iter()
            .enumerate()
            .filter(|(_, snapshot)| {
                let account = &snapshot.account;
                self.provider
                    .as_ref()
                    .is_none_or(|provider| provider == account.provider.id())
                    && self.account.as_ref().is_none_or(|id| id == &account.id)
                    && (query.is_empty() || matches_query(&account_search(account), &query))
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn panes(&self, width: u16, reserved: Option<Rect>) -> Vec<PlacedPane> {
        let matching = self.indices();
        let details = matching.len() == 1
            && (self.account.is_some()
                || !self.filter.trim().is_empty()
                || self.provider.is_some());
        let ready: Vec<_> = matching
            .into_iter()
            .filter(|&index| self.completed[index])
            .collect();
        layout_panes(
            &self.snapshots,
            &ready,
            width,
            &self.theme,
            self.color,
            details,
            reserved,
        )
    }

    fn loading_providers(&self) -> Vec<String> {
        self.pending
            .iter()
            .map(|&index| clean(&self.snapshots[index].account.provider.to_string()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn loading_area(content: Rect, providers: &[String]) -> Option<Rect> {
        if providers.is_empty() || content.width == 0 || content.height == 0 {
            return None;
        }
        let columns = (content.width / MIN_PANE_WIDTH).max(1);
        let width = content.width / columns;
        // Keep a useful data viewport even on a single narrow column. Long provider
        // lists page inside this footprint instead of consuming the entire screen.
        let height = u16::try_from(providers.len())
            .unwrap_or(u16::MAX)
            .saturating_add(4)
            .min((content.height / 2).max(1));
        Some(Rect::new(
            content.width - width,
            content.height - height,
            width,
            height,
        ))
    }

    fn content_area(area: Rect) -> Rect {
        Rect::new(
            area.x,
            area.y.saturating_add(area.height.min(2)),
            area.width,
            area.height.saturating_sub(4),
        )
    }

    fn clamp_scroll(&mut self, panes: &[PlacedPane], height: u16, reserved: Option<Rect>) {
        let maximum = panes
            .iter()
            .map(|pane| {
                (pane.y + usize::from(pane.height))
                    .saturating_sub(usize::from(pane_viewport_height(pane, height, reserved)))
            })
            .max()
            .unwrap_or(0);
        self.scroll = self.scroll.min(maximum);
    }

    fn visible_ids(&mut self, area: Rect) -> Vec<String> {
        let content = Self::content_area(area);
        let reserved = Self::loading_area(content, &self.loading_providers());
        let panes = self.panes(content.width, reserved);
        self.clamp_scroll(&panes, content.height, reserved);
        panes
            .iter()
            .filter(|pane| {
                intersects(
                    pane,
                    self.scroll,
                    pane_viewport_height(pane, content.height, reserved),
                )
            })
            .map(|pane| self.snapshots[pane.index].account.id.clone())
            .collect()
    }

    fn complete(&mut self, index: usize, update: AccountSnapshot) {
        if !self.pending.remove(&index) {
            return;
        }
        self.refresh_completed += 1;
        self.refresh_failures += usize::from(update.error.is_some());
        self.snapshots[index] = update;
        self.completed[index] = true;
        self.status = format!(
            "Updated {} account(s); {} unavailable; {} pending",
            self.refresh_completed,
            self.refresh_failures,
            self.pending.len()
        );
    }

    fn fail_pending(&mut self, message: &str) {
        while let Some(&index) = self.pending.first() {
            let mut snapshot = self.snapshots[index].clone();
            snapshot.usage = None;
            snapshot.error = Some(message.to_owned());
            self.complete(index, snapshot);
        }
    }

    fn choices(&self, kind: PickerKind, query: &str) -> Vec<Choice> {
        let mut choices = Vec::new();
        match kind {
            PickerKind::Provider => {
                choices.push(Choice {
                    label: "All providers · clear selection and filter".to_owned(),
                    value: None,
                    search: String::new(),
                });
                let providers: BTreeSet<_> = self
                    .snapshots
                    .iter()
                    .map(|snapshot| snapshot.account.provider.id().to_owned())
                    .collect();
                choices.extend(providers.into_iter().map(|provider| Choice {
                    label: crate::dashboard::Provider::from_id(&provider).to_string(),
                    search: provider.to_lowercase(),
                    value: Some(provider),
                }));
            }
            PickerKind::Account => {
                choices.push(Choice {
                    label: "All accounts · clear selection and filter".to_owned(),
                    value: None,
                    search: String::new(),
                });
                choices.extend(
                    self.snapshots
                        .iter()
                        .filter(|snapshot| {
                            self.provider
                                .as_ref()
                                .is_none_or(|provider| provider == snapshot.account.provider.id())
                        })
                        .map(|snapshot| {
                            let account = &snapshot.account;
                            let label = account_label(account);
                            let email = account
                                .email
                                .as_deref()
                                .filter(|email| !label.contains(email))
                                .map_or_else(String::new, |email| format!(" · {email}"));
                            Choice {
                                label: format!(
                                    "{} · {label}{email} · #{}",
                                    account.provider,
                                    short_account_id(account)
                                ),
                                value: Some(account.id.clone()),
                                search: account_search(account),
                            }
                        }),
                );
            }
            PickerKind::Theme => choices.extend(BUILTIN_THEMES.iter().map(|theme| Choice {
                label: theme.name.to_owned(),
                value: Some(theme.name.to_owned()),
                search: theme.name.to_owned(),
            })),
        }
        let query = query.to_lowercase();
        choices.retain(|choice| choice.value.is_none() || matches_query(&choice.search, &query));
        choices
    }

    fn apply_background(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        frame
            .buffer_mut()
            .set_style(area, style(self.theme.background, self.color));
    }

    fn draw(&mut self, frame: &mut ratatui::Frame<'_>) {
        let area = frame.area();
        if area.width == 0 || area.height == 0 {
            return;
        }
        let providers = self.loading_providers();
        let content = Self::content_area(area);
        let reserved = Self::loading_area(content, &providers);
        let panes = self.panes(content.width, reserved);
        self.clamp_scroll(&panes, content.height, reserved);
        let heading = format!(
            "Agent usage · {} account(s) · theme {}",
            panes.len(),
            self.theme.name
        );
        frame.render_widget(
            Paragraph::new(heading).style(style(self.theme.title, self.color)),
            Rect::new(area.x, area.y, area.width, 1),
        );
        if content.height == 0 && !providers.is_empty() {
            self.draw_loading(frame, area, &providers);
            self.apply_background(frame, area);
            return;
        }
        if area.height == 1 {
            self.apply_background(frame, area);
            return;
        }
        let filters = format!(
            "/ {}  | provider: {} | account: {}",
            self.filter,
            self.provider.as_deref().unwrap_or("All"),
            self.account.as_ref().map_or("All", |_| "selected")
        );
        frame.render_widget(
            Paragraph::new(filters).style(style(self.theme.window, self.color)),
            Rect::new(area.x, area.y.saturating_add(1), area.width, 1),
        );
        if area.height < 4 {
            self.apply_background(frame, area);
            return;
        }
        if panes.is_empty() && providers.is_empty() {
            frame.render_widget(
                Paragraph::new("No matching accounts. Clear / filter or select All with p / a.")
                    .style(style(self.theme.label, self.color)),
                content,
            );
        }
        for pane in &panes {
            let height = pane_viewport_height(pane, content.height, reserved);
            if !intersects(pane, self.scroll, height) {
                continue;
            }
            let mut buffer = Buffer::empty(Rect::new(0, 0, pane.width, pane.height));
            (&pane.pane).render(buffer.area, &mut buffer);
            let start = self.scroll.saturating_sub(pane.y);
            let end = usize::from(pane.height).min(self.scroll + usize::from(height) - pane.y);
            for row in start..end {
                let y = content.y + u16::try_from(pane.y + row - self.scroll).unwrap_or(0);
                for x in 0..pane.width {
                    frame.buffer_mut()[(content.x + pane.x + x, y)] =
                        buffer[(x, u16::try_from(row).unwrap_or(0))].clone();
                }
            }
        }
        if let Some(reserved) = reserved {
            self.draw_loading(
                frame,
                Rect::new(
                    content.x + reserved.x,
                    content.y + reserved.y,
                    reserved.width,
                    reserved.height,
                ),
                &providers,
            );
        }
        let footer_y = area.bottom().saturating_sub(2);
        frame.render_widget(
            Paragraph::new(self.status.as_str()).style(style(self.theme.label, self.color)),
            Rect::new(area.x, footer_y, area.width, 1),
        );
        let help = match self.mode {
            Mode::Filter => "Type filter (q is text) · Enter done · Esc/Ctrl-C quit",
            Mode::Picker { .. } => "Type to search (q is text) · arrows move · Enter select · Esc/Ctrl-C quit",
            Mode::Browse => "/ filter · p/a/t pick · arrows/PgUp/PgDn scroll · [/] loading · r visible refresh · q/Esc quit",
        };
        frame.render_widget(
            Paragraph::new(help).style(style(self.theme.window, self.color)),
            Rect::new(area.x, footer_y.saturating_add(1), area.width, 1),
        );
        self.draw_picker(frame, area);
        self.apply_background(frame, area);
    }

    fn draw_loading(&mut self, frame: &mut ratatui::Frame<'_>, area: Rect, providers: &[String]) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let block = Block::default()
            .borders(if area.height >= 3 {
                Borders::ALL
            } else {
                Borders::NONE
            })
            .padding(if area.height >= 5 {
                Padding::uniform(1)
            } else {
                Padding::ZERO
            })
            .border_style(style(self.theme.active_border, self.color))
            .title_style(style(self.theme.title, self.color));
        let inner = block.inner(area);
        let rows = usize::from(inner.height);
        self.loading_scroll = self
            .loading_scroll
            .min(providers.len().saturating_sub(rows));
        let title = if providers.len() > rows {
            format!(
                " Loading {}–{}/{} [ ] ",
                self.loading_scroll + 1,
                (self.loading_scroll + rows).min(providers.len()),
                providers.len()
            )
        } else {
            " Loading ".to_owned()
        };
        frame.render_widget(block.title(title), area);
        let items: Vec<_> = providers
            .iter()
            .skip(self.loading_scroll)
            .take(rows)
            .map(|provider| {
                if area.height < 3 {
                    ListItem::new(format!("Loading: {provider}"))
                } else {
                    ListItem::new(provider.as_str())
                }
            })
            .collect();
        frame.render_widget(
            List::new(items).style(style(self.theme.unknown, self.color)),
            inner,
        );
    }

    fn draw_picker(&mut self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let Mode::Picker {
            kind,
            selected,
            offset,
            query,
        } = &self.mode
        else {
            return;
        };
        let choices = self.choices(*kind, query);
        let width = area.width.saturating_sub(4).clamp(1, 90);
        let height = area
            .height
            .saturating_sub(4)
            .min(
                u16::try_from(choices.len())
                    .unwrap_or(u16::MAX)
                    .saturating_add(5),
            )
            .max(1);
        let popup = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let title = match kind {
            PickerKind::Provider => " Providers ",
            PickerKind::Account => " Accounts ",
            PickerKind::Theme => " Themes ",
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1))
            .border_style(style(self.theme.active_border, self.color))
            .title(ratatui::text::Span::styled(
                title,
                style(self.theme.title, self.color),
            ));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        if inner.height == 0 || inner.width == 0 {
            return;
        }
        frame.render_widget(
            Paragraph::new(format!("Search: {query}")).style(style(self.theme.account, self.color)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        if inner.height < 2 {
            return;
        }
        let items: Vec<_> = choices
            .iter()
            .map(|choice| ListItem::new(choice.label.as_str()))
            .collect();
        let list = List::new(items)
            .style(style(self.theme.label, self.color))
            .highlight_symbol("> ")
            .highlight_style(style(self.theme.selection, self.color));
        let mut state = ListState::default()
            .with_selected(Some(*selected))
            .with_offset(*offset);
        let list_area = Rect::new(
            inner.x,
            inner.y.saturating_add(1),
            inner.width,
            inner.height.saturating_sub(2),
        );
        frame.render_stateful_widget(list, list_area, &mut state);
        if choices.is_empty() {
            frame.render_widget(
                Paragraph::new("No matching themes").style(style(self.theme.unknown, self.color)),
                list_area,
            );
        }
        frame.render_widget(
            Paragraph::new("Type to search · q is text · Enter select")
                .style(style(self.theme.label, self.color)),
            Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
        );
        if let Mode::Picker { offset, .. } = &mut self.mode {
            *offset = state.offset();
        }
    }

    fn key(&mut self, key: KeyEvent, area: Rect) -> Action {
        if key.code == KeyCode::Esc
            || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return Action::Quit;
        }
        if matches!(self.mode, Mode::Filter) {
            match key.code {
                KeyCode::Enter => self.mode = Mode::Browse,
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.scroll = 0;
                }
                KeyCode::Char(character)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.filter.push(character);
                    self.scroll = 0;
                }
                _ => {}
            }
            return Action::None;
        }
        if let Mode::Picker {
            kind,
            selected,
            ref query,
            ..
        } = self.mode
        {
            let choices = self.choices(kind, query);
            let page = usize::from(area.height.saturating_sub(9).max(1));
            let next = match key.code {
                KeyCode::Down | KeyCode::Right => selected.saturating_add(1),
                KeyCode::Up | KeyCode::Left => selected.saturating_sub(1),
                KeyCode::PageDown => selected.saturating_add(page),
                KeyCode::PageUp => selected.saturating_sub(page),
                KeyCode::Home => 0,
                KeyCode::End => choices.len().saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(choice) = choices.get(selected) {
                        self.apply_choice(kind, choice);
                    }
                    self.mode = Mode::Browse;
                    self.scroll = 0;
                    return Action::None;
                }
                KeyCode::Backspace => {
                    if let Mode::Picker { query, .. } = &mut self.mode {
                        query.pop();
                    }
                    self.reset_picker_selection(kind);
                    return Action::None;
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Mode::Picker { query, .. } = &mut self.mode {
                        query.clear();
                    }
                    self.reset_picker_selection(kind);
                    return Action::None;
                }
                KeyCode::Char(character)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    if let Mode::Picker { query, .. } = &mut self.mode {
                        query.push(character);
                    }
                    self.reset_picker_selection(kind);
                    return Action::None;
                }
                _ => selected,
            }
            .min(choices.len().saturating_sub(1));
            if let Mode::Picker { selected, .. } = &mut self.mode {
                *selected = next;
            }
            return Action::None;
        }
        self.browse_key(key, area)
    }

    fn browse_key(&mut self, key: KeyEvent, area: Rect) -> Action {
        if key.code == KeyCode::Char('q') {
            return Action::Quit;
        }
        let page = usize::from(Self::content_area(area).height.max(1));
        match key.code {
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Char('p') => {
                self.mode = Mode::Picker {
                    kind: PickerKind::Provider,
                    selected: 0,
                    offset: 0,
                    query: String::new(),
                }
            }
            KeyCode::Char('a') => {
                self.mode = Mode::Picker {
                    kind: PickerKind::Account,
                    selected: 0,
                    offset: 0,
                    query: String::new(),
                }
            }
            KeyCode::Char('t') => {
                self.mode = Mode::Picker {
                    kind: PickerKind::Theme,
                    selected: 0,
                    offset: 0,
                    query: String::new(),
                }
            }
            KeyCode::Down | KeyCode::Right => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Left => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(page),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(page),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = usize::MAX,
            KeyCode::Char(']') => self.loading_scroll = self.loading_scroll.saturating_add(1),
            KeyCode::Char('[') => self.loading_scroll = self.loading_scroll.saturating_sub(1),
            KeyCode::Char('r') => return Action::Refresh(self.visible_ids(area)),
            _ => {}
        }
        Action::None
    }

    fn reset_picker_selection(&mut self, kind: PickerKind) {
        let Mode::Picker { query, .. } = &self.mode else {
            return;
        };
        let choices = self.choices(kind, query);
        let first_match = usize::from(
            !query.is_empty()
                && choices.first().is_some_and(|choice| choice.value.is_none())
                && choices.len() > 1,
        );
        if let Mode::Picker {
            selected, offset, ..
        } = &mut self.mode
        {
            *selected = first_match;
            *offset = 0;
        }
    }

    fn apply_choice(&mut self, kind: PickerKind, choice: &Choice) {
        match kind {
            PickerKind::Provider => {
                self.provider.clone_from(&choice.value);
                self.account = None;
                self.filter.clear();
            }
            PickerKind::Account => {
                self.account.clone_from(&choice.value);
                self.filter.clear();
            }
            PickerKind::Theme => {
                if let Some(theme) = choice
                    .value
                    .as_deref()
                    .and_then(crate::theme::theme_by_name)
                {
                    self.theme = theme;
                }
            }
        }
    }
}

fn account_search(account: &crate::dashboard::AccountInfo) -> String {
    format!(
        "{} {} {} {} {}",
        account_label(account),
        account.name.as_deref().unwrap_or(""),
        account.email.as_deref().unwrap_or(""),
        account.provider,
        account.provider.id()
    )
    .to_lowercase()
}

fn matches_query(text: &str, query: &str) -> bool {
    query.split_whitespace().all(|word| text.contains(word))
}

fn intersects(pane: &PlacedPane, scroll: usize, height: u16) -> bool {
    height != 0
        && pane.y < scroll.saturating_add(usize::from(height))
        && pane.y + usize::from(pane.height) > scroll
}

fn pane_viewport_height(pane: &PlacedPane, height: u16, reserved: Option<Rect>) -> u16 {
    if let Some(reserved) = reserved {
        if pane.x < reserved.right() && pane.x.saturating_add(pane.width) > reserved.x {
            return height.min(reserved.y);
        }
    }
    height
}

struct TerminalSession;

impl TerminalSession {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().wrap_err("failed to enable terminal raw mode")?;
        let session = Self;
        execute!(io::stdout(), terminal::EnterAlternateScreen, Hide)
            .wrap_err("failed to enter dashboard screen")?;
        Ok(session)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(io::stdout(), Show, terminal::LeaveAlternateScreen);
}

pub(crate) async fn run(dashboard: Arc<Dashboard>, theme: &Theme, refresh: Duration) -> Result<()> {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return Err(eyre!(
            "--tui requires an interactive terminal; use --report or --json for pipes"
        ));
    }
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous_hook(info);
    }));
    let session = TerminalSession::enter()?;
    let backend = ratatui::backend::CrosstermBackend::new(io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;
    let mut view = DashboardView::new(&dashboard, theme);
    let mut jobs = JoinSet::new();
    let (updates, mut completions) = mpsc::channel(32);
    let all_ids = dashboard
        .accounts()
        .iter()
        .map(|account| account.id.clone())
        .collect();
    start_refresh(&mut jobs, &dashboard, all_ids, &mut view, &updates);
    let mut events = EventStream::new();
    let mut redraw = tokio::time::interval(Duration::from_secs(1));
    redraw.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut automatic = tokio::time::interval_at(tokio::time::Instant::now() + refresh, refresh);
    automatic.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result: Result<()> = async {
    loop {
        terminal.draw(|frame| view.draw(frame))?;
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                    let size = terminal.size()?;
                    let area = Rect::new(0, 0, size.width, size.height);
                    match view.key(key, area) {
                        Action::Quit => break,
                        Action::Refresh(ids) => start_refresh(&mut jobs, &dashboard, ids, &mut view, &updates),
                        Action::None => {}
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error).wrap_err("terminal event stream failed"),
                None => break,
            },
            Some((index, snapshot)) = completions.recv() => view.complete(index, snapshot),
            result = jobs.join_next(), if !jobs.is_empty() => {
                drain_completions(&mut completions, &mut view);
                if result.is_some() {
                    view.fail_pending("Refresh worker failed; press r to retry");
                }
            },
            _ = automatic.tick() => {
                if jobs.is_empty() {
                    let size = terminal.size()?;
                    let ids = view.visible_ids(Rect::new(0, 0, size.width, size.height));
                    start_refresh(&mut jobs, &dashboard, ids, &mut view, &updates);
                }
            },
            _ = redraw.tick() => {}
        }
    }
        Ok(())
    }.await;
    dashboard.cancel_pending();
    // Stop queued work but preserve in-flight token rotation/persistence. Restore
    // the screen immediately; only the bounded active requests finish afterward.
    drop(session);
    // Closing the UI receiver releases a blocked progress send, not the fetch/save.
    completions.close();
    while jobs.join_next().await.is_some() {}
    drain_completions(&mut completions, &mut view);
    view.fail_pending("Refresh canceled during shutdown");
    result
}

fn start_refresh(
    jobs: &mut JoinSet<()>,
    dashboard: &Arc<Dashboard>,
    ids: Vec<String>,
    view: &mut DashboardView,
    updates: &mpsc::Sender<(usize, AccountSnapshot)>,
) {
    if !jobs.is_empty() {
        "Refresh already in progress".clone_into(&mut view.status);
        return;
    }
    let ids: BTreeSet<_> = ids.into_iter().collect();
    let selected: BTreeSet<_> = view
        .snapshots
        .iter()
        .enumerate()
        .filter(|(_, snapshot)| ids.contains(&snapshot.account.id))
        .map(|(index, _)| index)
        .collect();
    if selected.is_empty() {
        "No visible accounts to refresh".clone_into(&mut view.status);
        return;
    }
    let ids: Vec<_> = selected
        .iter()
        .map(|&index| view.snapshots[index].account.id.clone())
        .collect();
    view.pending = selected;
    view.refresh_completed = 0;
    view.refresh_failures = 0;
    view.loading_scroll = 0;
    view.status = format!("Refreshing {} account(s)…", ids.len());
    let dashboard = Arc::clone(dashboard);
    let updates = updates.clone();
    jobs.spawn(async move {
        let stream = dashboard.refresh_stream(&ids);
        futures_util::pin_mut!(stream);
        while let Some(completion) = stream.next().await {
            // A closed receiver must never cancel a credential rotation or save.
            let _ = updates.send(completion).await;
        }
    });
}

fn drain_completions(
    completions: &mut mpsc::Receiver<(usize, AccountSnapshot)>,
    view: &mut DashboardView,
) {
    while let Ok((index, snapshot)) = completions.try_recv() {
        view.complete(index, snapshot);
    }
}
