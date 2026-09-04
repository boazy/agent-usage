use super::{mask_key, masked_key_matches, Dashboard, Provider};
use crate::config::{AccountExclusion, DashboardConfig, SourceConfig, SourceKind};
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
    let (file, source) = fixture(
        &json!({
            "anthropic":{"type":"oauth","access":"first","refresh":"refresh-first","accountId":"first-id","email":"same@example.test"},
            "openai-codex":{"type":"oauth","access":"second","refresh":"refresh-second","accountId":"second-id","email":"same@example.test"}
        }),
        SourceKind::Omp,
        "fixture",
    )?;
    let mut config = DashboardConfig {
        sources: vec![source],
        ..DashboardConfig::default()
    };
    config.exclude.account_ids.push("first-id".to_owned());
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
                            let failed = String::from_utf8_lossy(&data).contains("synthetic-failed");
                            let status = if failed { "503 Service Unavailable" } else { "200 OK" };
                            let body = if failed { "{\"error\":\"MUST-NOT-LEAK\"}" } else { "{\"data\":{\"usage\":12.5,\"limit\":50,\"limit_remaining\":37.5,\"is_management_key\":false}}" };
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
