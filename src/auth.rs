use crate::dashboard::{CredentialKind, Provider};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use eyre::{eyre, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const JWT_AUTH_CLAIM: &str = "https://api.openai.com/auth";
const JWT_PROFILE_CLAIM: &str = "https://api.openai.com/profile";

/// Secret material deliberately has no `Debug`, `Display`, or serialization implementation.
#[derive(Clone)]
pub(crate) struct AuthRecord {
    pub(crate) access_token: String,
    pub(crate) id_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) account_id: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) oauth_client_id: Option<String>,
    pub(crate) kind: CredentialKind,
    /// Unix epoch milliseconds, matching OMP's persisted `expires` field.
    pub(crate) expires_at: Option<i64>,
    pub(crate) project_id: Option<String>,
    pub(crate) org_id: Option<String>,
    /// A person/user identity, never a substitute for the workspace/account ID.
    pub(crate) subject: Option<String>,
}

impl AuthRecord {
    pub(crate) fn new(access_token: String, kind: CredentialKind) -> Self {
        Self {
            access_token,
            kind,
            id_token: None,
            refresh_token: None,
            account_id: None,
            email: None,
            oauth_client_id: None,
            expires_at: None,
            project_id: None,
            org_id: None,
            subject: None,
        }
    }

    pub(crate) fn needs_persisted_refresh(&self, prior: &Self) -> bool {
        self.access_token != prior.access_token
            || self.id_token != prior.id_token
            || self.refresh_token != prior.refresh_token
            || self.account_id != prior.account_id
            || self.email != prior.email
            || self.expires_at != prior.expires_at
            || self.project_id != prior.project_id
            || self.org_id != prior.org_id
        // Client and subject metadata are derived from persisted tokens, not new storage fields.
    }

    pub(crate) fn default_oauth_client_id(&self) -> &str {
        self.oauth_client_id
            .as_deref()
            .unwrap_or("app_EMoamEEZ73f0CkXaXp7hrann")
    }

    /// Decode only documented identity claims. This is not signature verification;
    /// these local hints must never authorize access or select a network endpoint.
    pub(crate) fn enrich_from_tokens(&mut self, provider: &Provider) {
        if self.kind != CredentialKind::OAuth {
            return;
        }
        let access = parse_jwt(&self.access_token);
        let identity = self.id_token.as_deref().and_then(parse_jwt);
        for claims in identity.iter().chain(access.iter()) {
            if self.email.is_none() {
                self.email = string_field(claims, "email").or_else(|| {
                    claims
                        .get(JWT_PROFILE_CLAIM)
                        .and_then(|profile| string_field(profile, "email"))
                });
            }
            if self.subject.is_none() {
                self.subject = if *provider == Provider::Codex {
                    claims
                        .get(JWT_AUTH_CLAIM)
                        .and_then(|auth| {
                            string_field(auth, "chatgpt_user_id")
                                .or_else(|| string_field(auth, "user_id"))
                        })
                        .or_else(|| string_field(claims, "sub"))
                } else {
                    string_field(claims, "sub")
                };
            }
            if *provider == Provider::Codex {
                if self.account_id.is_none() {
                    self.account_id = claims
                        .get(JWT_AUTH_CLAIM)
                        .and_then(|auth| string_field(auth, "chatgpt_account_id"));
                }
                if self.oauth_client_id.is_none() {
                    self.oauth_client_id = match claims.get("aud") {
                        Some(Value::String(value)) if value.starts_with("app_") => {
                            Some(value.clone())
                        }
                        Some(Value::Array(values)) => values
                            .iter()
                            .filter_map(Value::as_str)
                            .find(|value| value.starts_with("app_"))
                            .map(str::to_owned),
                        _ => None,
                    };
                }
            }
        }
        if *provider == Provider::Codex && self.org_id.is_none() {
            self.org_id.clone_from(&self.account_id);
        }
        if self.expires_at.is_none() {
            self.expires_at = access
                .as_ref()
                .and_then(|claims| claims.get("exp"))
                .and_then(Value::as_i64)
                .and_then(|seconds| seconds.checked_mul(1_000));
        }
    }
}

pub(crate) fn expand_path(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(path)
}

pub(crate) fn codex_auth_from_value(payload: &Value) -> Result<AuthRecord> {
    let mode = payload.get("auth_mode").and_then(Value::as_str);
    if mode.is_some_and(|mode| !matches!(mode, "apikey" | "chatgpt"))
        || (mode.is_none()
            && [
                "personal_access_token",
                "bedrock_api_key",
                "bedrock_access_keys",
            ]
            .iter()
            .any(|field| payload.get(*field).is_some_and(|value| !value.is_null())))
    {
        return Err(eyre!("unsupported Codex credential kind"));
    }
    if mode == Some("apikey")
        || (mode.is_none()
            && payload
                .get("OPENAI_API_KEY")
                .is_some_and(|key| !key.is_null()))
    {
        let key = string_field(payload, "OPENAI_API_KEY")
            .ok_or_else(|| eyre!("Codex API-key credential is missing a nonempty key"))?;
        return Ok(AuthRecord::new(key, CredentialKind::ApiKey));
    }
    let tokens = payload
        .get("tokens")
        .ok_or_else(|| eyre!("Codex OAuth credential is missing tokens"))?;
    let access = string_field(tokens, "access_token")
        .ok_or_else(|| eyre!("Codex OAuth credential is missing a nonempty access token"))?;
    let mut auth = AuthRecord::new(access, CredentialKind::OAuth);
    auth.id_token = string_field(tokens, "id_token");
    auth.refresh_token = string_field(tokens, "refresh_token");
    auth.account_id = string_field(tokens, "account_id");
    auth.enrich_from_tokens(&Provider::Codex);
    Ok(auth)
}

pub(crate) fn apply_refresh_payload(auth: &mut AuthRecord, payload: &Value) -> Result<()> {
    if auth.kind != CredentialKind::OAuth {
        return Err(eyre!("API-key credentials cannot be refreshed as OAuth"));
    }
    let access_token = string_field(payload, "access_token")
        .ok_or_else(|| eyre!("refresh response is missing a nonempty access token"))?;
    auth.access_token = access_token;
    if let Some(id_token) = string_field(payload, "id_token") {
        auth.id_token = Some(id_token);
    }
    if let Some(refresh_token) = string_field(payload, "refresh_token") {
        auth.refresh_token = Some(refresh_token);
    }
    auth.expires_at = payload
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|seconds| *seconds >= 0)
        .and_then(|seconds| seconds.checked_mul(1_000))
        .and_then(|duration| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
            i64::try_from(now.as_millis()).ok()?.checked_add(duration)
        });
    auth.enrich_from_tokens(&Provider::Codex);
    Ok(())
}

pub(crate) fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)?
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

fn parse_jwt(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let header = parts.next()?;
    let payload = parts.next()?;
    let signature = parts.next()?;
    if header.is_empty() || payload.is_empty() || signature.is_empty() || parts.next().is_some() {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    claims.is_object().then_some(claims)
}

#[cfg(test)]
mod tests {
    use super::{apply_refresh_payload, codex_auth_from_value, AuthRecord};
    use crate::dashboard::{CredentialKind, Provider};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use eyre::Result;
    use serde_json::json;

    #[test]
    fn api_keys_are_not_oauth_even_with_stale_tokens() -> Result<()> {
        let mut auth = codex_auth_from_value(&json!({
            "auth_mode":"apikey", "OPENAI_API_KEY":"synthetic-key",
            "tokens":{"access_token":"stale-oauth"}
        }))?;
        assert_eq!(auth.kind, CredentialKind::ApiKey);
        assert_eq!(auth.access_token, "synthetic-key");
        assert!(apply_refresh_payload(&mut auth, &json!({"access_token":"new"})).is_err());
        Ok(())
    }

    #[test]
    fn codex_inferred_mode_gives_stored_key_precedence_over_stale_oauth() -> Result<()> {
        let auth = codex_auth_from_value(&json!({
            "OPENAI_API_KEY":"active-key", "tokens":{"access_token":"stale-oauth"}
        }))?;
        assert_eq!(auth.kind, CredentialKind::ApiKey);
        assert_eq!(auth.access_token, "active-key");
        assert!(codex_auth_from_value(&json!({
            "personal_access_token":"unsupported", "OPENAI_API_KEY":"stale-key"
        }))
        .is_err());
        Ok(())
    }

    #[test]
    fn refresh_keeps_omitted_optional_tokens_and_tracks_expiry() -> Result<()> {
        let mut auth = AuthRecord::new("old".to_owned(), CredentialKind::OAuth);
        auth.id_token = Some("old-id".to_owned());
        auth.refresh_token = Some("old-refresh".to_owned());
        let prior = auth.clone();
        apply_refresh_payload(&mut auth, &json!({"access_token":"new", "expires_in":3600}))?;
        assert_eq!(auth.access_token, "new");
        assert_eq!(auth.id_token.as_deref(), Some("old-id"));
        assert_eq!(auth.refresh_token.as_deref(), Some("old-refresh"));
        assert!(auth.expires_at.is_some());
        assert!(auth.needs_persisted_refresh(&prior));
        Ok(())
    }

    #[test]
    fn jwt_person_identity_remains_distinct_from_workspace() {
        let claims = json!({"sub":"person", "email":"fixture@example.invalid", "exp":1234,
            "https://api.openai.com/auth":{"chatgpt_account_id":"workspace", "chatgpt_user_id":"person-id"}});
        let token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let mut auth = AuthRecord::new(token, CredentialKind::OAuth);
        auth.enrich_from_tokens(&Provider::Codex);
        assert_eq!(auth.account_id.as_deref(), Some("workspace"));
        assert_eq!(auth.org_id.as_deref(), Some("workspace"));
        assert_eq!(auth.subject.as_deref(), Some("person-id"));
        assert_eq!(auth.email.as_deref(), Some("fixture@example.invalid"));
        assert_eq!(auth.expires_at, Some(1_234_000));
    }
}
