use super::{safe_text, AccountInfo, AccountSnapshot, AccountUsage, Provider};
use crate::auth::AuthRecord;
use crate::time::Millis;
use eyre::{eyre, Result, WrapErr};
use fs2::FileExt as _;
#[cfg(unix)]
use rustix::fs::{open, Mode, OFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::fs::OpenOptions;
use std::fs::{self, File, Metadata};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MINIMUM_SECONDS: u64 = 300;
const MAX_BACKOFF_SECONDS: u64 = 3_600;
const MAX_JSON_BYTES: u64 = 1024 * 1024;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

pub(super) fn directory() -> Option<PathBuf> {
    platform_dirs::AppDirs::new(Some("agent-usage"), true)
        .map(|directories| directories.cache_dir.join("claude"))
}

/// Only the hash of the provider-scoped identity reaches the filesystem.
struct CacheKey([u8; 32]);

impl CacheKey {
    fn from_account(account: &AccountInfo) -> Self {
        Self(Sha256::digest(account.id.as_bytes()).into())
    }

    fn path(&self, directory: &Path, extension: &str) -> PathBuf {
        let digest = sha2::digest::Output::<Sha256>::from(self.0);
        directory.join(format!("{digest:x}.{extension}"))
    }
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
struct CredentialFingerprint([u8; 32]);

impl CredentialFingerprint {
    fn anonymous(auth: &AuthRecord) -> Option<Self> {
        (auth.subject.is_none()
            && auth.email.is_none()
            && auth.account_id.is_none()
            && auth.project_id.is_none())
        .then(|| Self(Sha256::digest(auth.access_token.as_bytes()).into()))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    usage: Option<AccountUsage>,
    // When usage exists, this is always its original successful fetch time.
    fetched_at: Option<i64>,
    next_request: Millis,
    error: Option<String>,
    previous_backoff_seconds: u64,
    #[serde(default)]
    credential_fingerprint: Option<CredentialFingerprint>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            usage: None,
            fetched_at: None,
            next_request: Millis::new(0),
            error: None,
            previous_backoff_seconds: 0,
            credential_fingerprint: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn of(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        {
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        }
        #[cfg(windows)]
        {
            // Match sources.rs's strongest stable Windows replacement signal.
            Self {
                device: 0,
                inode: metadata.creation_time(),
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = metadata;
            Self {
                device: 0,
                inode: 0,
            }
        }
    }
}

/// The stable sidecar lock remains held across reading, requesting and saving.
pub(super) struct Cache {
    guard: File,
    directory: PathBuf,
    directory_identity: FileIdentity,
    key: CacheKey,
    state: State,
}

impl Cache {
    pub(super) fn open(directory: &Path, account: &AccountInfo) -> Result<Self> {
        if account.provider != Provider::Claude {
            return Err(eyre!("Claude usage cache only accepts Claude accounts"));
        }
        let directory = private_directory(directory)?;
        let directory_identity = FileIdentity::of(
            &fs::metadata(&directory).wrap_err("cannot inspect Claude cache directory")?,
        );
        let key = CacheKey::from_account(account);
        let guard = open_private_file(&key.path(&directory, "lock"), true)?;
        guard.try_lock_exclusive().map_err(|_| {
            eyre!("Claude usage cache/request is already in progress for this account")
        })?;
        let state = match existing_file(&key.path(&directory, "json"))? {
            Some(file) => {
                let mut bytes = Vec::new();
                file.take(MAX_JSON_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .wrap_err("cannot read Claude usage cache")?;
                if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_JSON_BYTES {
                    return Err(eyre!("Claude usage cache exceeds the 1 MiB limit"));
                }
                let state: State = serde_json::from_slice(&bytes).map_err(|_| {
                    eyre!("Claude usage cache is malformed; refusing to reset its cooldown")
                })?;
                if !matches!(
                    state.previous_backoff_seconds,
                    0 | 300 | 600 | 1200 | 2400 | 3600
                ) {
                    return Err(eyre!("Claude usage cache has invalid cooldown state"));
                }
                state
            }
            None => State::default(),
        };
        let cache = Self {
            guard,
            directory,
            directory_identity,
            key,
            state,
        };
        cache.check_location()?;
        Ok(cache)
    }

    /// Anonymous identities name a source slot, not a principal. Keep the slot's
    /// throttle when its grant changes, but never reuse the previous reading.
    pub(super) fn bind_credential(&mut self, auth: &AuthRecord) -> bool {
        let fingerprint = CredentialFingerprint::anonymous(auth);
        if self.state.credential_fingerprint == fingerprint {
            return false;
        }
        self.state.credential_fingerprint = fingerprint;
        self.state.usage = None;
        self.state.fetched_at = None;
        self.state.error =
            Some("Claude credentials changed; waiting for the existing usage cooldown".to_owned());
        true
    }

    pub(super) fn cached(&self, account: &AccountInfo, now: Millis) -> Option<AccountSnapshot> {
        let next_request = self.state.next_request;
        if now >= next_request {
            return None;
        }
        Some(self.snapshot(account, now, Vec::new(), true))
    }

    pub(super) fn record(
        &mut self,
        snapshot: AccountSnapshot,
        rate_limited: bool,
        retry_after: Option<Duration>,
        now: Millis,
    ) -> AccountSnapshot {
        let success = snapshot.error.is_none() && snapshot.usage.is_some();
        let backoff = if !success && rate_limited {
            if self.state.previous_backoff_seconds == 0 {
                MINIMUM_SECONDS
            } else {
                self.state
                    .previous_backoff_seconds
                    .saturating_mul(2)
                    .min(MAX_BACKOFF_SECONDS)
            }
        } else {
            0
        };
        self.state.previous_backoff_seconds = backoff;
        let server_seconds = if rate_limited && !success {
            retry_after.map_or(0, |delay| {
                delay
                    .as_secs()
                    .saturating_add(u64::from(delay.subsec_nanos() != 0))
            })
        } else {
            0
        };
        let delay = MINIMUM_SECONDS.max(backoff).max(server_seconds);
        self.state.next_request = now.saturating_add_seconds(delay);
        self.state.error = snapshot.error.as_deref().map(safe_text);
        if success || self.state.usage.is_none() {
            self.state.usage.clone_from(&snapshot.usage);
            self.state.fetched_at = snapshot.fetched_at;
        }
        if success {
            // A newly fetched success has no cache/cooldown warning.
            snapshot
        } else {
            self.snapshot(&snapshot.account, now, snapshot.warnings, false)
        }
    }

    fn snapshot(
        &self,
        account: &AccountInfo,
        now: Millis,
        mut warnings: Vec<String>,
        cached: bool,
    ) -> AccountSnapshot {
        if let (Some(_), Some(error)) = (&self.state.usage, &self.state.error) {
            warnings.push(format!(
                "Claude usage request failed: {error}; showing last successful usage (stale)."
            ));
        }
        let remaining = self
            .state
            .next_request
            .get()
            .saturating_sub(now.get())
            .div_ceil(1_000);
        warnings.push(if cached {
            format!("Claude usage is cached; next request permitted in {remaining} seconds.")
        } else {
            format!("Claude usage cooldown: next request permitted in {remaining} seconds.")
        });
        AccountSnapshot {
            account: account.clone(),
            usage: self.state.usage.clone(),
            error: if self.state.usage.is_some() {
                None
            } else {
                self.state.error.clone()
            },
            fetched_at: self.state.fetched_at,
            warnings,
        }
    }

    pub(super) fn save(&self) -> Result<()> {
        self.check_location()?;
        // Inspect an existing destination before replacing it; never overwrite an
        // insecure file or symlink, even though rename itself would not follow it.
        existing_file(&self.key.path(&self.directory, "json"))?;
        let bytes =
            serde_json::to_vec(&self.state).wrap_err("cannot serialize Claude usage cache")?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_JSON_BYTES {
            return Err(eyre!("Claude usage cache exceeds the 1 MiB limit"));
        }
        let mut temporary = tempfile::Builder::new()
            .prefix(".claude-usage-")
            .tempfile_in(&self.directory)
            .wrap_err("cannot create private temporary Claude cache file")?;
        #[cfg(unix)]
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .wrap_err("cannot secure temporary Claude cache file")?;
        temporary
            .write_all(&bytes)
            .wrap_err("cannot write Claude usage cache")?;
        temporary
            .as_file()
            .sync_all()
            .wrap_err("cannot sync Claude usage cache")?;
        self.check_location()?;
        temporary
            .persist(self.key.path(&self.directory, "json"))
            .map_err(|_| eyre!("cannot atomically replace Claude usage cache"))?;
        #[cfg(unix)]
        File::open(&self.directory)
            .and_then(|directory| directory.sync_all())
            .wrap_err("Claude usage cache written, but directory sync failed")?;
        Ok(())
    }

    fn check_location(&self) -> Result<()> {
        check_directory(&self.directory, true)?;
        if FileIdentity::of(
            &fs::metadata(&self.directory).wrap_err("cannot inspect Claude cache directory")?,
        ) != self.directory_identity
        {
            return Err(eyre!("Claude cache directory was replaced"));
        }
        let current = existing_file(&self.key.path(&self.directory, "lock"))?
            .ok_or_else(|| eyre!("Claude cache lock file was removed"))?;
        if FileIdentity::of(
            &current
                .metadata()
                .wrap_err("cannot inspect Claude cache lock")?,
        ) != FileIdentity::of(
            &self
                .guard
                .metadata()
                .wrap_err("cannot inspect Claude cache lock")?,
        ) {
            return Err(eyre!("Claude cache lock file was replaced"));
        }
        Ok(())
    }
}

fn private_directory(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            // Check the final component before canonicalizing its ancestors, as
            // sources.rs does, so a cache directory itself cannot be a link.
            check_directory(path, true)?;
            let directory = path
                .canonicalize()
                .wrap_err("cannot resolve Claude cache directory")?;
            for ancestor in directory.ancestors() {
                check_directory(ancestor, ancestor == directory)?;
            }
            Ok(directory)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let parent = if parent.exists() {
                let parent = parent
                    .canonicalize()
                    .wrap_err("cannot resolve Claude cache parent")?;
                for ancestor in parent.ancestors() {
                    check_directory(ancestor, false)?;
                }
                parent
            } else {
                private_directory(parent)?
            };
            let name = path
                .file_name()
                .ok_or_else(|| eyre!("Claude cache directory must have a name"))?;
            let directory = parent.join(name);
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            let builder = {
                let mut builder = builder;
                builder.mode(0o700);
                builder
            };
            match builder.create(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error).wrap_err("cannot create private Claude cache directory")
                }
            }
            private_directory(&directory)
        }
        Err(error) => Err(error).wrap_err("cannot inspect Claude cache directory"),
    }
}

fn check_directory(path: &Path, private: bool) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).wrap_err("cannot inspect Claude cache directory security")?;
    if !metadata.is_dir() {
        return Err(eyre!(
            "Claude cache directory must be a directory, not a symlink"
        ));
    }
    #[cfg(unix)]
    {
        let current_uid = rustix::process::geteuid().as_raw();
        let mode = metadata.permissions().mode();
        if (metadata.uid() != current_uid && metadata.uid() != 0)
            || (private && metadata.uid() != current_uid)
        {
            return Err(eyre!("Claude cache directory has an unsafe owner"));
        }
        if (private && mode & 0o077 != 0) || (mode & 0o022 != 0 && mode & 0o1000 == 0) {
            return Err(eyre!(
                "Claude cache directory must be private and not writable by other users"
            ));
        }
    }
    #[cfg(windows)]
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(eyre!("Claude cache directory cannot be a reparse point"));
    }
    #[cfg(not(unix))]
    let _ = private;
    Ok(())
}

fn existing_file(path: &Path) -> Result<Option<File>> {
    match fs::symlink_metadata(path) {
        Ok(_) => open_private_file(path, false).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).wrap_err("cannot inspect Claude cache file"),
    }
}

fn open_private_file(path: &Path, create: bool) -> Result<File> {
    #[cfg(unix)]
    let file = {
        let flags = if create {
            OFlags::RDWR | OFlags::CREATE
        } else {
            OFlags::RDONLY
        };
        let descriptor = open(
            path,
            flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| eyre!("cannot securely open Claude cache file (inaccessible or symlink)"))?;
        File::from(descriptor)
    };
    #[cfg(windows)]
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| eyre!("cannot securely open Claude cache file (inaccessible or symlink)"))?;
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, create);
        return Err(eyre!(
            "secure Claude cache files are unsupported on this platform"
        ));
    }
    #[cfg(any(unix, windows))]
    {
        let metadata = file
            .metadata()
            .wrap_err("cannot inspect Claude cache file security")?;
        if !metadata.is_file() {
            return Err(eyre!("Claude cache file must be a regular file"));
        }
        #[cfg(unix)]
        if metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(eyre!("Claude cache file must be singly linked, owned by the current user, and private (0600)"));
        }
        #[cfg(windows)]
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(eyre!("Claude cache file cannot be a reparse point"));
        }
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::CredentialKind;

    fn private_tempdir() -> Result<tempfile::TempDir> {
        let directory = tempfile::tempdir()?;
        #[cfg(unix)]
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))?;
        Ok(directory)
    }

    fn account(id: &str) -> AccountInfo {
        AccountInfo {
            id: id.to_owned(),
            provider: Provider::Claude,
            label: "private-label".to_owned(),
            email: Some("private@example.test".to_owned()),
            name: None,
            account_id: Some("private-account-id".to_owned()),
            sources: vec!["/private/credential/path".to_owned()],
            credential_kind: CredentialKind::OAuth,
            masked_key: None,
        }
    }

    fn success(account: &AccountInfo, timestamp: i64) -> AccountSnapshot {
        AccountSnapshot {
            account: account.clone(),
            usage: Some(AccountUsage::default()),
            error: None,
            fetched_at: Some(timestamp),
            warnings: Vec::new(),
        }
    }

    fn failure(account: &AccountInfo, timestamp: i64) -> AccountSnapshot {
        AccountSnapshot {
            account: account.clone(),
            usage: None,
            error: Some("Claude HTTP 429".to_owned()),
            fetched_at: Some(timestamp),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn backoff_grows_caps_honors_server_and_resets() -> Result<()> {
        let directory = private_tempdir()?;
        let account = account("claude:test-account");
        let mut cache = Cache::open(directory.path(), &account)?;
        let mut now = Millis::new(10_000);
        for seconds in [300, 600, 1200, 2400, 3600, 3600] {
            cache.record(failure(&account, 10), true, None, now);
            assert_eq!(cache.state.previous_backoff_seconds, seconds);
            assert_eq!(
                cache.state.next_request,
                now.saturating_add_seconds(seconds)
            );
            now = now.saturating_add_seconds(seconds);
        }
        cache.record(
            failure(&account, 20),
            true,
            Some(Duration::from_millis(7_200_001)),
            now,
        );
        assert_eq!(cache.state.next_request, now.saturating_add_seconds(7_201));
        assert_eq!(cache.state.previous_backoff_seconds, 3600);
        cache.record(success(&account, 30), false, None, now);
        assert_eq!(cache.state.previous_backoff_seconds, 0);
        assert_eq!(cache.state.next_request, now.saturating_add_seconds(300));
        cache.record(
            failure(&account, 40),
            true,
            Some(Duration::from_secs(1)),
            now,
        );
        assert_eq!(cache.state.next_request, now.saturating_add_seconds(300));
        Ok(())
    }

    #[test]
    fn failure_and_cached_reads_preserve_success_timestamp() -> Result<()> {
        let directory = private_tempdir()?;
        let account = account("claude:timestamp");
        let mut cache = Cache::open(directory.path(), &account)?;
        let now = Millis::new(1_000_000);
        assert!(cache.cached(&account, now).is_none());
        cache.record(success(&account, 123), false, None, now);
        let stale = cache.record(failure(&account, 456), true, None, now);
        assert!(stale.usage.is_some());
        assert!(stale.error.is_none());
        assert_eq!(stale.fetched_at, Some(123));
        assert!(stale
            .warnings
            .iter()
            .any(|warning| warning.contains("429") && warning.contains("stale")));
        let cached = cache
            .cached(&account, now.saturating_add_seconds(60))
            .ok_or_else(|| eyre!("expected cached successful usage"))?;
        assert!(cached.usage.is_some());
        assert!(cached.error.is_none());
        assert_eq!(cached.fetched_at, Some(123));
        assert!(cached
            .warnings
            .iter()
            .any(|warning| warning.contains("240 seconds")));
        assert!(cache
            .cached(&account, now.saturating_add_seconds(300))
            .is_none());
        Ok(())
    }

    #[test]
    fn failure_without_success_preserves_error_and_attempt_timestamp() -> Result<()> {
        let directory = private_tempdir()?;
        let account = account("claude:failed");
        let mut cache = Cache::open(directory.path(), &account)?;
        let now = Millis::new(1_000);
        let failed = cache.record(failure(&account, 789), true, None, now);
        assert!(failed.usage.is_none());
        assert_eq!(failed.error.as_deref(), Some("Claude HTTP 429"));
        let cached = cache
            .cached(&account, Millis::new(1_001))
            .ok_or_else(|| eyre!("expected cached failure"))?;
        assert_eq!(cached.fetched_at, Some(789));
        assert_eq!(cached.error, failed.error);
        assert!(cached
            .warnings
            .iter()
            .any(|warning| warning.contains("300 seconds")));
        Ok(())
    }

    #[test]
    fn accounts_are_isolated_locks_exclude_and_reopening_preserves_state() -> Result<()> {
        let directory = private_tempdir()?;
        let first = account("claude:first");
        let second = account("claude:second");
        let now = Millis::new(1_000);
        let mut cache = Cache::open(directory.path(), &first)?;
        let conflict = Cache::open(directory.path(), &first)
            .err()
            .ok_or_else(|| eyre!("expected account lock contention"))?;
        assert!(conflict.to_string().contains("already in progress"));
        let independent = Cache::open(directory.path(), &second)?;
        assert!(independent.cached(&second, now).is_none());
        cache.record(success(&first, 123), false, None, now);
        cache.record(failure(&first, 456), true, None, now);
        cache.save()?;
        let path = cache.key.path(&cache.directory, "json");
        let serialized = fs::read_to_string(path)?;
        for secret in [
            first.id.as_str(),
            first.label.as_str(),
            "private@example.test",
            "private-account-id",
            first.sources[0].as_str(),
        ] {
            assert!(!serialized.contains(secret));
        }
        drop(cache);
        let mut current_account = first.clone();
        current_account.label = "current label".to_owned();
        let mut reopened = Cache::open(directory.path(), &current_account)?;
        let cached = reopened
            .cached(&current_account, Millis::new(2_000))
            .ok_or_else(|| eyre!("expected persisted cached usage"))?;
        assert_eq!(cached.account.label, "current label");
        assert_eq!(cached.fetched_at, Some(123));
        assert!(cached.usage.is_some());
        assert!(cached.error.is_none());
        reopened.record(failure(&current_account, 789), true, None, now);
        assert_eq!(reopened.state.previous_backoff_seconds, 600);
        Ok(())
    }

    #[test]
    fn malformed_and_oversized_cache_fail_closed() -> Result<()> {
        let directory = private_tempdir()?;
        let account = account("claude:malformed");
        let mut cache = Cache::open(directory.path(), &account)?;
        cache.record(success(&account, 1), false, None, Millis::new(1_000));
        cache.save()?;
        let path = cache.key.path(&cache.directory, "json");
        drop(cache);
        fs::write(&path, b"{}")?;
        assert!(Cache::open(directory.path(), &account).is_err());
        fs::write(&path, vec![b' '; usize::try_from(MAX_JSON_BYTES + 1)?])?;
        assert!(Cache::open(directory.path(), &account).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_and_insecure_files() -> Result<()> {
        use std::os::unix::fs::symlink;
        let directory = private_tempdir()?;
        let account = account("claude:insecure");
        let cache = Cache::open(directory.path(), &account)?;
        let path = cache.key.path(&cache.directory, "json");
        drop(cache);
        let target = directory.path().join("target");
        fs::write(&target, b"{}")?;
        symlink(&target, &path)?;
        assert!(Cache::open(directory.path(), &account).is_err());
        fs::remove_file(&path)?;
        fs::write(&path, b"{}")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666))?;
        assert!(Cache::open(directory.path(), &account).is_err());
        Ok(())
    }
}
