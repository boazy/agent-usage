use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use eyre::{eyre, Result, WrapErr};
use serde_json::Value;
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const JWT_AUTH_CLAIM: &str = "https://api.openai.com/auth";
const JWT_PROFILE_CLAIM: &str = "https://api.openai.com/profile";

/// Authentication material deliberately has no `Debug` or `Display` implementation.
#[derive(Clone)]
pub(crate) struct AuthRecord {
    pub(crate) access_token: String,
    pub(crate) id_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) account_id: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) oauth_client_id: Option<String>,
}

impl AuthRecord {
    pub(crate) fn needs_persisted_refresh(&self, prior: &Self) -> bool {
        self.access_token != prior.access_token
            || self.id_token != prior.id_token
            || self.refresh_token != prior.refresh_token
            || self.account_id != prior.account_id
            || self.email != prior.email
        // `oauth_client_id` is derived from the persisted ID/access token and has no independent field.
    }

    pub(crate) fn default_oauth_client_id(&self) -> &str {
        self.oauth_client_id
            .as_deref()
            .unwrap_or("app_EMoamEEZ73f0CkXaXp7hrann")
    }
}

pub(crate) fn load_auth(path: &Path) -> Result<AuthRecord> {
    let raw = fs::read_to_string(path)
        .wrap_err_with(|| format!("failed to read authentication file {}", path.display()))?;
    let payload: Value =
        serde_json::from_str(&raw).wrap_err("authentication file is not valid JSON")?;
    auth_from_value(&payload)
}

pub(crate) fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn auth_from_value(payload: &Value) -> Result<AuthRecord> {
    let access_token = payload
        .pointer("/tokens/access_token")
        .and_then(as_string)
        .or_else(|| payload.pointer("/OPENAI_API_KEY").and_then(as_string))
        .ok_or_else(|| eyre!("authentication file is missing tokens.access_token"))?;
    let id_token = payload.pointer("/tokens/id_token").and_then(as_string);
    let refresh_token = payload.pointer("/tokens/refresh_token").and_then(as_string);
    let oauth_client_id = id_token
        .as_deref()
        .and_then(parse_jwt_aud)
        .or_else(|| parse_jwt_aud(&access_token));
    let account_id = payload
        .pointer("/tokens/account_id")
        .and_then(as_string)
        .or_else(|| parse_jwt_claim(&access_token, JWT_AUTH_CLAIM, "chatgpt_account_id"));
    let email = id_token
        .as_deref()
        .and_then(|token| parse_jwt_claim(token, JWT_PROFILE_CLAIM, "email"))
        .or_else(|| parse_jwt_claim(&access_token, JWT_PROFILE_CLAIM, "email"));

    Ok(AuthRecord {
        access_token,
        id_token,
        refresh_token,
        account_id,
        email,
        oauth_client_id,
    })
}

pub(crate) fn apply_refresh_payload(auth: &mut AuthRecord, payload: &Value) -> Result<()> {
    let access_token = payload
        .get("access_token")
        .and_then(as_string)
        .ok_or_else(|| eyre!("refresh response is missing access_token"))?;
    let id_token = payload.get("id_token").and_then(as_string);
    let refresh_token = payload.get("refresh_token").and_then(as_string);
    auth.access_token = access_token;
    if let Some(id_token) = id_token {
        auth.id_token = Some(id_token);
    }
    if let Some(refresh_token) = refresh_token {
        auth.refresh_token = Some(refresh_token);
    }

    if let Some(id_token) = auth.id_token.as_deref() {
        auth.oauth_client_id = parse_jwt_aud(id_token).or_else(|| auth.oauth_client_id.clone());
        auth.account_id = parse_jwt_claim(id_token, JWT_AUTH_CLAIM, "chatgpt_account_id")
            .or_else(|| parse_jwt_claim(&auth.access_token, JWT_AUTH_CLAIM, "chatgpt_account_id"))
            .or_else(|| auth.account_id.clone());
        if let Some(email) = parse_jwt_claim(id_token, JWT_PROFILE_CLAIM, "email") {
            auth.email = Some(email);
        }
    } else if let Some(email) = parse_jwt_claim(&auth.access_token, JWT_PROFILE_CLAIM, "email") {
        auth.email = Some(email);
    }
    Ok(())
}

pub(crate) fn persist_auth(path: &Path, auth: &AuthRecord) -> Result<()> {
    let raw = fs::read_to_string(path)
        .wrap_err_with(|| format!("failed to read authentication file {}", path.display()))?;
    let mut payload: Value =
        serde_json::from_str(&raw).wrap_err("authentication file is not valid JSON")?;
    let tokens = payload
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| eyre!("authentication file is missing tokens object"))?;
    tokens.insert(
        "access_token".to_owned(),
        Value::String(auth.access_token.clone()),
    );
    replace_token(tokens, "id_token", auth.id_token.as_deref());
    replace_token(tokens, "refresh_token", auth.refresh_token.as_deref());
    replace_token(tokens, "account_id", auth.account_id.as_deref());

    let formatted = serde_json::to_string_pretty(&payload)
        .wrap_err("failed to serialize refreshed authentication")?;
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let (mut file, mut guard) = create_temp_auth_file(path)?;
    file.write_all(formatted.as_bytes()).wrap_err_with(|| {
        format!(
            "failed writing temporary authentication file {}",
            guard.path().display()
        )
    })?;
    file.flush().wrap_err_with(|| {
        format!(
            "failed flushing temporary authentication file {}",
            guard.path().display()
        )
    })?;
    file.sync_all().wrap_err_with(|| {
        format!(
            "failed syncing temporary authentication file {}",
            guard.path().display()
        )
    })?;
    drop(file);
    fs::rename(guard.path(), path)
        .wrap_err_with(|| format!("failed replacing authentication file {}", path.display()))?;
    guard.commit();
    sync_directory_for(directory)
}

fn replace_token(tokens: &mut serde_json::Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        tokens.insert(key.to_owned(), Value::String(value.to_owned()));
    } else {
        tokens.remove(key);
    }
}

struct TempAuthFile {
    path: PathBuf,
    remove_on_drop: bool,
}

impl TempAuthFile {
    fn path(&self) -> &Path {
        &self.path
    }
    fn commit(&mut self) {
        self.remove_on_drop = false;
    }
}

impl Drop for TempAuthFile {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn create_temp_auth_file(path: &Path) -> Result<(fs::File, TempAuthFile)> {
    for attempt in 0..32_u128 {
        let temporary_path = next_temp_auth_path(path, attempt);
        match create_private_temp_file(&temporary_path) {
            Ok(file) => {
                return Ok((
                    file,
                    TempAuthFile {
                        path: temporary_path,
                        remove_on_drop: true,
                    },
                ))
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists && attempt < 31 => {}
            Err(error) => {
                return Err(error).wrap_err_with(|| {
                    format!(
                        "failed to create temporary authentication file {}",
                        temporary_path.display()
                    )
                })
            }
        }
    }
    Err(eyre!(
        "failed to create a unique temporary authentication file"
    ))
}

fn next_temp_auth_path(path: &Path, attempt: u128) -> PathBuf {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    directory.join(format!(
        ".codex-auth-{}-{nanos}-{attempt}.json.tmp",
        std::process::id()
    ))
}

fn create_private_temp_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path)?;
    #[cfg(unix)]
    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}

fn sync_directory_for(path: &Path) -> Result<()> {
    let directory = fs::File::open(path)
        .wrap_err_with(|| format!("failed to open authentication directory {}", path.display()))?;
    directory
        .sync_all()
        .wrap_err("failed to sync authentication directory")
}

fn parse_jwt_aud(token: &str) -> Option<String> {
    let claims = parse_jwt(token)?;
    match claims.get("aud") {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Array(values)) => values.iter().find_map(as_string),
        _ => None,
    }
}

fn parse_jwt_claim(token: &str, top: &str, inner: &str) -> Option<String> {
    parse_jwt(token)?
        .get(top)?
        .as_object()?
        .get(inner)
        .and_then(as_string)
}

fn parse_jwt(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _signature = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let mut payload = payload.to_owned();
    while !payload.len().is_multiple_of(4) {
        payload.push('=');
    }
    serde_json::from_slice(&URL_SAFE.decode(payload).ok()?).ok()
}

fn as_string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{apply_refresh_payload, load_auth, persist_auth, AuthRecord};
    use serde_json::json;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn load_auth_falls_back_to_api_key() {
        let path = std::env::temp_dir().join(format!("codex-usage-auth-{}", std::process::id()));
        fs::write(&path, r#"{"OPENAI_API_KEY":"synthetic-access"}"#)
            .unwrap_or_else(|error| panic!("failed to seed fixture: {error}"));
        let auth =
            load_auth(&path).unwrap_or_else(|error| panic!("failed to load fixture: {error}"));
        fs::remove_file(&path).unwrap_or_else(|error| panic!("failed to remove fixture: {error}"));
        assert_eq!(auth.access_token, "synthetic-access");
    }

    #[test]
    fn refresh_payload_keeps_existing_optional_tokens_when_omitted() {
        let mut auth = AuthRecord {
            access_token: "old".to_owned(),
            id_token: Some("old-id".to_owned()),
            refresh_token: Some("old-refresh".to_owned()),
            account_id: None,
            email: None,
            oauth_client_id: None,
        };
        apply_refresh_payload(&mut auth, &json!({"access_token":"new"}))
            .unwrap_or_else(|error| panic!("failed to apply fixture: {error}"));
        assert_eq!(auth.access_token, "new");
        assert_eq!(auth.id_token.as_deref(), Some("old-id"));
        assert_eq!(auth.refresh_token.as_deref(), Some("old-refresh"));
    }

    #[test]
    fn persistence_replaces_credentials_without_changing_private_file_mode() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|error| panic!("clock moved before epoch: {error}"))
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("codex-usage-auth-{unique}"));
        fs::create_dir(&directory)
            .unwrap_or_else(|error| panic!("failed to create fixture directory: {error}"));
        let path = directory.join("auth.json");
        fs::write(&path, r#"{"unrelated":"kept","tokens":{"access_token":"old","refresh_token":"old","keep":"value"}}"#)
            .unwrap_or_else(|error| panic!("failed to seed fixture: {error}"));
        let auth = AuthRecord {
            access_token: "new".to_owned(),
            id_token: None,
            refresh_token: None,
            account_id: Some("account".to_owned()),
            email: None,
            oauth_client_id: None,
        };

        persist_auth(&path, &auth)
            .unwrap_or_else(|error| panic!("failed to persist fixture: {error}"));
        let updated: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read fixture: {error}")),
        )
        .unwrap_or_else(|error| panic!("failed to parse fixture: {error}"));

        assert_eq!(updated["unrelated"], "kept");
        assert_eq!(updated["tokens"]["access_token"], "new");
        assert_eq!(updated["tokens"]["account_id"], "account");
        assert!(updated["tokens"].get("refresh_token").is_none());
        assert_eq!(updated["tokens"]["keep"], "value");
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path)
                .unwrap_or_else(|error| panic!("failed to inspect fixture: {error}"))
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(directory)
            .unwrap_or_else(|error| panic!("failed to remove fixture directory: {error}"));
    }
}
