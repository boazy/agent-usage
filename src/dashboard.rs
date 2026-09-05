use crate::api::fetch_account;
use crate::auth::AuthRecord;
use crate::config::{provider_matches, DashboardConfig, Exclusions, SourceConfig, SourceKind};
use crate::sources::{
    acquire_refresh_guard, discover_source, persist_refreshed, DiscoveredCredential,
};
use crate::time::Millis;
use eyre::{Result, WrapErr};
use futures_util::{stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, Semaphore};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Provider {
    Codex,
    Claude,
    OpenRouter,
    Antigravity,
    Cursor,
    Other(String),
}

impl Provider {
    pub(crate) fn from_id(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "codex" | "openai-codex" => Self::Codex,
            "claude" | "anthropic" => Self::Claude,
            "openrouter" | "open_router" => Self::OpenRouter,
            "antigravity" | "google-antigravity" => Self::Antigravity,
            "cursor" => Self::Cursor,
            other => Self::Other(other.to_owned()),
        }
    }

    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::OpenRouter => "openrouter",
            Self::Antigravity => "antigravity",
            Self::Cursor => "cursor",
            Self::Other(value) => value,
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codex => formatter.write_str("Codex"),
            Self::Claude => formatter.write_str("Claude"),
            Self::OpenRouter => formatter.write_str("OpenRouter"),
            Self::Antigravity => formatter.write_str("Antigravity"),
            Self::Cursor => formatter.write_str("Cursor"),
            Self::Other(value) => formatter.write_str(value),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CredentialKind {
    OAuth,
    ApiKey,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AccountInfo {
    /// Stable opaque provider-scoped identity; never a credential or an email.
    pub(crate) id: String,
    pub(crate) provider: Provider,
    pub(crate) label: String,
    pub(crate) email: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) account_id: Option<String>,
    pub(crate) sources: Vec<String>,
    pub(crate) credential_kind: CredentialKind,
    pub(crate) masked_key: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AccountSnapshot {
    pub(crate) account: AccountInfo,
    pub(crate) usage: Option<AccountUsage>,
    pub(crate) error: Option<String>,
    pub(crate) fetched_at: Option<i64>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct AccountUsage {
    pub(crate) limits: SubscriptionLimits,
    pub(crate) plan: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct SubscriptionLimits {
    pub(crate) allowances: Vec<UsageAllowance>,
    pub(crate) balances: Vec<CreditBalance>,
    pub(crate) banked_resets: Vec<BankedReset>,
    pub(crate) global_reset_at: Option<Millis>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct UsageAllowance {
    pub(crate) title: String,
    pub(crate) window: Option<AllowanceWindow>,
    pub(crate) resets_at: Option<Millis>,
    pub(crate) credits: UsageCredits,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AllowanceWindow {
    Seconds(u64),
    Daily,
    Weekly,
    Monthly,
    AllTime,
    Named(String),
}

impl AllowanceWindow {
    pub(crate) const fn from_seconds(seconds: u64) -> Self {
        match seconds {
            86_400 => Self::Daily,
            604_800 => Self::Weekly,
            _ => Self::Seconds(seconds),
        }
    }
}

impl fmt::Display for AllowanceWindow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Seconds(86_400) | Self::Daily => formatter.write_str("daily"),
            Self::Seconds(604_800) | Self::Weekly => formatter.write_str("weekly"),
            Self::Seconds(seconds) => {
                let (count, unit) = if seconds % 3_600 == 0 {
                    (seconds / 3_600, "hour")
                } else if seconds % 60 == 0 {
                    (seconds / 60, "minute")
                } else {
                    (*seconds, "second")
                };
                write!(
                    formatter,
                    "{count} {unit}{}",
                    if count == 1 { "" } else { "s" }
                )
            }
            Self::Monthly => formatter.write_str("monthly"),
            Self::AllTime => formatter.write_str("all time"),
            Self::Named(name) => formatter.write_str(name),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CreditAmount {
    Integer(u64),
    Decimal(f64),
}

impl CreditAmount {
    #[expect(
        clippy::cast_precision_loss,
        reason = "Derived ratios use floating point; stored integer amounts and subtraction remain exact"
    )]
    pub(crate) fn as_f64(self) -> f64 {
        match self {
            Self::Integer(value) => value as f64,
            Self::Decimal(value) => value,
        }
    }

    pub(crate) fn subtract(self, other: Self) -> Self {
        match (self, other) {
            (Self::Integer(left), Self::Integer(right)) if left >= right => {
                Self::Integer(left - right)
            }
            (Self::Integer(left), Self::Integer(right)) => {
                Self::Decimal(-Self::Integer(right - left).as_f64())
            }
            _ => Self::Decimal(self.as_f64() - other.as_f64()),
        }
    }
}

impl fmt::Display for CreditAmount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Integer(value) => value.fmt(formatter),
            Self::Decimal(value) => value.fmt(formatter),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CreditCount {
    Full {
        allocated: CreditAmount,
        consumed: CreditAmount,
    },
    Allocated(CreditAmount),
    Remaining(CreditAmount),
    Consumed(CreditAmount),
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Currency {
    Usd,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CreditUnit {
    Currency(Currency),
    GenericCredits,
    Percentage,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct UsageCredits {
    pub(crate) count: CreditCount,
    pub(crate) unit: CreditUnit,
}

impl UsageCredits {
    pub(crate) fn allocated(&self) -> Option<CreditAmount> {
        match self.count {
            CreditCount::Full { allocated, .. } | CreditCount::Allocated(allocated) => {
                Some(allocated)
            }
            _ => None,
        }
    }

    pub(crate) fn consumed(&self) -> Option<CreditAmount> {
        match self.count {
            CreditCount::Full { consumed, .. } | CreditCount::Consumed(consumed) => Some(consumed),
            _ => None,
        }
    }

    pub(crate) fn remaining(&self) -> Option<CreditAmount> {
        match self.count {
            CreditCount::Full {
                allocated,
                consumed,
            } => Some(allocated.subtract(consumed)),
            CreditCount::Remaining(remaining) => Some(remaining),
            _ => None,
        }
    }

    pub(crate) fn used_percent(&self) -> Option<f64> {
        let allocated = self.allocated()?.as_f64();
        let consumed = self.consumed()?.as_f64();
        (allocated.is_finite() && consumed.is_finite() && allocated > 0.0 && consumed >= 0.0)
            .then(|| consumed / allocated * 100.0)
            .filter(|percent| percent.is_finite())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CreditBalance {
    pub(crate) title: String,
    pub(crate) credits: UsageCredits,
    pub(crate) expires_at: Option<Millis>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct BankedReset {
    pub(crate) title: String,
    /// A reported bank may have neither a count nor individual reset details.
    pub(crate) count: Option<u64>,
    pub(crate) expires_at: Option<Millis>,
}

/// Owns credential material; only redacted account metadata crosses into the UI.
pub(crate) struct Dashboard {
    accounts: Vec<AccountInfo>,
    states: Vec<Mutex<AccountState>>,
    warnings: Vec<String>,
    client: Client,
    permits: Semaphore,
    timeout: Duration,
    codex_base_url: String,
    stopping: AtomicBool,
    #[cfg(test)]
    test_endpoint: Option<String>,
}

struct AccountState {
    credential: DiscoveredCredential,
    snapshot: Option<AccountSnapshot>,
}

impl Dashboard {
    pub(crate) fn load(config: &DashboardConfig) -> Result<Self> {
        config.validate()?;
        let mut credentials = Vec::new();
        let mut warnings = Vec::new();
        for source in &config.sources {
            if !source.enabled || source_excluded(&config.exclude, source) {
                continue;
            }
            match discover_source(source) {
                Ok(discovery) => {
                    warnings.extend(discovery.warnings);
                    credentials.extend(
                        discovery
                            .credentials
                            .into_iter()
                            .map(|credential| (source.name.clone(), credential)),
                    );
                }
                Err(error) => warnings.push(format!("Source {}: {error}", safe_text(&source.name))),
            }
        }
        Self::from_credentials(config, credentials, warnings)
    }

    fn from_credentials(
        config: &DashboardConfig,
        credentials: Vec<(String, DiscoveredCredential)>,
        warnings: Vec<String>,
    ) -> Result<Self> {
        let mut accounts: Vec<AccountInfo> = Vec::new();
        let mut states: Vec<Mutex<AccountState>> = Vec::new();
        let mut indices: HashMap<String, usize> = HashMap::new();
        let mut bearer_indices: HashMap<String, usize> = HashMap::new();
        let mut excluded: Vec<bool> = Vec::new();
        for (source, credential) in credentials {
            let info = account_info(&source, &credential);
            let blocked = account_excluded(&config.exclude, &info);
            let bearer = bearer_identity(&credential);
            if let Some(&index) = indices
                .get(&info.id)
                .or_else(|| bearer_indices.get(&bearer))
            {
                indices.insert(info.id.clone(), index);
                bearer_indices.insert(bearer, index);
                excluded[index] |= blocked;
                let existing: &mut AccountInfo = &mut accounts[index];
                if !existing.sources.contains(&source) {
                    existing.sources.push(safe_text(&source));
                }
                if info.email.is_some() {
                    existing.email.clone_from(&info.email);
                    existing.label.clone_from(&info.label);
                }
                if info.name.is_some() {
                    existing.name.clone_from(&info.name);
                    if existing.email.is_none() {
                        existing.label.clone_from(&info.label);
                    }
                }
                if info.account_id.is_some() {
                    existing.account_id.clone_from(&info.account_id);
                }
                let state: &mut AccountState = states[index].get_mut();
                if credential.auth.expires_at > state.credential.auth.expires_at {
                    state.credential = credential;
                }
                continue;
            }
            indices.insert(info.id.clone(), accounts.len());
            bearer_indices.insert(bearer, accounts.len());
            excluded.push(blocked);
            accounts.push(info);
            states.push(Mutex::new(AccountState {
                credential,
                snapshot: None,
            }));
        }
        let (mut accounts, states): (Vec<_>, Vec<_>) = accounts
            .into_iter()
            .zip(states)
            .zip(excluded)
            .filter_map(|((account, state), excluded)| (!excluded).then_some((account, state)))
            .unzip();
        disambiguate_oauth_accounts(&mut accounts);
        let timeout = Duration::from_secs(config.timeout_seconds);
        let client = Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(10)))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .wrap_err("failed to create usage HTTP client")?;
        Ok(Self {
            accounts,
            states,
            warnings,
            client,
            permits: Semaphore::new(config.concurrency),
            timeout,
            codex_base_url: config.codex_base_url.clone(),
            stopping: AtomicBool::new(false),
            #[cfg(test)]
            test_endpoint: None,
        })
    }

    pub(crate) fn accounts(&self) -> &[AccountInfo] {
        &self.accounts
    }

    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// An empty visible set fetches nothing. Calls can run on an Arc in a UI task.
    /// A global semaphore bounds concurrent calls, including overlapping refreshes.
    pub(crate) async fn refresh(&self, visible_ids: &[String]) -> Vec<AccountSnapshot> {
        let visible: HashSet<&str> = visible_ids.iter().map(String::as_str).collect();
        let indices: Vec<usize> = self
            .accounts
            .iter()
            .enumerate()
            .filter(|(_, account)| visible.contains(account.id.as_str()))
            .map(|(index, _)| index)
            .collect();
        let work = indices
            .into_iter()
            .map(|index| async move { (index, self.refresh_one(index).await) });
        let mut snapshots: Vec<_> = stream::iter(work)
            .buffer_unordered(self.states.len().clamp(1, 32))
            .collect()
            .await;
        snapshots.sort_unstable_by_key(|(index, _)| *index);
        snapshots
            .into_iter()
            .map(|(_, snapshot)| snapshot)
            .collect()
    }

    /// Stop queued fetches during shutdown without dropping an in-flight token rotation.
    pub(crate) fn cancel_pending(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    async fn refresh_one(&self, index: usize) -> AccountSnapshot {
        if self.stopping.load(Ordering::Acquire) {
            return failed_snapshot(&self.accounts[index], "refresh canceled during shutdown");
        }
        let mut state = if let Ok(state) = self.states[index].try_lock() {
            state
        } else {
            let state = self.states[index].lock().await;
            if let Some(snapshot) = &state.snapshot {
                return snapshot.clone();
            }
            state
        };
        let Ok(_permit) = self.permits.acquire().await else {
            return failed_snapshot(&self.accounts[index], "usage fetch queue closed");
        };
        if self.stopping.load(Ordering::Acquire) {
            return failed_snapshot(&self.accounts[index], "refresh canceled during shutdown");
        }
        let prior = state.credential.auth.clone();
        let provider = state.credential.provider.clone();
        let origin = state.credential.origin.clone();
        let lease_duration = self.timeout + Duration::from_secs(10);
        let lease = if prior.kind == CredentialKind::OAuth
            && matches!(
                provider,
                Provider::Codex | Provider::Claude | Provider::Antigravity | Provider::Cursor
            ) {
            match tokio::task::spawn_blocking(move || {
                acquire_refresh_guard(&origin, lease_duration)
            })
            .await
            {
                Ok(Ok(guard)) => Some(guard),
                Ok(Err(error)) => {
                    return failed_snapshot(&self.accounts[index], &error.to_string())
                }
                Err(_) => {
                    return failed_snapshot(
                        &self.accounts[index],
                        "credential refresh coordination failed",
                    )
                }
            }
        } else {
            None
        };
        let result = tokio::time::timeout(
            self.timeout,
            self.fetch_current(&provider, &mut state.credential.auth),
        )
        .await;
        let mut warnings = Vec::new();
        if state.credential.auth.needs_persisted_refresh(&prior) {
            let mut origin = state.credential.origin.clone();
            let current = state.credential.auth.clone();
            match tokio::task::spawn_blocking(move || {
                let result = persist_refreshed(&mut origin, &prior, &current);
                (origin, result)
            })
            .await
            {
                Ok((origin, result)) => {
                    state.credential.origin = origin;
                    if let Err(error) = result {
                        warnings.push(format!("Refreshed credentials could not be saved: {error}"));
                    }
                }
                Err(_) => warnings.push(
                    "Refreshed credentials could not be saved: persistence worker failed"
                        .to_owned(),
                ),
            }
        }
        if let Some(lease) = lease {
            let _ = tokio::task::spawn_blocking(move || drop(lease)).await;
        }
        let (usage, error) = match result {
            Ok(Ok(usage)) => (Some(usage), None),
            Ok(Err(error)) => (None, Some(error.to_string())),
            Err(_) => (None, Some("usage request timed out".to_owned())),
        };
        let mut account = self.accounts[index].clone();
        enrich_account_info(&mut account, &state.credential.auth);
        let snapshot = AccountSnapshot {
            account,
            usage,
            error,
            fetched_at: now_ms(),
            warnings,
        };
        state.snapshot = Some(snapshot.clone());
        snapshot
    }

    async fn fetch_current(
        &self,
        provider: &Provider,
        auth: &mut AuthRecord,
    ) -> Result<AccountUsage> {
        #[cfg(test)]
        if let Some(endpoint) = &self.test_endpoint {
            return crate::api::fetch_account_at(&self.client, provider, auth, endpoint).await;
        }
        fetch_account(&self.client, provider, auth, &self.codex_base_url).await
    }
}

fn failed_snapshot(account: &AccountInfo, message: &str) -> AccountSnapshot {
    AccountSnapshot {
        account: account.clone(),
        usage: None,
        error: Some(message.to_owned()),
        fetched_at: now_ms(),
        warnings: Vec::new(),
    }
}

fn now_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

fn source_excluded(exclusions: &Exclusions, source: &SourceConfig) -> bool {
    let kind = match source.kind {
        SourceKind::Codex => "codex",
        SourceKind::Omp => "omp",
    };
    exclusions
        .sources
        .iter()
        .any(|value| value.eq_ignore_ascii_case(kind) || value == &source.name)
}

fn account_excluded(exclusions: &Exclusions, account: &AccountInfo) -> bool {
    exclusions
        .providers
        .iter()
        .any(|value| provider_matches(value, &account.provider))
        || account.email.as_ref().is_some_and(|email| {
            exclusions
                .emails
                .iter()
                .any(|value| value.trim().eq_ignore_ascii_case(email))
                || exclusions.accounts.iter().any(|rule| {
                    rule.email.trim().eq_ignore_ascii_case(email)
                        && provider_matches(&rule.provider, &account.provider)
                })
        })
        || exclusions
            .account_ids
            .iter()
            .any(|value| value == &account.id || account.account_id.as_ref() == Some(value))
        || account.masked_key.as_ref().is_some_and(|mask| {
            exclusions
                .api_keys
                .iter()
                .any(|pattern| masked_key_matches(pattern, mask))
        })
}

fn masked_key_matches(pattern: &str, mask: &str) -> bool {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return false;
    };
    let Some((visible_prefix, visible_suffix)) = mask.split_once('*') else {
        return false;
    };
    visible_prefix.starts_with(prefix) && visible_suffix.ends_with(suffix)
}

fn account_info(source: &str, credential: &DiscoveredCredential) -> AccountInfo {
    let auth = &credential.auth;
    let masked_key = (auth.kind == CredentialKind::ApiKey).then(|| mask_key(&auth.access_token));
    let email = auth
        .email
        .as_deref()
        .map(|value| safe_text(&value.trim().to_ascii_lowercase()));
    let account_id = auth.account_id.clone();
    let name = auth.name.as_deref().map(safe_text);
    let label = email
        .clone()
        .or_else(|| name.clone())
        .or_else(|| masked_key.clone())
        .unwrap_or_else(|| "OAuth account".to_owned());
    AccountInfo {
        id: account_identity(credential),
        provider: credential.provider.clone(),
        label,
        email,
        name,
        account_id,
        sources: vec![safe_text(source)],
        credential_kind: auth.kind,
        masked_key,
    }
}

fn enrich_account_info(account: &mut AccountInfo, auth: &AuthRecord) {
    if let Some(email) = auth.email.as_deref() {
        account.email = Some(safe_text(&email.trim().to_ascii_lowercase()));
    }
    if let Some(name) = auth.name.as_deref() {
        account.name = Some(safe_text(name.trim()));
    }
    if auth.account_id.is_some() {
        account.account_id.clone_from(&auth.account_id);
    }
    if let Some(label) = account.email.as_ref().or(account.name.as_ref()) {
        account.label.clone_from(label);
    }
}

fn disambiguate_oauth_accounts(accounts: &mut [AccountInfo]) {
    let mut indices: Vec<_> = accounts
        .iter()
        .enumerate()
        .filter(|(_, account)| account.label == "OAuth account")
        .map(|(index, _)| index)
        .collect();
    indices.sort_unstable_by(|&left, &right| {
        accounts[left]
            .provider
            .id()
            .cmp(accounts[right].provider.id())
            .then_with(|| accounts[left].id.cmp(&accounts[right].id))
    });
    let mut start = 0;
    while start < indices.len() {
        let end = start
            + indices[start..]
                .iter()
                .take_while(|&&index| accounts[index].provider == accounts[indices[start]].provider)
                .count();
        if end - start > 1 {
            for (ordinal, &index) in indices[start..end].iter().enumerate() {
                accounts[index].label = format!("OAuth account {}", ordinal + 1);
            }
        }
        start = end;
    }
}

fn account_identity(credential: &DiscoveredCredential) -> String {
    let auth = &credential.auth;
    let mut hash = Sha256::new();
    hash.update(credential.provider.id().as_bytes());
    hash.update([0]);
    if auth.kind == CredentialKind::ApiKey {
        hash.update(b"key\0");
        hash.update(auth.access_token.as_bytes());
    } else {
        hash.update(b"oauth\0");
        let principal = auth
            .subject
            .clone()
            .or_else(|| {
                auth.email
                    .as_ref()
                    .map(|email| email.trim().to_ascii_lowercase())
            })
            .or_else(|| auth.account_id.clone())
            .or_else(|| auth.project_id.clone())
            .unwrap_or_else(|| credential.origin.identity_hint());
        hash.update(principal.as_bytes());
        // One user may have independent seats in several billing organizations.
        hash.update([0]);
        hash.update(
            auth.org_id
                .as_deref()
                .or_else(|| {
                    (credential.provider != Provider::Cursor)
                        .then_some(auth.account_id.as_deref())
                        .flatten()
                })
                .unwrap_or("")
                .as_bytes(),
        );
        hash.update([0]);
        hash.update(auth.project_id.as_deref().unwrap_or("").as_bytes());
    }
    format!("{}:{:x}", credential.provider.id(), hash.finalize())
}

fn bearer_identity(credential: &DiscoveredCredential) -> String {
    let mut hash = Sha256::new();
    hash.update(credential.provider.id().as_bytes());
    hash.update([0]);
    hash.update(credential.auth.access_token.as_bytes());
    // The same grant imported with partial metadata can enrich one account,
    // but different workspace-scoped credentials must never share a pane.
    hash.update([0]);
    hash.update(
        credential
            .auth
            .org_id
            .as_deref()
            .or_else(|| {
                (credential.provider != Provider::Cursor)
                    .then_some(credential.auth.account_id.as_deref())
                    .flatten()
            })
            .unwrap_or("")
            .as_bytes(),
    );
    format!("{:x}", hash.finalize())
}

fn mask_key(key: &str) -> String {
    // Short/malformed values cannot safely reveal a prefix and suffix.
    if key.len() < 12 || !key.is_ascii() || key.bytes().any(|byte| !byte.is_ascii_graphic()) {
        return "****".to_owned();
    }
    format!("{}*{}", &key[..4], &key[key.len() - 4..])
}

fn safe_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect()
}
