//! Host-scoped HTTP token auth (lore.md §1.6) — the SINGLE owner API for the
//! `auth.token.*` namespace in the GLOBAL config store.
//!
//! Tokens are AES-256-GCM-encrypted with the global vault unseal key
//! (`~/.libra/vault-unseal-key`, created 0600) and stored as hex ciphertext
//! in `~/.libra/config.db` — the row's sanctioned "文件 fallback 加密"; a
//! real OS keyring is the 2.7 follow-up and swaps in behind this module
//! boundary. The plaintext token NEVER appears in logs, errors, JSON, or
//! status output; errors name only host/port.
//!
//! TRUST BOUNDARY (the lore row's client-side contract, STORED tokens only —
//! the interactive 401 prompt remains a process-global fallback): a stored
//! token is attached ONLY to requests whose normalized host:port scope
//! matches, over https (or http to a loopback host, for local dev remotes —
//! note a token stored without an explicit port normalizes to 443 and will
//! NOT match `http://localhost:80`; log in with the explicit port for
//! non-443 loopback remotes). Cross-host requests never see it.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    command::config::{ConfigScope, ScopedConfig},
    internal::{config::ConfigKv, vault},
};

/// Namespace prefix in the global config store (locked away from `libra
/// config get/set/list/unset` — this module is the only surface).
pub const AUTH_TOKEN_PREFIX: &str = "auth.token.";

/// A normalized host scope: lowercase host + effective port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostScope {
    pub host: String,
    pub port: u16,
}

impl HostScope {
    /// Parse a user-supplied host argument: bare `host`, `host:port`, or a
    /// full `https://host[:port]` URL. A bare form gets `https://` prepended
    /// BEFORE parsing (Url::parse("host:8443") would read `host` as a
    /// scheme). Requires https (or http to a loopback host); refuses
    /// userinfo, paths, queries, and fragments.
    pub fn parse(input: &str) -> Result<HostScope> {
        let text = input.trim();
        if text.is_empty() {
            bail!("host must not be empty");
        }
        let with_scheme = if text.contains("://") {
            text.to_string()
        } else {
            format!("https://{text}")
        };
        let url = url::Url::parse(&with_scheme)
            .map_err(|error| anyhow!("cannot parse host '{text}': {error}"))?;
        Self::from_url(&url)
    }

    /// Scope of a request URL (returns None for non-token-eligible schemes).
    pub fn from_request_url(url: &url::Url) -> Option<HostScope> {
        Self::from_url(url).ok()
    }

    fn from_url(url: &url::Url) -> Result<HostScope> {
        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("host is missing"))?
            .to_ascii_lowercase();
        let loopback = host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        match url.scheme() {
            "https" => {}
            "http" if loopback => {}
            other => bail!("scheme '{other}' is not supported (https only; http for loopback)"),
        }
        if !url.username().is_empty() || url.password().is_some() {
            bail!("host must not carry credentials");
        }
        if url.path() != "/" && !url.path().is_empty() {
            bail!("host must not carry a path");
        }
        if url.query().is_some() || url.fragment().is_some() {
            bail!("host must not carry a query or fragment");
        }
        let port = url
            .port_or_known_default()
            .ok_or_else(|| anyhow!("cannot determine the port"))?;
        Ok(HostScope { host, port })
    }

    pub fn display(&self) -> String {
        if self.port == 443 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    fn storage_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"libra-auth-v1\0https\0");
        hasher.update(self.host.as_bytes());
        hasher.update(b"\0");
        hasher.update(self.port.to_string().as_bytes());
        format!("{AUTH_TOKEN_PREFIX}{}", hex::encode(hasher.finalize()))
    }
}

/// The encrypted-at-rest record (entirely inside the ciphertext).
#[derive(Debug, Serialize, Deserialize)]
struct StoredAuthToken {
    version: u32,
    host: String,
    port: u16,
    username: String,
    token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
    created_at: u64,
}

/// A non-secret status row (`auth status` / list — no token field exists).
#[derive(Debug, Clone, Serialize)]
pub struct TokenStatus {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// `valid` / `expired` / `undecryptable`.
    pub state: String,
}

impl TokenStatus {
    pub fn host_display(&self) -> String {
        if self.port == 443 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// Outcome of a read on the network hot path.
#[derive(Debug)]
pub enum Lookup {
    Miss,
    /// Ciphertext exists but the unseal key cannot open it (key rotated).
    Undecryptable,
    Expired {
        expires_at: u64,
    },
    Valid {
        username: String,
        token: String,
    },
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// chmod-0600 repair for the secret-bearing global files (Unix; Windows
/// relies on per-user profile ACLs — the service-token precedent).
fn repair_global_modes() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [
            ConfigScope::Global.get_config_path(),
            dirs::home_dir().map(|home| home.join(".libra").join("vault-unseal-key")),
        ]
        .into_iter()
        .flatten()
        {
            if path.exists() {
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}

/// Store (upsert) a token for a scope. `expires_at` is unix seconds.
pub async fn store_token(
    scope: &HostScope,
    username: &str,
    token: &str,
    expires_at: Option<u64>,
) -> Result<()> {
    let unseal_key = vault::lazy_init_vault_for_scope("global")
        .await
        .map_err(|_| anyhow!("failed to initialize the global vault key"))?;
    let record = StoredAuthToken {
        version: 1,
        host: scope.host.clone(),
        port: scope.port,
        username: username.to_string(),
        token: token.to_string(),
        expires_at,
        created_at: now_unix(),
    };
    let plaintext = serde_json::to_vec(&record).context("failed to serialize the auth record")?;
    let encrypted = vault::encrypt_token(&unseal_key, &plaintext)
        // The vault error never contains the secret.
        .map_err(|_| anyhow!("failed to encrypt the token"))?;
    let conn = ScopedConfig::get_connection(ConfigScope::Global)
        .await
        .map_err(|error| anyhow!("failed to open the global config store: {error}"))?;
    // Pre-encrypted hex with secret=false — the credential.rs precedent (the
    // config `encrypted` flag drives the vault door, which this bypasses).
    ConfigKv::set_with_conn(&conn, &scope.storage_key(), &hex::encode(encrypted), false)
        .await
        .map_err(|error| anyhow!("failed to persist the token record: {error}"))?;
    repair_global_modes();
    Ok(())
}

async fn read_record(scope: &HostScope) -> Result<Option<Result<StoredAuthToken, ()>>> {
    let conn = ScopedConfig::get_connection(ConfigScope::Global)
        .await
        .map_err(|error| anyhow!("failed to open the global config store: {error}"))?;
    let entry = ConfigKv::get_with_conn(&conn, &scope.storage_key())
        .await
        .map_err(|error| anyhow!("failed to read the token record: {error}"))?;
    let Some(entry) = entry else {
        return Ok(None);
    };
    Ok(Some(decrypt_record(&entry.value).await))
}

async fn decrypt_record(cipher_hex: &str) -> Result<StoredAuthToken, ()> {
    let Ok(cipher) = hex::decode(cipher_hex) else {
        return Err(());
    };
    let Ok(unseal_key) = vault::lazy_init_vault_for_scope("global").await else {
        return Err(());
    };
    let Ok(plaintext) = vault::decrypt_token(&unseal_key, &cipher) else {
        return Err(());
    };
    serde_json::from_str(&plaintext).map_err(|_| ())
}

/// The network-hot-path read: never errors loudly (a broken store must not
/// take down an unauthenticated clone) — degrades to `Miss`.
pub async fn lookup(scope: &HostScope) -> Lookup {
    match read_record(scope).await {
        Ok(None) => Lookup::Miss,
        Ok(Some(Err(()))) => Lookup::Undecryptable,
        Ok(Some(Ok(record))) => match record.expires_at {
            Some(expires_at) if expires_at <= now_unix() => Lookup::Expired { expires_at },
            _ => Lookup::Valid {
                username: record.username,
                token: record.token,
            },
        },
        Err(_) => Lookup::Miss,
    }
}

/// Remove one scope's token (idempotent; works without decryption so revoke
/// survives key rotation). Returns whether something was removed.
pub async fn remove(scope: &HostScope) -> Result<bool> {
    let conn = ScopedConfig::get_connection(ConfigScope::Global)
        .await
        .map_err(|error| anyhow!("failed to open the global config store: {error}"))?;
    let rows = ConfigKv::unset_all_with_conn(&conn, &scope.storage_key())
        .await
        .map_err(|error| anyhow!("failed to remove the token record: {error}"))?;
    Ok(rows > 0)
}

/// Remove every stored token. Returns the count.
pub async fn remove_all() -> Result<usize> {
    let conn = ScopedConfig::get_connection(ConfigScope::Global)
        .await
        .map_err(|error| anyhow!("failed to open the global config store: {error}"))?;
    let entries = ConfigKv::list_all_with_conn(&conn)
        .await
        .map_err(|error| anyhow!("failed to list the global config store: {error}"))?;
    let mut removed = 0usize;
    for entry in entries {
        if entry.key.starts_with(AUTH_TOKEN_PREFIX) {
            removed += ConfigKv::unset_all_with_conn(&conn, &entry.key)
                .await
                .map_err(|error| anyhow!("failed to remove a token record: {error}"))?
                as usize;
        }
    }
    Ok(removed)
}

/// Status rows for every stored token (undecryptable entries reported, not
/// hidden). No token material is ever included.
pub async fn list() -> Result<Vec<TokenStatus>> {
    let conn = ScopedConfig::get_connection(ConfigScope::Global)
        .await
        .map_err(|error| anyhow!("failed to open the global config store: {error}"))?;
    let entries = ConfigKv::list_all_with_conn(&conn)
        .await
        .map_err(|error| anyhow!("failed to list the global config store: {error}"))?;
    let now = now_unix();
    let mut rows = Vec::new();
    for entry in entries {
        if !entry.key.starts_with(AUTH_TOKEN_PREFIX) {
            continue;
        }
        match decrypt_record(&entry.value).await {
            Ok(record) => {
                let state = match record.expires_at {
                    Some(expires_at) if expires_at <= now => "expired",
                    _ => "valid",
                };
                rows.push(TokenStatus {
                    host: record.host,
                    port: record.port,
                    username: record.username,
                    created_at: record.created_at,
                    expires_at: record.expires_at,
                    state: state.to_string(),
                });
            }
            Err(()) => rows.push(TokenStatus {
                host: "<undecryptable>".to_string(),
                port: 0,
                username: String::new(),
                created_at: 0,
                expires_at: None,
                state: "undecryptable".to_string(),
            }),
        }
    }
    rows.sort_by(|a, b| a.host.cmp(&b.host).then(a.port.cmp(&b.port)));
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_scope_parse_matrix() {
        // Bare host, host:port, and full URL all normalize.
        assert_eq!(
            HostScope::parse("Git.Example.COM").unwrap(),
            HostScope {
                host: "git.example.com".to_string(),
                port: 443
            }
        );
        assert_eq!(HostScope::parse("git.example.com:8443").unwrap().port, 8443);
        assert_eq!(
            HostScope::parse("https://git.example.com:8443/")
                .unwrap()
                .port,
            8443
        );
        // Loopback http is allowed; non-loopback http refused.
        assert!(HostScope::parse("http://localhost:8000").is_ok());
        assert!(HostScope::parse("http://127.0.0.1:8000").is_ok());
        assert!(HostScope::parse("http://git.example.com").is_err());
        // Junk refused.
        for bad in [
            "",
            "https://user:pw@host",
            "https://host/path/repo",
            "https://host?q=1",
            "ssh://host",
        ] {
            assert!(HostScope::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn display_elides_default_port() {
        assert_eq!(
            HostScope::parse("h.example").unwrap().display(),
            "h.example"
        );
        assert_eq!(
            HostScope::parse("h.example:8443").unwrap().display(),
            "h.example:8443"
        );
    }
}
