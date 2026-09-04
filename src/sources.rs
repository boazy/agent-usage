use crate::auth::{codex_auth_from_value, expand_path, string_field, AuthRecord};
use crate::config::{SourceConfig, SourceKind};
use crate::dashboard::{CredentialKind, Provider};
use eyre::{eyre, Result, WrapErr};
use fs2::FileExt as _;
use rusqlite::{params, Connection, OpenFlags};
use rustix::fs::{open, Mode, OFlags};
use serde_json::{Map, Value};
use std::fs::{self, File, Metadata};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;
const JSON_RACE_WARNING: &str = "JSON refresh persistence uses advisory locks and a final conflict check; a foreign writer that ignores locks can still race the atomic replacement";
static LEASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct SourceDiscovery {
    pub(crate) credentials: Vec<DiscoveredCredential>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) struct DiscoveredCredential {
    pub(crate) provider: Provider,
    pub(crate) auth: AuthRecord,
    pub(crate) origin: CredentialOrigin,
}

/// Contains expected secret snapshots; never implement debugging or serialization.
#[derive(Clone)]
pub(crate) struct CredentialOrigin {
    source: OriginSource,
}

#[derive(Clone)]
enum OriginSource {
    Json(JsonOrigin),
    Sqlite(SqliteOrigin),
}

#[derive(Clone)]
struct JsonOrigin {
    path: PathBuf,
    entry: JsonEntry,
    expected: Value,
}

#[derive(Clone)]
enum JsonEntry {
    CodexOAuth { mode: Option<Value>, api_key: Option<Value> },
    CodexApiKey,
    Omp { provider: String, index: Option<usize> },
}

#[derive(Clone)]
struct SqliteOrigin {
    path: PathBuf,
    identity: FileIdentity,
    id: i64,
    provider: String,
    credential_type: String,
    expected: String,
    lease: Option<Arc<Mutex<Option<String>>>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn of(metadata: &Metadata) -> Self {
        Self { device: metadata.dev(), inode: metadata.ino() }
    }
}

/// Held across the bounded provider request and CAS persistence.
pub(crate) struct RefreshGuard {
    database: Option<DatabaseLease>,
    _advisory_lock: Option<File>,
}

struct DatabaseLease {
    connection: Connection,
    id: i64,
    owner: String,
    active_owner: Arc<Mutex<Option<String>>>,
}

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        if let Some(lease) = &self.database {
            // An expired lease may already belong to a peer; never delete its owner.
            let _ = lease.connection.execute(
                "DELETE FROM auth_credential_refresh_leases WHERE credential_id = ?1 AND owner = ?2",
                params![lease.id, lease.owner],
            );
            if let Ok(mut owner) = lease.active_owner.lock() {
                if owner.as_deref() == Some(lease.owner.as_str()) {
                    *owner = None;
                }
            }
        }
    }
}

pub(crate) fn acquire_refresh_guard(origin: &CredentialOrigin, ttl: Duration) -> Result<RefreshGuard> {
    let origin = match &origin.source {
        OriginSource::Json(origin) => {
            // No JSON refresh lease protocol exists, but reject an already stale
            // snapshot before sending it to a rotating-token endpoint.
            let mut file = open_credential_file(&origin.path, false)?;
            let payload: Value = serde_json::from_slice(&read_json_bytes(&mut file)?)
                .map_err(|_| eyre!("credential source is not valid JSON"))?;
            if json_entry(&payload, &origin.entry)? != &origin.expected {
                return Err(eyre!("JSON credential changed since discovery; reload before retrying"));
            }
            return Ok(RefreshGuard { database: None, _advisory_lock: None });
        }
        OriginSource::Sqlite(origin) => origin,
    };
    let Some(active_owner) = &origin.lease else {
        let lock = lock_json_source(&origin.path)?;
        let connection = open_database(&origin.path, false, origin.identity)?;
        let unchanged: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM auth_credentials WHERE id = ?1 AND provider = ?2 AND credential_type = ?3 AND data = ?4 AND disabled_cause IS NULL)",
            params![origin.id, origin.provider, origin.credential_type, origin.expected], |row| row.get(0),
        ).map_err(|_| eyre!("cannot check OMP credential snapshot"))?;
        if !unchanged {
            return Err(eyre!("OMP credential changed since discovery; reload before retrying"));
        }
        return Ok(RefreshGuard { database: None, _advisory_lock: Some(lock) });
    };
    let now = epoch_millis()?;
    let expires = now.checked_add(i64::try_from(ttl.as_millis()).map_err(|_| eyre!("refresh lease duration is too large"))?)
        .filter(|expires| *expires > now).ok_or_else(|| eyre!("refresh lease duration must be positive"))?;
    let owner = format!("agent-usage:{}:{now}:{}", std::process::id(), LEASE_SEQUENCE.fetch_add(1, Ordering::Relaxed));
    let connection = open_database(&origin.path, true, origin.identity)?;
    let changed = connection.execute(
        "INSERT INTO auth_credential_refresh_leases (credential_id, owner, expires_at_ms, updated_at)
         SELECT ?1, ?2, ?3, CAST(strftime('%s','now') AS INTEGER)
         WHERE EXISTS (SELECT 1 FROM auth_credentials WHERE id = ?1 AND provider = ?5 AND credential_type = ?6 AND data = ?7 AND disabled_cause IS NULL)
         ON CONFLICT(credential_id) DO UPDATE SET owner = excluded.owner, expires_at_ms = excluded.expires_at_ms, updated_at = excluded.updated_at
         WHERE auth_credential_refresh_leases.expires_at_ms <= ?4",
        params![origin.id, owner, expires, now, origin.provider, origin.credential_type, origin.expected],
    ).map_err(|_| eyre!("cannot acquire OMP credential refresh lease"))?;
    if changed != 1 {
        return Err(eyre!("credential is already refreshing or changed since discovery; reload before retrying"));
    }
    let guard = RefreshGuard {
        database: Some(DatabaseLease { connection, id: origin.id, owner: owner.clone(), active_owner: Arc::clone(active_owner) }),
        _advisory_lock: None,
    };
    *active_owner.lock().map_err(|_| eyre!("refresh lease state is unavailable"))? = Some(owner);
    Ok(guard)
}

fn epoch_millis() -> Result<i64> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| eyre!("system time precedes Unix epoch"))?;
    i64::try_from(duration.as_millis()).map_err(|_| eyre!("system time exceeds supported epoch range"))
}

impl CredentialOrigin {
    /// Internal fallback identity only: hash this before it enters a UI snapshot.
    pub(crate) fn identity_hint(&self) -> String {
        match &self.source {
            OriginSource::Json(origin) => {
                let entry = match &origin.entry {
                    JsonEntry::CodexOAuth { .. } => "tokens".to_owned(),
                    JsonEntry::CodexApiKey => "OPENAI_API_KEY".to_owned(),
                    JsonEntry::Omp { provider, index } => format!("{}:{provider}:{index:?}", provider.len()),
                };
                format!("{}:json:{entry}", origin.path.display())
            }
            OriginSource::Sqlite(origin) => format!("{}:sqlite:{}", origin.path.display(), origin.id),
        }
    }
}

pub(crate) fn discover_source(source: &SourceConfig) -> Result<SourceDiscovery> {
    let mut discovery = SourceDiscovery { credentials: Vec::new(), warnings: Vec::new() };
    if !source.enabled {
        return Ok(discovery);
    }
    let expanded = source.path.to_str().map_or_else(|| source.path.clone(), expand_path);
    if source.optional {
        match fs::symlink_metadata(&expanded) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(discovery),
            Err(_) => return Err(eyre!("cannot inspect optional credential source")),
            Ok(_) => {}
        }
    }
    let path = secure_source_path(&expanded)?;
    let mut file = open_credential_file(&path, false)?;
    let mut header = [0; 16];
    let count = file.read(&mut header).wrap_err("cannot inspect credential source format")?;
    file.seek(SeekFrom::Start(0)).wrap_err("cannot rewind credential source")?;
    if source.kind == SourceKind::Omp && count == header.len() && header == *b"SQLite format 3\0" {
        let mut sqlite = discover_sqlite(&path, &file)?;
        warn_readable_source(&file, &mut sqlite)?;
        return Ok(sqlite);
    }
    warn_readable_source(&file, &mut discovery)?;
    let raw = read_json_bytes(&mut file)?;
    let payload: Value = serde_json::from_slice(&raw)
        .map_err(|_| eyre!("credential source is not valid JSON"))?;
    match source.kind {
        SourceKind::Codex => match codex_auth_from_value(&payload) {
            Ok(auth) => {
                let entry = if auth.kind == CredentialKind::OAuth {
                    JsonEntry::CodexOAuth {
                        mode: payload.get("auth_mode").cloned(),
                        api_key: payload.get("OPENAI_API_KEY").cloned(),
                    }
                } else {
                    JsonEntry::CodexApiKey
                };
                let expected = json_entry(&payload, &entry)?.clone();
                discovery.credentials.push(DiscoveredCredential {
                    provider: Provider::Codex,
                    auth,
                    origin: CredentialOrigin { source: OriginSource::Json(JsonOrigin { path, entry, expected }) },
                });
            }
            Err(error) => discovery.warnings.push(error.to_string()),
        },
        SourceKind::Omp => discover_omp_json(&path, &payload, &mut discovery)?,
    }
    if discovery.credentials.iter().any(|credential| credential.auth.kind == CredentialKind::OAuth) {
        discovery.warnings.push(JSON_RACE_WARNING.to_owned());
    }
    Ok(discovery)
}

fn discover_omp_json(path: &Path, payload: &Value, discovery: &mut SourceDiscovery) -> Result<()> {
    let providers = payload.as_object().ok_or_else(|| eyre!("OMP JSON credentials must be a provider map"))?;
    for (provider, value) in providers {
        if let Some(entries) = value.as_array() {
            for (index, entry) in entries.iter().enumerate() {
                discover_omp_entry(path, provider, Some(index), entry, discovery);
            }
        } else {
            discover_omp_entry(path, provider, None, value, discovery);
        }
    }
    Ok(())
}

fn discover_omp_entry(path: &Path, provider: &str, index: Option<usize>, value: &Value, discovery: &mut SourceDiscovery) {
    let kind = value.get("type").and_then(Value::as_str).unwrap_or_default();
    let parsed_provider = Provider::from_id(provider);
    match omp_auth_from_value(value, kind, &parsed_provider) {
        Ok(auth) => discovery.credentials.push(DiscoveredCredential {
            provider: parsed_provider,
            auth,
            origin: CredentialOrigin { source: OriginSource::Json(JsonOrigin {
                path: path.to_path_buf(),
                entry: JsonEntry::Omp { provider: provider.to_owned(), index },
                expected: value.clone(),
            }) },
        }),
        Err(error) => discovery.warnings.push(format!("OMP JSON credential entry skipped: {error}")),
    }
}

fn omp_auth_from_value(value: &Value, kind: &str, provider: &Provider) -> Result<AuthRecord> {
    if !value.is_object() {
        return Err(eyre!("credential entry must be an object"));
    }
    if kind == "api_key" {
        let key = string_field(value, "key").ok_or_else(|| eyre!("API-key entry is missing a nonempty key"))?;
        return Ok(AuthRecord::new(key, CredentialKind::ApiKey));
    }
    if kind != "oauth" {
        return Err(eyre!("unsupported credential kind (expected oauth or api_key)"));
    }
    let access = string_field(value, "access").ok_or_else(|| eyre!("OAuth entry is missing a nonempty access token"))?;
    let mut auth = AuthRecord::new(access, CredentialKind::OAuth);
    auth.refresh_token = string_field(value, "refresh");
    if auth.refresh_token.as_deref() == Some("__remote__") {
        return Err(eyre!("broker-managed OAuth refresh requires the OMP broker and is unsupported"));
    }
    auth.expires_at = match value.get("expires") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_i64().ok_or_else(|| eyre!("OAuth expiry must be Unix milliseconds"))?),
    };
    auth.account_id = string_field(value, "accountId");
    auth.email = string_field(value, "email");
    auth.project_id = string_field(value, "projectId");
    auth.org_id = string_field(value, "orgId");
    auth.enrich_from_tokens(provider);
    Ok(auth)
}

fn discover_sqlite(path: &Path, file: &File) -> Result<SourceDiscovery> {
    let identity = FileIdentity::of(&file.metadata().wrap_err("cannot inspect credential database")?);
    let connection = open_database(path, false, identity)?;
    let lease_supported: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'auth_credential_refresh_leases')",
        [], |row| row.get(0),
    ).map_err(|_| eyre!("cannot inspect OMP refresh lease support"))?;
    let mut statement = connection.prepare(
        "SELECT id, provider, credential_type, data FROM auth_credentials WHERE disabled_cause IS NULL ORDER BY id",
    ).map_err(|_| eyre!("cannot read OMP credential rows; unsupported or inaccessible credential schema"))?;
    let mut rows = statement.query([]).map_err(|_| eyre!("cannot query OMP credential rows"))?;
    let mut discovery = SourceDiscovery { credentials: Vec::new(), warnings: Vec::new() };
    if !lease_supported {
        discovery.warnings.push("This OMP database has no shared refresh lease table; advisory locking coordinates agent-usage only, so another application's simultaneous OAuth refresh can still rotate the grant".to_owned());
    }
    while let Some(row) = rows.next().map_err(|_| eyre!("cannot read OMP credential row"))? {
        let record = (|| -> rusqlite::Result<(i64, String, String, String)> {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })();
        let Ok((id, provider, credential_type, raw)) = record else {
            discovery.warnings.push("OMP database credential row has invalid column types".to_owned());
            continue;
        };
        let parsed_provider = Provider::from_id(&provider);
        let auth = serde_json::from_str::<Value>(&raw)
            .map_err(|_| eyre!("credential data is not valid JSON"))
            .and_then(|value| omp_auth_from_value(&value, &credential_type, &parsed_provider));
        match auth {
            Ok(auth) => discovery.credentials.push(DiscoveredCredential {
                provider: parsed_provider,
                auth,
                origin: CredentialOrigin { source: OriginSource::Sqlite(SqliteOrigin {
                    path: path.to_path_buf(), identity, id, provider, credential_type, expected: raw,
                    lease: lease_supported.then(|| Arc::new(Mutex::new(None))),
                }) },
            }),
            Err(error) => discovery.warnings.push(format!("OMP credential row {id} skipped: {error}")),
        }
    }
    Ok(discovery)
}

pub(crate) fn persist_refreshed(origin: &mut CredentialOrigin, prior: &AuthRecord, current: &AuthRecord) -> Result<()> {
    if prior.kind != CredentialKind::OAuth || current.kind != CredentialKind::OAuth {
        return Err(eyre!("only OAuth credentials may persist a refresh"));
    }
    if !current.needs_persisted_refresh(prior) {
        return Ok(());
    }
    match &mut origin.source {
        OriginSource::Json(origin) => persist_json(origin, prior, current),
        OriginSource::Sqlite(origin) => persist_sqlite(origin, prior, current),
    }
}

fn check_prior(expected: &AuthRecord, prior: &AuthRecord) -> Result<()> {
    // Compare the grant used for the request, not derived identity/expiry hints:
    // Codex JSON does not store expires_in, and OMP does not store Codex ID tokens.
    if expected.kind != prior.kind
        || expected.access_token != prior.access_token
        || expected.refresh_token != prior.refresh_token
    {
        return Err(eyre!("refresh origin does not match the credentials used for the request"));
    }
    Ok(())
}

fn persist_sqlite(origin: &mut SqliteOrigin, prior: &AuthRecord, current: &AuthRecord) -> Result<()> {
    let mut value: Value = serde_json::from_str(&origin.expected).map_err(|_| eyre!("invalid credential snapshot"))?;
    check_prior(&omp_auth_from_value(&value, &origin.credential_type, &Provider::from_id(&origin.provider))?, prior)?;
    update_omp_value(&mut value, prior, current)?;
    let updated = serde_json::to_string(&value).map_err(|_| eyre!("cannot serialize refreshed credential"))?;
    let connection = open_database(&origin.path, true, origin.identity)?;
    // A single SQL statement is a cross-process CAS; do not rewrite the provider pool,
    // identity_key, unrelated columns, or caches. Disabled/login-replaced rows must lose.
    let changed = if let Some(active_owner) = &origin.lease {
        let owner = active_owner.lock().map_err(|_| eyre!("refresh lease state is unavailable"))?
            .clone().ok_or_else(|| eyre!("OMP refresh persistence requires an active refresh guard"))?;
        connection.execute(
            "UPDATE auth_credentials SET data = ?1 WHERE id = ?2 AND provider = ?3 AND credential_type = ?4 AND data = ?5 AND disabled_cause IS NULL
             AND EXISTS (SELECT 1 FROM auth_credential_refresh_leases WHERE credential_id = ?2 AND owner = ?6 AND expires_at_ms > ?7)",
            params![updated, origin.id, origin.provider, origin.credential_type, origin.expected, owner, epoch_millis()?],
        )
    } else {
        connection.execute(
            "UPDATE auth_credentials SET data = ?1 WHERE id = ?2 AND provider = ?3 AND credential_type = ?4 AND data = ?5 AND disabled_cause IS NULL",
            params![updated, origin.id, origin.provider, origin.credential_type, origin.expected],
        )
    }.map_err(|_| eyre!("cannot persist refreshed OMP credential row"))?;
    if changed != 1 {
        return Err(eyre!("refresh conflict: the OMP credential changed, was disabled or removed, or its refresh lease expired; reload before retrying"));
    }
    origin.expected = updated;
    Ok(())
}

fn persist_json(origin: &mut JsonOrigin, prior: &AuthRecord, current: &AuthRecord) -> Result<()> {
    let directory = origin.path.parent().ok_or_else(|| eyre!("credential source has no parent directory"))?;
    check_directory(directory)?;
    let directory_file = File::open(directory).wrap_err("cannot open credential directory")?;
    let directory_identity = FileIdentity::of(&directory_file.metadata().wrap_err("cannot inspect credential directory")?);
    let _lock = lock_json_source(&origin.path)?;
    let mut file = open_credential_file(&origin.path, false)?;
    file.try_lock_exclusive().map_err(|_| eyre!("credential source is locked by another writer"))?;
    let identity = FileIdentity::of(&file.metadata().wrap_err("cannot inspect credential source")?);
    let raw = read_json_bytes(&mut file)?;
    let mut payload: Value = serde_json::from_slice(&raw).map_err(|_| eyre!("credential source is not valid JSON"))?;
    if json_entry(&payload, &origin.entry)? != &origin.expected {
        return Err(eyre!("refresh conflict: JSON credential was changed or replaced; reload before retrying"));
    }
    let expected_auth = match &origin.entry {
        JsonEntry::CodexOAuth { .. } => codex_auth_from_value(&payload)?,
        JsonEntry::CodexApiKey => return Err(eyre!("API-key credentials cannot be refreshed")),
        JsonEntry::Omp { provider, .. } => omp_auth_from_value(&origin.expected, "oauth", &Provider::from_id(provider))?,
    };
    check_prior(&expected_auth, prior)?;
    if matches!(&origin.entry, JsonEntry::CodexOAuth { .. }) && expected_auth.id_token != prior.id_token {
        return Err(eyre!("refresh origin does not match the Codex ID token used for the request"));
    }
    let entry = json_entry_mut(&mut payload, &origin.entry)?;
    match &origin.entry {
        JsonEntry::CodexOAuth { .. } => update_codex_value(entry, prior, current)?,
        JsonEntry::Omp { .. } => update_omp_value(entry, prior, current)?,
        JsonEntry::CodexApiKey => return Err(eyre!("API-key credentials cannot be refreshed")),
    }
    let next_expected = entry.clone();
    let formatted = serde_json::to_vec_pretty(&payload).map_err(|_| eyre!("cannot serialize refreshed credential source"))?;
    let mut temporary = tempfile::Builder::new().prefix(".agent-usage-auth-").tempfile_in(directory)
        .wrap_err("cannot create private temporary credential file")?;
    temporary.as_file().set_permissions(fs::Permissions::from_mode(0o600))
        .wrap_err("cannot secure temporary credential file")?;
    temporary.write_all(&formatted).wrap_err("cannot write temporary credential file")?;
    temporary.as_file().sync_all().wrap_err("cannot sync temporary credential file")?;
    // Re-read the full latest source immediately before replacement, not just our
    // entry: an unrelated account/login changed during serialization must survive.
    ensure_json_unchanged(&origin.path, &raw, identity)?;
    check_directory(directory)?;
    if FileIdentity::of(&fs::metadata(directory).wrap_err("cannot inspect credential directory")?) != directory_identity {
        return Err(eyre!("refresh conflict: credential directory was replaced"));
    }
    // The stable sidecar protects our writers across rename. Foreign writers that
    // ignore both advisory locks retain a check-to-rename race; discovery says so.
    temporary.persist(&origin.path).map_err(|_| eyre!("cannot atomically replace credential source"))?;
    origin.expected = next_expected;
    directory_file.sync_all().wrap_err("credential refresh written, but directory sync failed")?;
    Ok(())
}

fn ensure_json_unchanged(path: &Path, expected: &[u8], identity: FileIdentity) -> Result<()> {
    let mut file = open_credential_file(path, false)?;
    let current_identity = FileIdentity::of(&file.metadata().wrap_err("cannot inspect credential source")?);
    if current_identity != identity || read_json_bytes(&mut file)? != expected {
        return Err(eyre!("refresh conflict: credential source changed before replacement; reload before retrying"));
    }
    Ok(())
}

fn json_entry<'a>(payload: &'a Value, entry: &JsonEntry) -> Result<&'a Value> {
    let value = match entry {
        JsonEntry::CodexOAuth { mode, api_key } => {
            if payload.get("auth_mode") != mode.as_ref() || payload.get("OPENAI_API_KEY") != api_key.as_ref() {
                return Err(eyre!("refresh conflict: Codex login mode changed"));
            }
            payload.get("tokens")
        }
        JsonEntry::CodexApiKey => payload.get("OPENAI_API_KEY"),
        JsonEntry::Omp { provider, index } => payload.get(provider).and_then(|value| {
            index.map_or(Some(value), |index| value.as_array().and_then(|values| values.get(index)))
        }),
    };
    value.ok_or_else(|| eyre!("refresh conflict: originating JSON credential entry is missing"))
}

fn json_entry_mut<'a>(payload: &'a mut Value, entry: &JsonEntry) -> Result<&'a mut Value> {
    let value = match entry {
        JsonEntry::CodexOAuth { .. } => payload.get_mut("tokens"),
        JsonEntry::CodexApiKey => payload.get_mut("OPENAI_API_KEY"),
        JsonEntry::Omp { provider, index } => payload.get_mut(provider).and_then(|value| {
            match index {
                Some(index) => value.as_array_mut().and_then(|values| values.get_mut(*index)),
                None => Some(value),
            }
        }),
    };
    value.ok_or_else(|| eyre!("originating JSON credential entry is missing"))
}

fn update_codex_value(value: &mut Value, prior: &AuthRecord, current: &AuthRecord) -> Result<()> {
    let object = value.as_object_mut().ok_or_else(|| eyre!("Codex token entry is not an object"))?;
    object.insert("access_token".to_owned(), Value::String(current.access_token.clone()));
    replace_changed_string(object, "id_token", prior.id_token.as_deref(), current.id_token.as_deref());
    replace_changed_string(object, "refresh_token", prior.refresh_token.as_deref(), current.refresh_token.as_deref());
    replace_changed_string(object, "account_id", prior.account_id.as_deref(), current.account_id.as_deref());
    Ok(())
}

fn update_omp_value(value: &mut Value, prior: &AuthRecord, current: &AuthRecord) -> Result<()> {
    let object = value.as_object_mut().ok_or_else(|| eyre!("OMP credential entry is not an object"))?;
    object.insert("access".to_owned(), Value::String(current.access_token.clone()));
    replace_changed_string(object, "refresh", prior.refresh_token.as_deref(), current.refresh_token.as_deref());
    replace_changed_string(object, "accountId", prior.account_id.as_deref(), current.account_id.as_deref());
    replace_changed_string(object, "email", prior.email.as_deref(), current.email.as_deref());
    replace_changed_string(object, "projectId", prior.project_id.as_deref(), current.project_id.as_deref());
    replace_changed_string(object, "orgId", prior.org_id.as_deref(), current.org_id.as_deref());
    if prior.expires_at != current.expires_at {
        if let Some(expires) = current.expires_at {
            object.insert("expires".to_owned(), Value::from(expires));
        } else {
            object.remove("expires");
        }
    }
    Ok(())
}

fn replace_changed_string(object: &mut Map<String, Value>, key: &str, prior: Option<&str>, current: Option<&str>) {
    if prior == current {
        return;
    }
    if let Some(current) = current {
        object.insert(key.to_owned(), Value::String(current.to_owned()));
    } else {
        object.remove(key);
    }
}

fn secure_source_path(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let directory = parent.canonicalize().wrap_err("cannot resolve credential source directory")?;
    check_directory(&directory)?;
    let name = path.file_name().ok_or_else(|| eyre!("credential source must name a file"))?;
    Ok(directory.join(name))
}

fn check_directory(directory: &Path) -> Result<()> {
    let current_uid = rustix::process::geteuid().as_raw();
    for ancestor in directory.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).wrap_err("cannot inspect credential directory security")?;
        if !metadata.is_dir() || (metadata.uid() != current_uid && metadata.uid() != 0) {
            return Err(eyre!("credential directory must be owned by the current user or root and cannot be a symlink"));
        }
        let mode = metadata.permissions().mode();
        let protected_temporary_root = mode & 0o1000 != 0;
        if mode & 0o022 != 0 && !protected_temporary_root {
            return Err(eyre!("credential directory is writable by another user; refusing insecure access"));
        }
    }
    Ok(())
}

fn open_credential_file(path: &Path, write: bool) -> Result<File> {
    let access = if write { OFlags::RDWR } else { OFlags::RDONLY };
    let descriptor = open(path, access | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty())
        .map_err(|_| eyre!("cannot securely open credential file (missing, inaccessible, or symlink)"))?;
    let file = File::from(descriptor);
    check_credential_file(&file)?;
    if write {
        file.set_permissions(fs::Permissions::from_mode(0o600)).wrap_err("cannot make credential persistence private")?;
    }
    Ok(file)
}

fn check_credential_file(file: &File) -> Result<()> {
    let metadata = file.metadata().wrap_err("cannot inspect credential file security")?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(eyre!("credential file must be a regular, singly-linked file owned by the current user"));
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        return Err(eyre!("credential file is writable by another user; refusing insecure access"));
    }
    Ok(())
}

fn warn_readable_source(file: &File, discovery: &mut SourceDiscovery) -> Result<()> {
    if file.metadata().wrap_err("cannot inspect credential source permissions")?.permissions().mode() & 0o077 != 0 {
        discovery.warnings.push("Credential source is readable by other users; restrict its permissions to mode 0600. Refresh persistence writes private credentials.".to_owned());
    }
    Ok(())
}

fn read_json_bytes(file: &mut File) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(MAX_JSON_BYTES + 1).read_to_end(&mut bytes).wrap_err("cannot read credential JSON")?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_JSON_BYTES {
        return Err(eyre!("credential JSON exceeds the supported size limit"));
    }
    Ok(bytes)
}

fn lock_json_source(path: &Path) -> Result<File> {
    let name = path.file_name().ok_or_else(|| eyre!("credential source must name a file"))?;
    let mut lock_name = std::ffi::OsString::from(".");
    lock_name.push(name);
    lock_name.push(".agent-usage.lock");
    let lock_path = path.with_file_name(lock_name);
    let descriptor = open(&lock_path,
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    ).map_err(|_| eyre!("cannot open private credential lock file"))?;
    let file = File::from(descriptor);
    check_credential_file(&file)?;
    if file.metadata().wrap_err("cannot inspect credential lock permissions")?.permissions().mode() & 0o077 != 0 {
        return Err(eyre!("credential lock file must have private mode 0600"));
    }
    file.try_lock_exclusive().map_err(|_| eyre!("credential source refresh is already in progress"))?;
    // Deliberately retain the sidecar: removing it permits concurrent lock inodes.
    Ok(file)
}

fn open_database(path: &Path, writable: bool, identity: FileIdentity) -> Result<Connection> {
    let directory = path.parent().ok_or_else(|| eyre!("credential database has no parent"))?;
    check_directory(directory)?;
    let file = open_credential_file(path, writable)?;
    if FileIdentity::of(&file.metadata().wrap_err("cannot inspect credential database")?) != identity {
        return Err(eyre!("refresh conflict: credential database was replaced"));
    }
    // SQLite manages its WAL/journal, but we never query those or any cache/log table.
    // Refuse existing sidecar links or permissive sidecars before SQLite opens them.
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        match fs::symlink_metadata(&sidecar) {
            Ok(_) => { open_credential_file(&sidecar, writable)?; }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(eyre!("cannot inspect credential database sidecar security")),
        }
    }
    let access = if writable { OpenFlags::SQLITE_OPEN_READ_WRITE } else { OpenFlags::SQLITE_OPEN_READ_ONLY };
    let connection = Connection::open_with_flags(path, access | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW)
        .map_err(|_| eyre!("cannot open OMP credential database"))?;
    connection.busy_timeout(Duration::from_secs(2)).map_err(|_| eyre!("cannot bound credential database lock wait"))?;
    let reopened = open_credential_file(path, false)?;
    if FileIdentity::of(&reopened.metadata().wrap_err("cannot inspect credential database")?) != identity {
        return Err(eyre!("refresh conflict: credential database changed while opening"));
    }
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::{acquire_refresh_guard, discover_source, ensure_json_unchanged, persist_refreshed, FileIdentity};
    use crate::config::{SourceConfig, SourceKind};
    use crate::dashboard::{CredentialKind, Provider};
    use eyre::{eyre, Result};
    use rusqlite::{params, Connection};
    use serde_json::{json, Value};
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    fn source(path: &Path, kind: SourceKind) -> SourceConfig {
        SourceConfig { name: "fixture".to_owned(), kind, path: path.to_path_buf(), enabled: true, optional: false }
    }

    fn write_json(path: &Path, value: &Value) -> Result<()> {
        fs::write(path, serde_json::to_vec(value)?)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    fn read_json(path: &Path) -> Result<Value> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn database_fixture(with_leases: bool) -> Result<(TempDir, Connection)> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("agent.db");
        let connection = Connection::open(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        connection.execute_batch(
            "CREATE TABLE auth_credentials (
                id INTEGER PRIMARY KEY, provider TEXT NOT NULL, credential_type TEXT NOT NULL,
                data TEXT NOT NULL, disabled_cause TEXT, identity_key TEXT, unrelated TEXT
            );",
        )?;
        if with_leases {
            connection.execute_batch(
                "CREATE TABLE auth_credential_refresh_leases (
                    credential_id INTEGER PRIMARY KEY, owner TEXT NOT NULL,
                    expires_at_ms INTEGER NOT NULL, updated_at INTEGER NOT NULL
                );",
            )?;
        }
        Ok((directory, connection))
    }

    fn insert_oauth(connection: &Connection, id: i64) -> Result<String> {
        let value = json!({"access":format!("access-{id}"),"refresh":format!("refresh-{id}"),
            "expires":1000,"accountId":"shared-workspace","orgId":"workspace",
            "email":format!("person-{id}@example.invalid"),"unknown":{"nested":"retained"}});
        let raw = value.to_string();
        connection.execute(
            "INSERT INTO auth_credentials VALUES (?1, 'openai-codex', 'oauth', ?2, NULL, ?3, 'keep-column')",
            params![id, raw, format!("email:person-{id}@example.invalid|org:workspace")],
        )?;
        Ok(raw)
    }

    #[test]
    fn legacy_json_discovers_arrays_singletons_and_unknown_providers() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("auth.json");
        write_json(&path, &json!({
            "anthropic":[
                {"type":"oauth","access":"one","refresh":"r1","expires":1000,"email":"same@example.invalid","orgId":"org-one"},
                {"type":"oauth","access":"two","refresh":"r2","expires":2000,"email":"same@example.invalid","orgId":"org-two"}
            ],
            "openrouter":{"type":"api_key","key":"synthetic-key"},
            "new-provider":{"type":"api_key","key":"synthetic-other"},
            "unsupported":{"type":"future-secret-kind","secret":"must-not-be-in-warning"}
        }))?;
        let discovery = discover_source(&source(&path, SourceKind::Omp))?;
        let claude: Vec<_> = discovery.credentials.iter().filter(|entry| entry.provider == Provider::Claude).collect();
        assert_eq!(claude.len(), 2);
        assert_eq!(claude[0].auth.org_id.as_deref(), Some("org-one"));
        assert_eq!(claude[1].auth.org_id.as_deref(), Some("org-two"));
        let key = discovery.credentials.iter().find(|entry| entry.provider == Provider::OpenRouter)
            .ok_or_else(|| eyre!("fixture key missing"))?;
        assert_eq!(key.auth.kind, CredentialKind::ApiKey);
        assert_eq!(key.auth.access_token, "synthetic-key");
        assert!(discovery.credentials.iter().any(|entry| entry.provider == Provider::Other("new-provider".to_owned())));
        assert!(discovery.warnings.iter().any(|warning| warning.contains("unsupported credential kind")));
        assert!(discovery.warnings.iter().all(|warning| !warning.contains("must-not-be-in-warning")));
        Ok(())
    }

    #[test]
    fn codex_keys_and_oauth_use_distinct_source_entries() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let oauth_path = directory.path().join("personal.json");
        let key_path = directory.path().join("key.json");
        write_json(&oauth_path, &json!({"OPENAI_API_KEY":null,"tokens":{"access_token":"oauth","refresh_token":"refresh","account_id":"workspace"}}))?;
        write_json(&key_path, &json!({"OPENAI_API_KEY":"synthetic-api-key"}))?;
        let oauth = discover_source(&source(&oauth_path, SourceKind::Codex))?;
        let key = discover_source(&source(&key_path, SourceKind::Codex))?;
        assert_eq!(oauth.credentials[0].auth.kind, CredentialKind::OAuth);
        assert_eq!(oauth.credentials[0].auth.account_id.as_deref(), Some("workspace"));
        assert_eq!(key.credentials[0].auth.kind, CredentialKind::ApiKey);
        assert!(key.credentials[0].auth.refresh_token.is_none());
        assert_ne!(oauth.credentials[0].origin.identity_hint(), key.credentials[0].origin.identity_hint());
        Ok(())
    }

    #[test]
    fn json_refresh_merges_other_accounts_and_rejects_stale_refresh() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("auth.json");
        let initial = json!({"anthropic":[
            {"type":"oauth","access":"first","refresh":"r1","expires":1000,"unknown":"keep"},
            {"type":"oauth","access":"second","refresh":"r2","expires":2000}
        ],"openrouter":{"type":"api_key","key":"unrelated"}});
        write_json(&path, &initial)?;
        let mut discovery = discover_source(&source(&path, SourceKind::Omp))?;
        let entry = &mut discovery.credentials[0];
        let prior = entry.auth.clone();
        let mut refreshed = prior.clone();
        refreshed.access_token = "first-new".to_owned();
        refreshed.refresh_token = Some("r1-new".to_owned());
        refreshed.expires_at = Some(9000);
        let mut stale_origin = entry.origin.clone();
        let hint = entry.origin.identity_hint();
        let mut concurrent = initial;
        concurrent["anthropic"][1]["access"] = json!("second-concurrently-updated");
        concurrent["unrelated-top-level"] = json!({"keep":true});
        write_json(&path, &concurrent)?;
        persist_refreshed(&mut entry.origin, &prior, &refreshed)?;
        let result = read_json(&path)?;
        assert_eq!(result["anthropic"][0]["access"], "first-new");
        assert_eq!(result["anthropic"][0]["refresh"], "r1-new");
        assert_eq!(result["anthropic"][0]["expires"], 9000);
        assert_eq!(result["anthropic"][0]["unknown"], "keep");
        assert_eq!(result["anthropic"][1], concurrent["anthropic"][1]);
        assert_eq!(result["openrouter"], concurrent["openrouter"]);
        assert_eq!(result["unrelated-top-level"], concurrent["unrelated-top-level"]);
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        assert_eq!(entry.origin.identity_hint(), hint);
        assert!(persist_refreshed(&mut stale_origin, &prior, &refreshed).is_err());
        assert_eq!(read_json(&path)?, result);
        Ok(())
    }

    #[test]
    fn codex_refresh_preserves_unknown_fields_and_rejects_login_switch() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("auth.json");
        write_json(&path, &json!({"auth_mode":"chatgpt","OPENAI_API_KEY":null,
            "last_refresh":"leave-unrelated-metadata","tokens":{"access_token":"old","refresh_token":"refresh","extra":{"keep":true}}}))?;
        let mut discovery = discover_source(&source(&path, SourceKind::Codex))?;
        let entry = &mut discovery.credentials[0];
        let prior = entry.auth.clone();
        let mut refreshed = prior.clone();
        refreshed.access_token = "new".to_owned();
        refreshed.refresh_token = Some("new-refresh".to_owned());
        persist_refreshed(&mut entry.origin, &prior, &refreshed)?;
        let mut result = read_json(&path)?;
        assert_eq!(result["tokens"]["access_token"], "new");
        assert_eq!(result["tokens"]["refresh_token"], "new-refresh");
        assert_eq!(result["tokens"]["extra"], json!({"keep":true}));
        assert_eq!(result["last_refresh"], "leave-unrelated-metadata");
        result["auth_mode"] = json!("apikey");
        result["OPENAI_API_KEY"] = json!("new-login-key");
        write_json(&path, &result)?;
        let mut next = refreshed.clone();
        next.access_token = "stale-refresh".to_owned();
        assert!(persist_refreshed(&mut entry.origin, &refreshed, &next).is_err());
        assert_eq!(read_json(&path)?, result);
        Ok(())
    }

    #[test]
    fn json_final_conflict_check_detects_unrelated_edits_and_inode_replacement() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("auth.json");
        write_json(&path, &json!({"tokens":{"access_token":"old"}}))?;
        let expected = fs::read(&path)?;
        let identity = FileIdentity::of(&fs::metadata(&path)?);
        write_json(&path, &json!({"tokens":{"access_token":"old"},"unrelated":"concurrent"}))?;
        assert!(ensure_json_unchanged(&path, &expected, identity).is_err());
        let replacement = directory.path().join("replacement.json");
        fs::write(&replacement, &expected)?;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        fs::rename(&replacement, &path)?;
        assert!(ensure_json_unchanged(&path, &expected, identity).is_err());
        Ok(())
    }

    #[test]
    fn sqlite_discovery_reads_only_active_credential_schema_without_initialization() -> Result<()> {
        let (directory, connection) = database_fixture(false)?;
        insert_oauth(&connection, 1)?;
        insert_oauth(&connection, 2)?;
        connection.execute("UPDATE auth_credentials SET disabled_cause = 'disabled' WHERE id = 2", [])?;
        connection.execute("INSERT INTO auth_credentials VALUES (3,'openrouter','api_key',?1,NULL,NULL,NULL)", [json!({"key":"synthetic-key"}).to_string()])?;
        connection.execute("INSERT INTO auth_credentials VALUES (4,'unknown','future_kind','{}',NULL,NULL,NULL)", [])?;
        let before: i64 = connection.query_row("PRAGMA schema_version", [], |row| row.get(0))?;
        let discovery = discover_source(&source(&directory.path().join("agent.db"), SourceKind::Omp))?;
        let after: i64 = connection.query_row("PRAGMA schema_version", [], |row| row.get(0))?;
        assert_eq!(discovery.credentials.len(), 2);
        assert_eq!(discovery.credentials[0].auth.email.as_deref(), Some("person-1@example.invalid"));
        assert_eq!(discovery.credentials[1].auth.kind, CredentialKind::ApiKey);
        assert_eq!(discovery.credentials[1].auth.access_token, "synthetic-key");
        assert!(discovery.warnings.iter().any(|warning| warning.contains("unsupported credential kind")));
        assert_eq!(before, after);
        let lease_tables: i64 = connection.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name = 'auth_credential_refresh_leases'", [], |row| row.get(0))?;
        assert_eq!(lease_tables, 0);
        Ok(())
    }

    #[test]
    fn sqlite_cas_targets_exact_row_and_preserves_unknown_fields() -> Result<()> {
        let (directory, connection) = database_fixture(true)?;
        insert_oauth(&connection, 1)?;
        let second = insert_oauth(&connection, 2)?;
        let mut discovery = discover_source(&source(&directory.path().join("agent.db"), SourceKind::Omp))?;
        let entry = &mut discovery.credentials[0];
        let prior = entry.auth.clone();
        let mut refreshed = prior.clone();
        refreshed.access_token = "new-one".to_owned();
        refreshed.refresh_token = Some("new-r1".to_owned());
        refreshed.expires_at = Some(8000);
        let mut stale_origin = entry.origin.clone();
        let _guard = acquire_refresh_guard(&entry.origin, Duration::from_secs(30))?;
        persist_refreshed(&mut entry.origin, &prior, &refreshed)?;
        let (raw, identity, unrelated): (String, String, String) = connection.query_row(
            "SELECT data,identity_key,unrelated FROM auth_credentials WHERE id = 1", [],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        )?;
        let value: Value = serde_json::from_str(&raw)?;
        assert_eq!(value["access"], "new-one");
        assert_eq!(value["refresh"], "new-r1");
        assert_eq!(value["expires"], 8000);
        assert_eq!(value["unknown"], json!({"nested":"retained"}));
        assert_eq!(identity, "email:person-1@example.invalid|org:workspace");
        assert_eq!(unrelated, "keep-column");
        let other: String = connection.query_row("SELECT data FROM auth_credentials WHERE id = 2", [], |row| row.get(0))?;
        assert_eq!(other, second);
        assert!(persist_refreshed(&mut stale_origin, &prior, &refreshed).is_err());
        Ok(())
    }

    #[test]
    fn sqlite_disabled_and_replaced_credentials_cannot_be_refreshed() -> Result<()> {
        let (directory, connection) = database_fixture(false)?;
        insert_oauth(&connection, 1)?;
        let mut discovery = discover_source(&source(&directory.path().join("agent.db"), SourceKind::Omp))?;
        let entry = &mut discovery.credentials[0];
        let prior = entry.auth.clone();
        let mut refreshed = prior.clone();
        refreshed.access_token = "must-not-write".to_owned();
        connection.execute("UPDATE auth_credentials SET disabled_cause = 'login disabled' WHERE id = 1", [])?;
        assert!(persist_refreshed(&mut entry.origin, &prior, &refreshed).is_err());
        connection.execute("UPDATE auth_credentials SET disabled_cause = NULL, provider = 'anthropic' WHERE id = 1", [])?;
        assert!(persist_refreshed(&mut entry.origin, &prior, &refreshed).is_err());
        Ok(())
    }

    #[test]
    fn omp_refresh_leases_block_foreign_refresh_and_release_only_their_owner() -> Result<()> {
        let (directory, connection) = database_fixture(true)?;
        insert_oauth(&connection, 1)?;
        let mut discovery = discover_source(&source(&directory.path().join("agent.db"), SourceKind::Omp))?;
        let entry = &mut discovery.credentials[0];
        connection.execute("INSERT INTO auth_credential_refresh_leases VALUES (1,'foreign-owner',9223372036854775807,0)", [])?;
        assert!(acquire_refresh_guard(&entry.origin, Duration::from_secs(30)).is_err());
        connection.execute("UPDATE auth_credential_refresh_leases SET expires_at_ms = 0 WHERE credential_id = 1", [])?;
        let guard = acquire_refresh_guard(&entry.origin, Duration::from_secs(30))?;
        assert!(acquire_refresh_guard(&entry.origin, Duration::from_secs(30)).is_err());
        let prior = entry.auth.clone();
        let mut refreshed = prior.clone();
        refreshed.access_token = "new".to_owned();
        connection.execute("UPDATE auth_credential_refresh_leases SET owner = 'replacement-owner', expires_at_ms = 9223372036854775807 WHERE credential_id = 1", [])?;
        assert!(persist_refreshed(&mut entry.origin, &prior, &refreshed).is_err());
        drop(guard);
        let owner: String = connection.query_row("SELECT owner FROM auth_credential_refresh_leases WHERE credential_id = 1", [], |row| row.get(0))?;
        assert_eq!(owner, "replacement-owner");
        connection.execute("DELETE FROM auth_credential_refresh_leases", [])?;
        drop(acquire_refresh_guard(&entry.origin, Duration::from_secs(30))?);
        let count: i64 = connection.query_row("SELECT COUNT(*) FROM auth_credential_refresh_leases", [], |row| row.get(0))?;
        assert_eq!(count, 0);
        Ok(())
    }

    #[test]
    fn insecure_files_and_symlinks_are_rejected_before_secret_reads() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("auth.json");
        write_json(&path, &json!({"OPENAI_API_KEY":"synthetic-key"}))?;
        let link = directory.path().join("link.json");
        symlink(&path, &link)?;
        assert!(discover_source(&source(&link, SourceKind::Codex)).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666))?;
        assert!(discover_source(&source(&path, SourceKind::Codex)).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let hardlink = directory.path().join("hardlink.json");
        fs::hard_link(&path, &hardlink)?;
        assert!(discover_source(&source(&hardlink, SourceKind::Codex)).is_err());
        Ok(())
    }

    #[test]
    fn readable_omp_database_warns_without_mutation_and_refresh_makes_it_private() -> Result<()> {
        let (directory, connection) = database_fixture(true)?;
        insert_oauth(&connection, 1)?;
        let path = directory.path().join("agent.db");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
        let mut discovery = discover_source(&source(&path, SourceKind::Omp))?;
        assert!(discovery.warnings.iter().any(|warning| warning.contains("readable by other users")));
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o644);
        let entry = &mut discovery.credentials[0];
        let prior = entry.auth.clone();
        let mut refreshed = prior.clone();
        refreshed.access_token = "private-refresh".to_owned();
        let _guard = acquire_refresh_guard(&entry.origin, Duration::from_secs(30))?;
        persist_refreshed(&mut entry.origin, &prior, &refreshed)?;
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        Ok(())
    }

    #[test]
    fn repeated_omp_refresh_does_not_require_unstored_codex_id_token() -> Result<()> {
        let (directory, connection) = database_fixture(true)?;
        insert_oauth(&connection, 1)?;
        let mut discovery = discover_source(&source(&directory.path().join("agent.db"), SourceKind::Omp))?;
        let entry = &mut discovery.credentials[0];
        let prior = entry.auth.clone();
        let mut first = prior.clone();
        first.access_token = "first-refresh".to_owned();
        first.id_token = Some("provider-returned-id-token".to_owned());
        let guard = acquire_refresh_guard(&entry.origin, Duration::from_secs(30))?;
        persist_refreshed(&mut entry.origin, &prior, &first)?;
        drop(guard);
        let mut second = first.clone();
        second.access_token = "second-refresh".to_owned();
        let _guard = acquire_refresh_guard(&entry.origin, Duration::from_secs(30))?;
        persist_refreshed(&mut entry.origin, &first, &second)?;
        let raw: String = connection.query_row("SELECT data FROM auth_credentials WHERE id = 1", [], |row| row.get(0))?;
        let value: Value = serde_json::from_str(&raw)?;
        assert_eq!(value["access"], "second-refresh");
        assert!(value.get("id_token").is_none());
        Ok(())
    }

    #[test]
    fn readonly_wal_discovery_keeps_permissions_and_persistence_secures_sidecars() -> Result<()> {
        let (directory, connection) = database_fixture(true)?;
        let path = directory.path().join("agent.db");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
        let mode: String = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        assert_eq!(mode, "wal");
        insert_oauth(&connection, 1)?;
        let wal = directory.path().join("agent.db-wal");
        let shm = directory.path().join("agent.db-shm");
        fs::set_permissions(&wal, fs::Permissions::from_mode(0o644))?;
        fs::set_permissions(&shm, fs::Permissions::from_mode(0o644))?;
        let mut discovery = discover_source(&source(&path, SourceKind::Omp))?;
        assert_eq!(discovery.credentials[0].auth.access_token, "access-1");
        assert_eq!(fs::metadata(&wal)?.permissions().mode() & 0o777, 0o644);
        let entry = &mut discovery.credentials[0];
        let prior = entry.auth.clone();
        let mut refreshed = prior.clone();
        refreshed.access_token = "wal-refresh".to_owned();
        let _guard = acquire_refresh_guard(&entry.origin, Duration::from_secs(30))?;
        persist_refreshed(&mut entry.origin, &prior, &refreshed)?;
        for private_path in [&path, &wal, &shm] {
            assert_eq!(fs::metadata(private_path)?.permissions().mode() & 0o777, 0o600);
        }
        let raw: String = connection.query_row("SELECT data FROM auth_credentials WHERE id = 1", [], |row| row.get(0))?;
        assert_eq!(serde_json::from_str::<Value>(&raw)?["access"], "wal-refresh");
        Ok(())
    }
}
