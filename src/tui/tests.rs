use super::{Action, DashboardView, Mode, PickerKind};
use crate::dashboard::{
    AccountInfo, AccountSnapshot, AccountUsage, AllowanceWindow, CredentialKind, CreditAmount,
    CreditCount, CreditUnit, Provider, SubscriptionLimits, UsageAllowance, UsageCredits,
};
use crate::theme::BUILTIN_THEMES;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use eyre::Result;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

fn snapshot(index: usize, provider: Provider) -> AccountSnapshot {
    AccountSnapshot {
        account: AccountInfo {
            id: format!("account-{index}"),
            provider,
            label: format!("User {index}"),
            email: Some(format!("user{index}@example.test")),
            name: Some(format!("Person {index}")),
            account_id: Some(format!("workspace-{index}")),
            sources: vec!["fixture".to_owned()],
            credential_kind: CredentialKind::OAuth,
            masked_key: None,
        },
        usage: Some(AccountUsage {
            limits: SubscriptionLimits {
                allowances: vec![UsageAllowance {
                    title: "Usage".to_owned(),
                    window: Some(AllowanceWindow::Weekly),
                    credits: UsageCredits {
                        count: CreditCount::Full {
                            allocated: CreditAmount::Integer(100),
                            consumed: CreditAmount::Integer(42),
                        },
                        unit: CreditUnit::Percentage,
                    },
                    resets_at: None,
                }],
                ..SubscriptionLimits::default()
            },
            plan: Some("Fixture".to_owned()),
        }),
        error: None,
        fetched_at: Some(100),
        warnings: Vec::new(),
    }
}

fn view() -> DashboardView {
    DashboardView {
        snapshots: (0..12)
            .map(|index| {
                snapshot(
                    index,
                    if index % 2 == 0 {
                        Provider::Codex
                    } else {
                        Provider::Claude
                    },
                )
            })
            .collect(),
        filter: String::new(),
        provider: None,
        account: None,
        theme: BUILTIN_THEMES[0],
        color: false,
        scroll: 0,
        mode: Mode::Browse,
        status: String::new(),
    }
}

fn key(view: &mut DashboardView, code: KeyCode) -> Action {
    view.key(
        KeyEvent::new(code, KeyModifiers::NONE),
        Rect::new(0, 0, 90, 20),
    )
}

#[test]
fn live_filter_accepts_q_and_provider_all_clears_prior_query() {
    let mut view = view();
    key(&mut view, KeyCode::Char('/'));
    key(&mut view, KeyCode::Char('q'));
    assert_eq!(view.filter, "q");
    assert!(view.indices().is_empty());
    key(&mut view, KeyCode::Backspace);
    for character in "user1@".chars() {
        key(&mut view, KeyCode::Char(character));
    }
    assert_eq!(view.indices(), [1]);
    key(&mut view, KeyCode::Enter);
    key(&mut view, KeyCode::Char('p'));
    key(&mut view, KeyCode::Down);
    key(&mut view, KeyCode::Enter);
    assert!(view.filter.is_empty());
    assert_eq!(view.indices(), [1, 3, 5, 7, 9, 11]);
    key(&mut view, KeyCode::Char('p'));
    key(&mut view, KeyCode::Enter);
    assert_eq!(view.indices(), (0..12).collect::<Vec<_>>());
    assert!(matches!(key(&mut view, KeyCode::Char('q')), Action::Quit));
}

#[test]
fn account_picker_selects_exact_provider_account_and_all_restores_set() {
    let mut view = view();
    key(&mut view, KeyCode::Char('a'));
    key(&mut view, KeyCode::End);
    key(&mut view, KeyCode::Enter);
    assert_eq!(view.indices(), [11]);
    assert_eq!(view.account.as_deref(), Some("account-11"));
    key(&mut view, KeyCode::Char('a'));
    key(&mut view, KeyCode::Enter);
    assert_eq!(view.indices().len(), 12);
}

#[test]
fn refresh_uses_screen_intersection_and_merge_preserves_unseen_state() {
    let mut view = view();
    let initial = view.visible_ids(Rect::new(0, 0, 90, 20));
    assert!(initial.contains(&"account-0".to_owned()));
    assert!(!initial.contains(&"account-11".to_owned()));
    key(&mut view, KeyCode::End);
    let Action::Refresh(last) = key(&mut view, KeyCode::Char('r')) else {
        panic!("refresh action missing");
    };
    assert!(last.contains(&"account-11".to_owned()));
    assert!(!last.contains(&"account-0".to_owned()));
    let previous = view.snapshots[0].fetched_at;
    let mut updated = view.snapshots[11].clone();
    updated.fetched_at = Some(200);
    updated.error = Some("isolated failure".to_owned());
    view.merge(vec![updated]);
    assert_eq!(view.snapshots[0].fetched_at, previous);
    assert_eq!(view.snapshots[11].fetched_at, Some(200));
    assert_eq!(
        view.snapshots[11].error.as_deref(),
        Some("isolated failure")
    );
    view.scroll = 0;
    let wide = view.visible_ids(Rect::new(0, 0, 140, 20));
    let narrow = view.visible_ids(Rect::new(0, 0, 40, 20));
    assert!(wide.len() > narrow.len());
}

#[test]
fn tiny_and_resized_surfaces_stay_inside_the_buffer() -> Result<()> {
    let mut view = view();
    for (width, height) in [(0, 0), (1, 1), (2, 2), (3, 3), (20, 4), (44, 9), (140, 30)] {
        let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| view.draw(frame))?;
        assert_eq!(
            terminal.backend().buffer().area,
            Rect::new(0, 0, width, height)
        );
    }
    Ok(())
}

#[test]
fn picker_search_matches_names_email_provider_and_navigates_filtered_rows() {
    let mut view = view();
    key(&mut view, KeyCode::Char('a'));
    for character in "PERSON 1".chars() {
        key(&mut view, KeyCode::Char(character));
    }
    key(&mut view, KeyCode::Down);
    key(&mut view, KeyCode::Enter);
    assert_eq!(view.indices(), [10]);
    key(&mut view, KeyCode::Char('a'));
    for character in "claude user11@".chars() {
        key(&mut view, KeyCode::Char(character));
    }
    key(&mut view, KeyCode::Enter);
    assert_eq!(view.indices(), [11]);
    key(&mut view, KeyCode::Char('a'));
    key(&mut view, KeyCode::Char('q'));
    assert!(matches!(&view.mode, Mode::Picker { query, .. } if query == "q"));
    key(&mut view, KeyCode::Home);
    key(&mut view, KeyCode::Enter);
    assert_eq!(view.indices(), (0..12).collect::<Vec<_>>());
    key(&mut view, KeyCode::Char('p'));
    for character in "CODEX".chars() {
        key(&mut view, KeyCode::Char(character));
    }
    key(&mut view, KeyCode::Enter);
    assert_eq!(view.indices(), [0, 2, 4, 6, 8, 10]);
    key(&mut view, KeyCode::Char('p'));
    key(&mut view, KeyCode::Char('q'));
    assert!(matches!(key(&mut view, KeyCode::Esc), Action::Quit));
    assert!(matches!(
        view.key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Rect::new(0, 0, 100, 24)
        ),
        Action::Quit
    ));
}

fn screen(view: &mut DashboardView, width: u16, height: u16) -> Result<String> {
    let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height))?;
    terminal.draw(|frame| view.draw(frame))?;
    Ok(terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect())
}

#[test]
fn opaque_identity_is_hidden_in_overview_and_picker_but_visible_in_single_details() -> Result<()> {
    let mut view = view();
    let id = "a72ac094-56d6-4e0c-bd20-f4ce7cb94132";
    view.snapshots[0].account.account_id = Some(id.to_owned());
    view.snapshots[0].account.label = id.to_owned();
    assert!(!screen(&mut view, 100, 24)?.contains(id));
    assert!(view
        .choices(PickerKind::Account, "")
        .iter()
        .all(|choice| !choice.label.contains(id)));
    key(&mut view, KeyCode::Char('a'));
    for character in "user0@".chars() {
        key(&mut view, KeyCode::Char(character));
    }
    key(&mut view, KeyCode::Enter);
    assert!(screen(&mut view, 100, 24)?.contains(id));
    key(&mut view, KeyCode::Char('a'));
    key(&mut view, KeyCode::Enter);
    assert!(!screen(&mut view, 100, 24)?.contains(id));
    key(&mut view, KeyCode::Char('/'));
    for character in "user0@".chars() {
        key(&mut view, KeyCode::Char(character));
    }
    assert!(screen(&mut view, 100, 24)?.contains(id));
    Ok(())
}

#[test]
fn pane_padding_is_blank_and_visible_refresh_uses_the_padded_bottom_edge() -> Result<()> {
    let mut view = view();
    let panes = view.panes(100);
    let bottom = panes[0].height;
    let top = DashboardView::content_area(Rect::new(0, 0, 100, 24)).y;
    let mut terminal = ratatui::Terminal::new(TestBackend::new(100, 24))?;
    terminal.draw(|frame| view.draw(frame))?;
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(2, top + 1)].symbol(), " ");
    assert_eq!(buffer[(1, top + 2)].symbol(), " ");
    assert_eq!(buffer[(2, top + 2)].symbol(), "S");
    view.scroll = usize::from(bottom - 1);
    assert!(view
        .visible_ids(Rect::new(0, 0, 100, 8))
        .contains(&"account-0".to_owned()));
    view.scroll = usize::from(bottom);
    assert!(!view
        .visible_ids(Rect::new(0, 0, 100, 8))
        .contains(&"account-0".to_owned()));
    for (width, height) in [(5, 4), (44, 12), (100, 24)] {
        key(&mut view, KeyCode::Char('a'));
        let _ = screen(&mut view, width, height)?;
        key(&mut view, KeyCode::Enter);
    }
    Ok(())
}
