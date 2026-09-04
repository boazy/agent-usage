use crate::dashboard::Provider;
use eyre::{eyre, Result, WrapErr};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceKind {
    Codex,
    Omp,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceConfig {
    pub(crate) name: String,
    pub(crate) kind: SourceKind,
    pub(crate) path: PathBuf,
    #[serde(default = "enabled_by_default")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) optional: bool,
}

const fn enabled_by_default() -> bool { true }

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Exclusions {
    pub(crate) sources: Vec<String>,
    pub(crate) providers: Vec<String>,
    pub(crate) emails: Vec<String>,
    pub(crate) accounts: Vec<AccountExclusion>,
    pub(crate) account_ids: Vec<String>,
    /// Match only the displayed mask: literal ASCII prefix and suffix around one '*'.
    pub(crate) api_keys: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountExclusion {
    pub(crate) email: String,
    pub(crate) provider: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DashboardConfig {
    #[serde(default = "default_sources")]
    pub(crate) sources: Vec<SourceConfig>,
    pub(crate) exclude: Exclusions,
    pub(crate) concurrency: usize,
    pub(crate) timeout_seconds: u64,
    pub(crate) refresh_seconds: u64,
    pub(crate) theme: String,
    pub(crate) codex_base_url: String,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            exclude: Exclusions::default(),
            concurrency: 4,
            timeout_seconds: 20,
            refresh_seconds: 60,
            theme: "default".to_owned(),
            codex_base_url: "https://chatgpt.com/backend-api".to_owned(),
        }
    }
}

impl DashboardConfig {
    pub(crate) fn load(path: Option<&Path>) -> Result<Self> {
        if path.is_some_and(|path| !path.is_file()) {
            return Err(eyre!("explicit configuration file is missing or is not a regular file"));
        }
        let path = path.map_or_else(default_config_path, |path| Some(path.to_path_buf()));
        let mut builder = ::config::Config::builder();
        if let Some(path) = path.filter(|path| path.exists()) {
            builder = builder.add_source(::config::File::from(path));
        }
        builder = builder.add_source(::config::Environment::with_prefix("AGENT_USAGE")
            .prefix_separator("_").separator("__").try_parsing(true));
        let result: Self = builder.build().wrap_err("failed to load dashboard configuration")?
            .try_deserialize().wrap_err("invalid dashboard configuration")?;
        result.validate()?;
        Ok(result)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if !(1..=32).contains(&self.concurrency) {
            return Err(eyre!("concurrency must be between 1 and 32"));
        }
        if !(1..=300).contains(&self.timeout_seconds) {
            return Err(eyre!("timeout_seconds must be between 1 and 300"));
        }
        if self.refresh_seconds == 0 {
            return Err(eyre!("refresh_seconds must be at least 1"));
        }
        let mut names = HashSet::new();
        for source in &self.sources {
            if source.name.trim().is_empty() || !names.insert(&source.name) {
                return Err(eyre!("source names must be nonempty and unique"));
            }
        }
        for rule in &self.exclude.api_keys {
            validate_key_pattern(rule)?;
        }
        for rule in &self.exclude.accounts {
            if rule.email.trim().is_empty() || rule.provider.trim().is_empty() {
                return Err(eyre!("account exclusions require both email and provider"));
            }
        }
        Ok(())
    }
}

pub(crate) fn default_config_path() -> Option<PathBuf> {
    platform_dirs::AppDirs::new(Some("agent-usage"), true)
        .map(|directories| directories.config_dir.join("config.toml"))
}

pub(crate) fn default_sources() -> Vec<SourceConfig> {
    let codex = std::env::var_os("CODEX_HOME").map_or_else(
        || PathBuf::from("~/.codex/auth.json"),
        |home| PathBuf::from(home).join("auth.json"),
    );
    let directory = default_omp_directory();
    let omp = omp_credential_path(&directory);
    vec![
        SourceConfig { name: "codex".to_owned(), kind: SourceKind::Codex, path: codex, enabled: true, optional: true },
        SourceConfig { name: "omp".to_owned(), kind: SourceKind::Omp, path: omp, enabled: true, optional: true },
    ]
}

fn omp_credential_path(directory: &Path) -> PathBuf {
    let database = directory.join("agent.db");
    // Current OMP stores credentials exclusively in SQLite. A leftover legacy
    // file must never resurrect accounts deleted or rotated in that database.
    if database.exists() { database } else { directory.join("auth.json") }
}

fn default_omp_directory() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("~"), PathBuf::from);
    let root_name = std::env::var_os("PI_CONFIG_DIR").filter(|value| !value.is_empty())
        .unwrap_or_else(|| ".omp".into());
    let mut root = home.join(root_name);
    let profile = std::env::var("OMP_PROFILE").or_else(|_| std::env::var("PI_PROFILE"))
        .ok().filter(|profile| !profile.trim().is_empty() && profile.trim() != "default");
    if let Some(profile) = &profile {
        // Invalid profiles are rejected by OMP itself. Do not interpret them as paths.
        if !profile.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            || matches!(profile.as_str(), "." | "..") {
            return root.join("agent");
        }
        root = root.join("profiles").join(profile);
    } else if let Some(directory) = std::env::var_os("PI_CODING_AGENT_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(directory);
    }
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        if let Some(directory) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
            let mut xdg = PathBuf::from(directory).join("omp");
            if let Some(profile) = profile { xdg = xdg.join("profiles").join(profile); }
            if xdg.exists() { return xdg; }
        }
    }
    root.join("agent")
}

pub(crate) fn validate_key_pattern(pattern: &str) -> Result<()> {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return Err(eyre!("API-key exclusion must contain one '*' between a visible prefix and suffix"));
    };
    if suffix.contains('*') || prefix.len() > 4 || suffix.len() > 4
        || prefix.len() + suffix.len() < 4 || !pattern.is_ascii()
        || !prefix.bytes().chain(suffix.bytes()).all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) {
        return Err(eyre!("API-key exclusion must use 4–8 visible ASCII characters, at most four on each side of one '*'"));
    }
    Ok(())
}

pub(crate) fn provider_matches(value: &str, provider: &Provider) -> bool {
    Provider::from_id(value) == *provider
}

#[cfg(test)]
mod tests {
    use super::{omp_credential_path, validate_key_pattern, DashboardConfig};
    use eyre::Result;
    use std::collections::HashMap;

    #[test]
    fn explicit_missing_config_never_discovers_default_credentials() -> Result<()> {
        let directory = tempfile::tempdir()?;
        assert!(DashboardConfig::load(Some(&directory.path().join("missing.toml"))).is_err());
        Ok(())
    }

    #[test]
    fn explicit_empty_source_list_disables_default_discovery() -> Result<()> {
        let config: DashboardConfig = ::config::Config::builder()
            .add_source(::config::File::from_str("sources = []", ::config::FileFormat::Toml))
            .build()?.try_deserialize()?;
        assert!(config.sources.is_empty());
        Ok(())
    }

    #[test]
    fn existing_theme_environment_name_and_nested_keys_work() -> Result<()> {
        let environment = HashMap::from([
            ("AGENT_USAGE_THEME".to_owned(), "monokai".to_owned()),
            ("AGENT_USAGE_CONCURRENCY".to_owned(), "3".to_owned()),
        ]);
        let config: DashboardConfig = ::config::Config::builder()
            .add_source(::config::File::from_str("sources = []", ::config::FileFormat::Toml))
            .add_source(::config::Environment::with_prefix("AGENT_USAGE")
                .prefix_separator("_").separator("__").try_parsing(true).source(Some(environment)))
            .build()?.try_deserialize()?;
        assert_eq!(config.theme, "monokai");
        assert_eq!(config.concurrency, 3);
        Ok(())
    }

    #[test]
    fn api_key_selectors_accept_only_visible_mask_components() {
        assert!(validate_key_pattern("sk-o*abcd").is_ok());
        assert!(validate_key_pattern("*abcd").is_ok());
        assert!(validate_key_pattern("sk-synthetic-complete-secret").is_err());
        assert!(validate_key_pattern("sk-*a").is_ok());
        assert!(validate_key_pattern("sk*").is_err());
        assert!(validate_key_pattern("sk**abcd").is_err());
        assert!(validate_key_pattern("prefix*abcd").is_err());
    }

    #[test]
    fn database_presence_prevents_legacy_account_resurrection() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let legacy = directory.path().join("auth.json");
        std::fs::write(&legacy, "{}")?;
        assert_eq!(omp_credential_path(directory.path()), legacy);
        let database = directory.path().join("agent.db");
        std::fs::write(&database, "synthetic corrupt database")?;
        assert_eq!(omp_credential_path(directory.path()), database);
        Ok(())
    }
}
