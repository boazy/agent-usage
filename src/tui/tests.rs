use super::{Action, DashboardView, Mode};
use crate::dashboard::{
    AccountInfo, AccountSnapshot, AccountUsage, CredentialKind, Provider, UsageMetric,
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
            account_id: Some(format!("workspace-{index}")),
            sources: vec!["fixture".to_owned()],
            credential_kind: CredentialKind::OAuth,
            masked_key: None,
        },
        usage: Some(AccountUsage {
            windows: vec![UsageMetric {
                name: "Weekly".to_owned(),
                used_percent: Some(42.0),
                used: Some(42.0),
                limit: Some(100.0),
                unit: Some("percent".to_owned()),
                resets_at: None,
            }],
            credits: Vec::new(),
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
