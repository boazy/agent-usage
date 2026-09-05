use super::{
    mask_key, masked_key_matches, CreditAmount, CreditCount, CreditUnit, Dashboard, Provider,
    UsageCredits,
};
use crate::config::{AccountExclusion, DashboardConfig, SourceConfig, SourceKind};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use eyre::Result;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn fixture(
    value: &Value,
    kind: SourceKind,
    name: &str,
) -> Result<(tempfile::NamedTempFile, SourceConfig)> {
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(serde_json::to_string(value)?.as_bytes())?;
    let source = SourceConfig {
        name: name.to_owned(),
        kind,
        path: file.path().to_path_buf(),
        enabled: true,
        optional: false,
    };
    Ok((file, source))
}

#[test]
fn aliases_and_dedup_keep_distinct_billing_providers_and_orgs() -> Result<()> {
    let oauth = json!({"type":"oauth","access":"synthetic-access-token","refresh":"synthetic-refresh","email":"same@example.test","accountId":"account","orgId":"personal"});
    let mut organization = oauth.clone();
    organization["orgId"] = json!("team");
    let (first_file, first) = fixture(
        &json!({"anthropic":[oauth.clone(), organization],"google-antigravity":oauth}),
        SourceKind::Omp,
        "first",
    )?;
    let (second_file, second) = fixture(
        &json!({"claude":{"type":"oauth","access":"other-token","refresh":"other-refresh","email":"same@example.test","accountId":"account","orgId":"personal"}}),
        SourceKind::Omp,
        "second",
    )?;
    let config = DashboardConfig {
        sources: vec![first, second],
        ..DashboardConfig::default()
    };
    let dashboard = Dashboard::load(&config)?;
    assert_eq!(dashboard.accounts.len(), 3);
    let personal = dashboard
        .accounts
        .iter()
        .find(|account| account.sources.len() == 2)
        .ok_or_else(|| eyre::eyre!("missing merged account"))?;
    assert_eq!(personal.provider, Provider::Claude);
    assert!(dashboard
        .accounts
        .iter()
        .any(|account| account.provider == Provider::Antigravity));
    drop((first_file, second_file));
    Ok(())
}

#[test]
fn exclusion_uses_all_duplicate_metadata_and_source_kind_before_discovery() -> Result<()> {
    let (first_file, first) = fixture(
        &json!({"anthropic":{"type":"oauth","access":"same-synthetic-access","refresh":"synthetic-refresh","accountId":"id"}}),
        SourceKind::Omp,
        "old",
    )?;
    let (second_file, second) = fixture(
        &json!({"anthropic":{"type":"oauth","access":"same-synthetic-access","refresh":"synthetic-refresh","accountId":"id","email":"excluded@example.test"}}),
        SourceKind::Omp,
        "new",
    )?;
    let mut config = DashboardConfig {
        sources: vec![first, second],
        ..DashboardConfig::default()
    };
    config.exclude.accounts.push(AccountExclusion {
        email: "excluded@example.test".to_owned(),
        provider: "codex".to_owned(),
    });
    let dashboard = Dashboard::load(&config)?;
    assert_eq!(dashboard.accounts.len(), 1);
    assert_eq!(
        dashboard.accounts[0].email.as_deref(),
        Some("excluded@example.test")
    );
    config.exclude.accounts[0].provider = "claude".to_owned();
    assert!(Dashboard::load(&config)?.accounts.is_empty());
    config.exclude.accounts.clear();
    config.exclude.sources.push("omp".to_owned());
    config.sources.push(SourceConfig {
        name: "custom-missing".to_owned(),
        kind: SourceKind::Omp,
        path: first_file.path().join("missing"),
        enabled: true,
        optional: false,
    });
    let excluded = Dashboard::load(&config)?;
    assert!(excluded.accounts.is_empty());
    assert!(excluded.warnings.is_empty());
    config.exclude.sources = vec!["old".to_owned(), "custom-missing".to_owned()];
    let retained = Dashboard::load(&config)?;
    assert_eq!(retained.accounts[0].sources, ["new"]);
    drop(second_file);
    Ok(())
}

#[test]
fn api_keys_dedup_by_provider_key_and_masks_never_expose_short_keys() -> Result<()> {
    let key = "sk-synthetic-private-1234";
    let (first_file, first) = fixture(
        &json!({"openrouter":{"type":"api_key","key":key},"anthropic":{"type":"api_key","key":key}}),
        SourceKind::Omp,
        "first",
    )?;
    let (second_file, second) = fixture(
        &json!({"openrouter":{"type":"api_key","key":key}}),
        SourceKind::Omp,
        "second",
    )?;
    let mut config = DashboardConfig {
        sources: vec![first, second],
        ..DashboardConfig::default()
    };
    let dashboard = Dashboard::load(&config)?;
    assert_eq!(dashboard.accounts.len(), 2);
    let serialized = serde_json::to_string(dashboard.accounts())?;
    assert!(!serialized.contains(key));
    assert!(serialized.contains("sk-s*1234"));
    assert_eq!(mask_key("short-key"), "****");
    assert!(masked_key_matches("*1234", "sk-s*1234"));
    assert!(!masked_key_matches("*9876", "sk-s*1234"));
    config.exclude.api_keys.push("*1234".to_owned());
    assert!(Dashboard::load(&config)?.accounts.is_empty());
    drop((first_file, second_file));
    Ok(())
}

#[test]
fn account_id_and_cross_provider_email_exclusions_are_independent() -> Result<()> {
    let full_account_id = "synthetic-opaque-account-".repeat(20);
    let (file, source) = fixture(
        &json!({
            "anthropic":{"type":"oauth","access":"first","refresh":"refresh-first","accountId":full_account_id,"email":"same@example.test"},
            "openai-codex":{"type":"oauth","access":"second","refresh":"refresh-second","accountId":"second-id","email":"same@example.test"}
        }),
        SourceKind::Omp,
        "fixture",
    )?;
    let mut config = DashboardConfig {
        sources: vec![source],
        ..DashboardConfig::default()
    };
    config.exclude.account_ids.push(full_account_id);
    let only_codex = Dashboard::load(&config)?;
    assert_eq!(only_codex.accounts.len(), 1);
    assert_eq!(only_codex.accounts[0].provider, Provider::Codex);
    config.exclude.account_ids.clear();
    config.exclude.emails.push("SAME@example.test".to_owned());
    assert!(Dashboard::load(&config)?.accounts.is_empty());
    drop(file);
    Ok(())
}

struct FixtureServer {
    endpoint: String,
    requests: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl FixtureServer {
    fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let requests = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let control = Arc::clone(&stop);
        let count = Arc::clone(&requests);
        let peak = Arc::clone(&maximum);
        let worker = std::thread::spawn(move || {
            let mut connections = Vec::new();
            while !control.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        let count = Arc::clone(&count);
                        let active = Arc::clone(&active);
                        let peak = Arc::clone(&peak);
                        connections.push(std::thread::spawn(move || {
                            let _ = socket.set_nonblocking(false);
                            let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
                            let mut data = Vec::new();
                            let mut bytes = [0; 1024];
                            while !data.windows(4).any(|window| window == b"\r\n\r\n") && data.len() < 8192 {
                                match socket.read(&mut bytes) {
                                    Ok(0) | Err(_) => return,
                                    Ok(length) => data.extend_from_slice(&bytes[..length]),
                                }
                            }
                            count.fetch_add(1, Ordering::SeqCst);
                            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                            peak.fetch_max(current, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(80));
                            let request = String::from_utf8_lossy(&data);
                            let (status, body) = if request.starts_with("GET /api/auth/me ") {
                                ("200 OK", r#"{"sub":"user_fixture","email":"person@example.invalid","name":"Fixture Person"}"#)
                            } else if request.starts_with("GET /api/usage-summary ") {
                                ("200 OK", r#"{"membershipType":"pro-plus"}"#)
                            } else if request.starts_with("GET /auth/usage ") || request.contains("synthetic-failed") {
                                ("503 Service Unavailable", r#"{"error":"MUST-NOT-LEAK"}"#)
                            } else {
                                ("200 OK", r#"{"data":{"usage":12.5,"limit":50,"limit_remaining":37.5,"is_management_key":false}}"#)
                            };
                            let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                            let _ = socket.write_all(response.as_bytes());
                            active.fetch_sub(1, Ordering::SeqCst);
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
            for connection in connections {
                let _ = connection.join();
            }
        });
        Ok(Self {
            endpoint,
            requests,
            maximum,
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[tokio::test]
async fn visible_refresh_is_bounded_single_flight_and_failure_isolated() -> Result<()> {
    let server = FixtureServer::start()?;
    let (file, source) = fixture(
        &json!({"openrouter":[
            {"type":"api_key","key":"synthetic-first-key"},
            {"type":"api_key","key":"synthetic-failed-key"},
            {"type":"api_key","key":"synthetic-third-key"},
            {"type":"api_key","key":"synthetic-hidden-key"}
        ]}),
        SourceKind::Omp,
        "fixture",
    )?;
    let config = DashboardConfig {
        sources: vec![source],
        concurrency: 2,
        ..DashboardConfig::default()
    };
    let mut dashboard = Dashboard::load(&config)?;
    dashboard.test_endpoint = Some(server.endpoint.clone());
    assert!(dashboard.refresh(&[]).await.is_empty());
    assert_eq!(server.requests.load(Ordering::SeqCst), 0);
    let visible: Vec<_> = dashboard
        .accounts
        .iter()
        .take(3)
        .map(|account| account.id.clone())
        .collect();
    let (first, second) = tokio::join!(dashboard.refresh(&visible), dashboard.refresh(&visible));
    assert_eq!(server.requests.load(Ordering::SeqCst), 3);
    assert_eq!(server.maximum.load(Ordering::SeqCst), 2);
    assert_eq!(
        first
            .iter()
            .filter(|snapshot| snapshot.usage.is_some())
            .count(),
        2
    );
    assert_eq!(
        second
            .iter()
            .filter(|snapshot| snapshot.error.is_some())
            .count(),
        1
    );
    assert!(!serde_json::to_string(&first)?.contains("MUST-NOT-LEAK"));
    assert!(dashboard.states[3].lock().await.snapshot.is_none());
    drop(file);
    Ok(())
}

#[tokio::test]
async fn shutdown_cancels_queued_work_without_dropping_inflight_requests() -> Result<()> {
    let server = FixtureServer::start()?;
    let keys: Vec<_> = (0..40)
        .map(|index| json!({"type":"api_key","key":format!("synthetic-{index}-key")}))
        .collect();
    let (file, source) = fixture(&json!({"openrouter":keys}), SourceKind::Omp, "fixture")?;
    let config = DashboardConfig {
        sources: vec![source],
        concurrency: 2,
        ..DashboardConfig::default()
    };
    let mut dashboard = Dashboard::load(&config)?;
    dashboard.test_endpoint = Some(server.endpoint.clone());
    let ids: Vec<_> = dashboard
        .accounts
        .iter()
        .map(|account| account.id.clone())
        .collect();
    let cancel = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.requests.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await?;
        dashboard.cancel_pending();
        Ok::<(), eyre::Report>(())
    };
    let (snapshots, canceled) = tokio::join!(dashboard.refresh(&ids), cancel);
    canceled?;
    assert_eq!(server.requests.load(Ordering::SeqCst), 2);
    assert_eq!(
        snapshots
            .iter()
            .filter(|snapshot| snapshot.usage.is_some())
            .count(),
        2
    );
    assert_eq!(snapshots.iter().filter(|snapshot| snapshot.error.as_deref() == Some("refresh canceled during shutdown")).count(), 38);
    drop(file);
    Ok(())
}

#[test]
fn credit_facts_keep_partial_allocation_and_consumption_unknown() {
    let consumed = UsageCredits {
        count: CreditCount::Consumed(CreditAmount::Integer(40)),
        unit: CreditUnit::GenericCredits,
    };
    assert_eq!(consumed.consumed(), Some(CreditAmount::Integer(40)));
    assert!(consumed.allocated().is_none());
    assert!(consumed.remaining().is_none());
    assert!(consumed.used_percent().is_none());
    let allocated = UsageCredits {
        count: CreditCount::Allocated(CreditAmount::Integer(80)),
        unit: CreditUnit::GenericCredits,
    };
    assert_eq!(allocated.allocated(), Some(CreditAmount::Integer(80)));
    assert!(allocated.consumed().is_none());
    assert!(allocated.remaining().is_none());
    assert!(allocated.used_percent().is_none());
    let mut remaining = UsageCredits {
        count: CreditCount::Remaining(CreditAmount::Integer(25)),
        unit: CreditUnit::GenericCredits,
    };
    assert_eq!(remaining.remaining(), Some(CreditAmount::Integer(25)));
    assert!(remaining.used_percent().is_none());
    remaining.unit = CreditUnit::Percentage;
    assert!(remaining.used_percent().is_none());
    remaining.count = CreditCount::Full {
        allocated: CreditAmount::Integer(100),
        consumed: CreditAmount::Integer(75),
    };
    assert!(remaining
        .used_percent()
        .is_some_and(|percent| (percent - 75.0).abs() < f64::EPSILON));
}

#[test]
fn invalid_or_zero_allocations_never_turn_into_known_percentages() {
    for (allocated, consumed) in [
        (0.0, 0.0),
        (0.0, 10.0),
        (f64::INFINITY, 10.0),
        (f64::NAN, 10.0),
        (100.0, f64::INFINITY),
        (100.0, f64::NAN),
    ] {
        let credits = UsageCredits {
            count: CreditCount::Full {
                allocated: CreditAmount::Decimal(allocated),
                consumed: CreditAmount::Decimal(consumed),
            },
            unit: CreditUnit::GenericCredits,
        };
        assert!(credits.used_percent().is_none());
    }
}

#[test]
fn integer_credits_keep_exact_serialization_subtraction_and_overdraw() -> Result<()> {
    let mut credits = UsageCredits {
        count: CreditCount::Full {
            allocated: CreditAmount::Integer(u64::MAX),
            consumed: CreditAmount::Integer(u64::MAX - 3),
        },
        unit: CreditUnit::GenericCredits,
    };
    let decoded: UsageCredits = serde_json::from_str(&serde_json::to_string(&credits)?)?;
    assert_eq!(decoded, credits);
    assert_eq!(decoded.remaining(), Some(CreditAmount::Integer(3)));
    credits.count = CreditCount::Full {
        allocated: CreditAmount::Integer(u64::MAX - 1),
        consumed: CreditAmount::Integer(u64::MAX),
    };
    assert!(credits
        .remaining()
        .is_some_and(|remaining| (remaining.as_f64() + 1.0).abs() < f64::EPSILON));
    Ok(())
}

#[test]
fn anonymous_oauth_labels_are_stable_after_dedup_and_exclusion() -> Result<()> {
    let first =
        json!({"type":"oauth","access":"first-synthetic","accountId":"opaque-first-account-id"});
    let (file, source) = fixture(
        &json!({"anthropic":[
        first.clone(), first,
        {"type":"oauth","access":"second-synthetic","accountId":"opaque-second-account-id"},
        {"type":"oauth","access":"excluded-synthetic","accountId":"excluded-account"}
    ],"cursor":{"type":"oauth","access":"cursor-synthetic","accountId":"opaque-cursor-account-id"}}),
        SourceKind::Omp,
        "fixture",
    )?;
    let mut config = DashboardConfig {
        sources: vec![source],
        ..DashboardConfig::default()
    };
    config
        .exclude
        .account_ids
        .push("excluded-account".to_owned());
    let dashboard = Dashboard::load(&config)?;
    let mut labels: Vec<_> = dashboard
        .accounts
        .iter()
        .filter(|account| account.provider == Provider::Claude)
        .map(|account| account.label.as_str())
        .collect();
    labels.sort_unstable();
    assert_eq!(labels, ["OAuth account 1", "OAuth account 2"]);
    assert!(dashboard
        .accounts
        .iter()
        .all(|account| !account.label.contains("opaque")));
    assert!(dashboard
        .accounts
        .iter()
        .any(|account| account.provider == Provider::Cursor
            && account.label == "OAuth account"
            && account.account_id.as_deref() == Some("opaque-cursor-account-id")));
    let reloaded = Dashboard::load(&config)?;
    assert!(dashboard
        .accounts
        .iter()
        .zip(&reloaded.accounts)
        .all(|(old, new)| old.id == new.id && old.label == new.label));
    drop(file);
    Ok(())
}

#[tokio::test]
async fn cursor_snapshot_keeps_verified_identity_and_plan_when_usage_is_partial() -> Result<()> {
    let server = FixtureServer::start()?;
    let token = format!(
        "header.{}.signature",
        URL_SAFE_NO_PAD
            .encode(json!({"sub":"auth0|user_fixture","exp":4_070_908_800_u64}).to_string())
    );
    let (file, source) = fixture(
        &json!({"cursor":[{"type":"oauth","access":token,
        "refresh":"synthetic-refresh","expires":4_070_908_800_000_u64},
        {"type":"oauth","access":"synthetic-unidentified-cursor"}]}),
        SourceKind::Omp,
        "fixture",
    )?;
    let mut dashboard = Dashboard::load(&DashboardConfig {
        sources: vec![source],
        concurrency: 1,
        ..DashboardConfig::default()
    })?;
    dashboard.test_endpoint = Some(server.endpoint.clone());
    let initial = dashboard
        .accounts
        .first()
        .ok_or_else(|| eyre::eyre!("missing account"))?;
    assert!(initial.label.starts_with("OAuth account "));
    let identity = initial.id.clone();
    let unidentified = dashboard
        .accounts
        .get(1)
        .ok_or_else(|| eyre::eyre!("missing second account"))?;
    assert_ne!(initial.label, unidentified.label);
    let unidentified_id = unidentified.id.clone();
    let unidentified_label = unidentified.label.clone();
    let snapshots = dashboard
        .refresh(&[identity.clone(), unidentified_id.clone()])
        .await;
    let snapshot = snapshots
        .first()
        .ok_or_else(|| eyre::eyre!("missing snapshot"))?;
    assert_eq!(snapshot.account.id, identity);
    assert_eq!(
        snapshot.account.email.as_deref(),
        Some("person@example.invalid")
    );
    assert_eq!(snapshot.account.name.as_deref(), Some("Fixture Person"));
    assert_eq!(snapshot.account.label, "person@example.invalid");
    assert_eq!(snapshot.account.account_id.as_deref(), Some("user_fixture"));
    assert_eq!(
        snapshot
            .usage
            .as_ref()
            .and_then(|usage| usage.plan.as_deref()),
        Some("pro-plus")
    );
    assert!(!serde_json::to_string(snapshot)?.contains("synthetic-refresh"));
    let remaining = snapshots
        .get(1)
        .ok_or_else(|| eyre::eyre!("missing second snapshot"))?;
    assert_eq!(remaining.account.id, unidentified_id);
    assert_eq!(remaining.account.label, unidentified_label);
    assert!(remaining.account.email.is_none());
    drop(file);
    Ok(())
}
