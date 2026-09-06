//! Composer-compatible credentials for dist downloads.
//!
//! Composer reads `auth.json` from two places plus the `COMPOSER_AUTH`
//! environment variable, merging them in ascending precedence: composer
//! home, then the project directory, then the environment. See
//! <https://getcomposer.org/doc/articles/http-basic-authentication.md>.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use reqwest::Url;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue};
use serde::Deserialize;

/// Per-host credentials, merged from `auth.json` and `COMPOSER_AUTH`.
///
/// `Debug` only ever lists hosts, never the credential values, so accidental
/// `{:?}` logging can't leak a token.
#[derive(Default)]
pub struct Auth {
    github_oauth: HashMap<String, String>,
    http_basic: HashMap<String, (String, String)>,
    bearer: HashMap<String, String>,
    gitlab_oauth: HashMap<String, String>,
    gitlab_token: HashMap<String, GitlabToken>,
    /// consumer-key, consumer-secret, keyed by host (always `bitbucket.org`
    /// today; Composer only ever reads this key for that host).
    bitbucket_oauth: HashMap<String, (String, String)>,
    /// Bearer token exchanged for a `bitbucket_oauth` entry, or loaded
    /// straight from an unexpired `access-token`/`access-token-expiration`
    /// pair in `auth.json`. Populated lazily by
    /// [`Auth::ensure_bitbucket_token`]; a `Mutex` because `header_for` and
    /// the callers around it only ever hold `&self`.
    bitbucket_tokens: Mutex<HashMap<String, BitbucketToken>>,
    /// `{"custom-headers": {"host": ["Header: value", ...]}}`: raw header
    /// lines appended to every request to that host, alongside whatever
    /// other credential applies.
    custom_headers: HashMap<String, Vec<String>>,
}

/// A cached Bitbucket bearer token and the Unix timestamp (seconds) it
/// expires at, matching the units Composer stores `access-token-expiration`
/// in (`time() + expires_in`, an absolute timestamp, not a duration).
#[derive(Clone)]
struct BitbucketToken {
    access_token: String,
    expires_at: u64,
}

/// A `gitlab-token` entry: either a bare token string, or `{username,
/// token}` where Composer lets the token *type* land in either field (see
/// `GitLab::authorizeOAuth` upstream) — `gitlab_token_header` sorts out
/// which is which.
#[derive(Deserialize)]
#[serde(untagged)]
enum GitlabToken {
    Plain(String),
    UsernameToken { username: String, token: String },
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn hosts<V>(map: &HashMap<String, V>) -> Vec<&String> {
            map.keys().collect()
        }
        let bitbucket_token_hosts: Vec<String> = self
            .bitbucket_tokens
            .lock()
            .map(|tokens| tokens.keys().cloned().collect())
            .unwrap_or_default();
        f.debug_struct("Auth")
            .field("github_oauth_hosts", &hosts(&self.github_oauth))
            .field("http_basic_hosts", &hosts(&self.http_basic))
            .field("bearer_hosts", &hosts(&self.bearer))
            .field("gitlab_oauth_hosts", &hosts(&self.gitlab_oauth))
            .field("gitlab_token_hosts", &hosts(&self.gitlab_token))
            .field("bitbucket_oauth_hosts", &hosts(&self.bitbucket_oauth))
            .field("bitbucket_token_hosts", &bitbucket_token_hosts)
            .field("custom_headers_hosts", &hosts(&self.custom_headers))
            .finish()
    }
}

/// `auth.json`'s shape.
#[derive(Deserialize, Default)]
struct RawAuth {
    #[serde(rename = "github-oauth", default)]
    github_oauth: HashMap<String, String>,
    #[serde(rename = "http-basic", default)]
    http_basic: HashMap<String, RawBasic>,
    #[serde(default)]
    bearer: HashMap<String, String>,
    #[serde(rename = "gitlab-oauth", default)]
    gitlab_oauth: HashMap<String, String>,
    #[serde(rename = "gitlab-token", default)]
    gitlab_token: HashMap<String, GitlabToken>,
    #[serde(rename = "bitbucket-oauth", default)]
    bitbucket_oauth: HashMap<String, RawBitbucketOauth>,
    #[serde(rename = "custom-headers", default)]
    custom_headers: HashMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct RawBasic {
    username: String,
    password: String,
}

/// Composer stores `access-token`/`access-token-expiration` here once it has
/// exchanged the consumer key/secret for a token; both are optional so an
/// entry with only the key/secret pair still loads.
#[derive(Deserialize)]
struct RawBitbucketOauth {
    #[serde(rename = "consumer-key")]
    consumer_key: String,
    #[serde(rename = "consumer-secret")]
    consumer_secret: String,
    #[serde(rename = "access-token", default)]
    access_token: Option<String>,
    #[serde(rename = "access-token-expiration", default)]
    access_token_expiration: Option<u64>,
}

impl Auth {
    /// Load and merge composer home's `auth.json`, `project_dir`'s
    /// `auth.json`, then `COMPOSER_AUTH`, lowest to highest precedence.
    /// A missing file is fine; malformed JSON is an error naming the file.
    pub fn load(project_dir: &Path) -> Result<Auth> {
        let mut auth = Auth::default();
        if let Some(home) = composer_home() {
            auth.merge_file(&home.join("auth.json"))?;
        }
        auth.merge_file(&project_dir.join("auth.json"))?;
        if let Ok(raw) = std::env::var("COMPOSER_AUTH") {
            let parsed: RawAuth =
                serde_json::from_str(&raw).context("COMPOSER_AUTH is not valid JSON")?;
            auth.merge(parsed);
        }
        Ok(auth)
    }

    fn merge_file(&mut self, path: &Path) -> Result<()> {
        let Ok(content) = fs_err::read_to_string(path) else {
            return Ok(());
        };
        let parsed: RawAuth = serde_json::from_str(&content)
            .with_context(|| format!("{}: not valid JSON", path.display()))?;
        self.merge(parsed);
        Ok(())
    }

    fn merge(&mut self, raw: RawAuth) {
        self.github_oauth.extend(raw.github_oauth);
        self.http_basic.extend(
            raw.http_basic
                .into_iter()
                .map(|(host, basic)| (host, (basic.username, basic.password))),
        );
        self.bearer.extend(raw.bearer);
        self.gitlab_oauth.extend(raw.gitlab_oauth);
        self.gitlab_token.extend(raw.gitlab_token);
        self.custom_headers.extend(raw.custom_headers);
        if let Ok(mut tokens) = self.bitbucket_tokens.lock() {
            for (host, b) in &raw.bitbucket_oauth {
                if let (Some(access_token), Some(expires_at)) =
                    (&b.access_token, b.access_token_expiration)
                    && token_is_fresh(expires_at, now_unix())
                {
                    tokens.insert(
                        host.clone(),
                        BitbucketToken {
                            access_token: access_token.clone(),
                            expires_at,
                        },
                    );
                }
            }
        }
        self.bitbucket_oauth.extend(
            raw.bitbucket_oauth
                .into_iter()
                .map(|(host, b)| (host, (b.consumer_key, b.consumer_secret))),
        );
    }

    /// The `Authorization` header to send for `url`'s host, if any
    /// credential matches it. GitHub token wins over http-basic for the
    /// same host; a `github-oauth` entry for `github.com` also covers
    /// `api.github.com` and `codeload.github.com`, matching Composer.
    pub fn header_for(&self, url: &Url) -> Option<(HeaderName, HeaderValue)> {
        let host = url.host_str()?.to_ascii_lowercase();
        if let Some(token) = self.github_token_for_host(&host) {
            return Some((
                AUTHORIZATION,
                HeaderValue::from_str(&format!("token {token}")).ok()?,
            ));
        }
        if let Some(token) = self.gitlab_oauth.get(&host) {
            return Some((
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).ok()?,
            ));
        }
        if let Some(cred) = self.gitlab_token.get(&host) {
            let (name, value) = gitlab_token_header(cred);
            return Some((name, HeaderValue::from_str(&value).ok()?));
        }
        if let Some(canonical) = bitbucket_canonical_host(&host)
            && self.bitbucket_oauth.contains_key(canonical)
        {
            // Composer never sends its bitbucket-oauth bearer token to the
            // token endpoint itself, nor to a public `/downloads/` URL (those
            // are served from S3 and reject the extra header); see
            // `AuthHelper::addAuthenticationOptions` and
            // `isPublicBitBucketDownload` upstream.
            if url.as_str() == BITBUCKET_TOKEN_URL || is_public_bitbucket_download(url) {
                return None;
            }
            let token = self
                .bitbucket_tokens
                .lock()
                .ok()
                .and_then(|tokens| tokens.get(canonical).cloned())
                .filter(|t| token_is_fresh(t.expires_at, now_unix()))
                .map(|t| t.access_token);
            // `None` here (no cached token yet, e.g. the exchange hasn't run
            // or failed) is deliberate: `credential_hint` still fires on the
            // resulting 401/403 instead of silently sending nothing labelled.
            return Some((
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", token?)).ok()?,
            ));
        }
        if let Some((user, pass)) = self.http_basic.get(&host) {
            let encoded = base64_encode(format!("{user}:{pass}").as_bytes());
            return Some((
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Basic {encoded}")).ok()?,
            ));
        }
        if let Some(token) = self.bearer.get(&host) {
            return Some((
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).ok()?,
            ));
        }
        None
    }

    fn github_token_for_host(&self, host: &str) -> Option<&str> {
        if let Some(token) = self.github_oauth.get(host) {
            return Some(token);
        }
        if matches!(host, "api.github.com" | "codeload.github.com") {
            return self.github_oauth.get("github.com").map(String::as_str);
        }
        None
    }

    /// The `custom-headers` lines configured for `url`'s host, parsed into
    /// header name/value pairs and appended alongside whatever `header_for`
    /// sends. A malformed line (no `:`) is skipped with a debug log rather
    /// than failing the whole fetch over one bad entry.
    pub fn custom_headers_for(&self, url: &Url) -> Vec<(HeaderName, HeaderValue)> {
        let Some(host) = url.host_str() else {
            return Vec::new();
        };
        let Some(lines) = self.custom_headers.get(&host.to_ascii_lowercase()) else {
            return Vec::new();
        };
        lines
            .iter()
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                let name = HeaderName::from_bytes(name.trim().as_bytes()).ok()?;
                let value = HeaderValue::from_str(value.trim()).ok()?;
                Some((name, value))
            })
            .collect()
    }

    /// Make sure a fresh Bitbucket bearer token is cached for `host`'s
    /// canonical `bitbucket.org` entry, exchanging the consumer key/secret at
    /// [`BITBUCKET_TOKEN_URL`] if the cache is empty or expired. A no-op when
    /// `host` has no `bitbucket-oauth` entry, or the cache is already fresh
    /// (an unexpired `access-token` from `auth.json`, or an earlier
    /// exchange). Errors are the caller's to decide whether to surface;
    /// `header_for` falls back to sending nothing when the cache stays
    /// empty, matching Composer's behaviour on an invalid consumer.
    pub async fn ensure_bitbucket_token(&self, host: &str) -> Result<()> {
        let Some(canonical) = bitbucket_canonical_host(host) else {
            return Ok(());
        };
        let Some((key, secret)) = self.bitbucket_oauth.get(canonical) else {
            return Ok(());
        };
        let fresh = self
            .bitbucket_tokens
            .lock()
            .ok()
            .and_then(|tokens| tokens.get(canonical).cloned())
            .is_some_and(|t| token_is_fresh(t.expires_at, now_unix()));
        if fresh {
            return Ok(());
        }
        let (access_token, expires_in) = exchange_bitbucket_token(key, secret).await?;
        let token = BitbucketToken {
            access_token,
            expires_at: now_unix() + expires_in,
        };
        if let Ok(mut tokens) = self.bitbucket_tokens.lock() {
            tokens.insert(canonical.to_string(), token);
        }
        Ok(())
    }
}

/// `Composer\Util\Bitbucket::OAUTH2_ACCESS_TOKEN_URL`.
const BITBUCKET_TOKEN_URL: &str = "https://bitbucket.org/site/oauth2/access_token";

/// `bitbucket.org` and `api.bitbucket.org` share one `bitbucket-oauth` entry
/// (keyed by `bitbucket.org`), matching `AuthHelper::findAuthOrigin` upstream.
fn bitbucket_canonical_host(host: &str) -> Option<&'static str> {
    matches!(host, "bitbucket.org" | "api.bitbucket.org").then_some("bitbucket.org")
}

/// `AuthHelper::isPublicBitBucketDownload`: a `/{user}/{repo}/downloads/...`
/// path on `bitbucket.org` is a public download (served from S3, which
/// rejects an unexpected `Authorization` header); anything else needs the
/// bearer token.
fn is_public_bitbucket_download(url: &Url) -> bool {
    url.path().split('/').nth(3) == Some("downloads")
}

/// Whether a cached token is still usable: Composer's own check is
/// `time() > expiration` for *expired*, i.e. valid while `now <= expiration`.
fn token_is_fresh(expires_at: u64, now: u64) -> bool {
    now <= expires_at
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// POST `consumer_key`/`consumer_secret` (HTTP Basic) to
/// [`BITBUCKET_TOKEN_URL`] and return `(access_token, expires_in)`, mirroring
/// `Bitbucket::requestAccessToken` upstream.
async fn exchange_bitbucket_token(
    consumer_key: &str,
    consumer_secret: &str,
) -> Result<(String, u64)> {
    let response = reqwest::Client::new()
        .post(BITBUCKET_TOKEN_URL)
        .basic_auth(consumer_key, Some(consumer_secret))
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body("grant_type=client_credentials")
        .send()
        .await
        .context("exchanging bitbucket-oauth consumer key/secret for an access token")?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .context("reading the bitbucket OAuth token response")?;
    if !status.is_success() {
        bail!(
            "bitbucket OAuth token exchange failed with HTTP {}; check the consumer-key/consumer-secret \
             in auth.json (a private OAuth consumer needs a callback URL configured)",
            status.as_u16()
        );
    }
    parse_bitbucket_token_response(&body)
}

/// Parses `{"access_token": "...", "expires_in": ...}`, isolated from the
/// network so it's testable with a literal response body.
fn parse_bitbucket_token_response(body: &[u8]) -> Result<(String, u64)> {
    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        expires_in: u64,
    }
    let parsed: TokenResponse = serde_json::from_slice(body)
        .context("bitbucket OAuth token response was not the expected JSON shape")?;
    Ok((parsed.access_token, parsed.expires_in))
}

/// The header name and value Composer's `AuthHelper::addAuthenticationOptions`
/// sends for a `gitlab-token` credential. A plain string is a personal
/// access token: `PRIVATE-TOKEN: <token>`. A `{username, token}` object is
/// only special-cased when `username` holds a *type* marker Composer
/// recognises (`oauth2` sends the actual token as a Bearer, `private-token`
/// and `gitlab-ci-token` send it via `PRIVATE-TOKEN`); anything else is a
/// genuine username, and falls back to HTTP Basic like Composer does.
fn gitlab_token_header(cred: &GitlabToken) -> (HeaderName, String) {
    match cred {
        GitlabToken::Plain(token) => (private_token_header(), token.clone()),
        GitlabToken::UsernameToken { username, token } => match username.as_str() {
            "oauth2" => (AUTHORIZATION, format!("Bearer {token}")),
            "private-token" | "gitlab-ci-token" => (private_token_header(), token.clone()),
            _ => (
                AUTHORIZATION,
                format!(
                    "Basic {}",
                    base64_encode(format!("{username}:{token}").as_bytes())
                ),
            ),
        },
    }
}

fn private_token_header() -> HeaderName {
    HeaderName::from_static("private-token")
}

/// Composer home on Linux: `$COMPOSER_HOME` if set, else `~/.composer` if
/// that directory already exists, else `${XDG_CONFIG_HOME:-~/.config}/composer`.
fn composer_home() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("COMPOSER_HOME")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var("HOME").ok()?;
    let legacy = PathBuf::from(&home).join(".composer");
    if legacy.is_dir() {
        return Some(legacy);
    }
    let xdg = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{home}/.config"));
    Some(PathBuf::from(xdg).join("composer"))
}

/// Minimal base64 (RFC 4648, standard alphabet, `=` padding) encoder for
/// `http-basic`'s `user:pass`; a dependency would do nothing this doesn't.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Serialises env-touching tests (this module's tests run in parallel
    /// otherwise) and restores every var it changed on drop.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        #[allow(
            unsafe_code,
            reason = "serialised by ENV_LOCK for the guard's lifetime"
        )]
        fn set(vars: &[(&'static str, Option<&str>)]) -> EnvGuard {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let saved = vars
                .iter()
                .map(|(k, _)| (*k, std::env::var(k).ok()))
                .collect();
            // SAFETY: serialised by ENV_LOCK for the guard's lifetime; no
            // other thread reads/writes these vars while it's held.
            unsafe {
                for (k, v) in vars {
                    match v {
                        Some(v) => std::env::set_var(k, v),
                        None => std::env::remove_var(k),
                    }
                }
            }
            EnvGuard { _lock: lock, saved }
        }
    }

    impl Drop for EnvGuard {
        #[allow(
            unsafe_code,
            reason = "serialised by ENV_LOCK for the guard's lifetime"
        )]
        fn drop(&mut self) {
            // SAFETY: see EnvGuard::set.
            unsafe {
                for (k, v) in &self.saved {
                    match v {
                        Some(v) => std::env::set_var(k, v),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }

    fn write(dir: &Path, contents: &str) {
        fs_err::write(dir.join("auth.json"), contents).unwrap();
    }

    fn gh(url: &str) -> Url {
        Url::parse(url).unwrap()
    }

    #[test]
    fn missing_files_give_empty_auth() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            ("COMPOSER_AUTH", None),
            ("HOME", Some("/nonexistent-vivace-test-home")),
            ("XDG_CONFIG_HOME", None),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        assert!(
            auth.header_for(&gh("https://github.com/acme/pkg"))
                .is_none()
        );
    }

    #[test]
    fn home_auth_json_alone_is_read() {
        let home_dir = tempfile::tempdir().unwrap();
        write(
            home_dir.path(),
            r#"{"github-oauth": {"github.com": "home-token"}}"#,
        );
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", Some(home_dir.path().to_str().unwrap())),
            ("COMPOSER_AUTH", None),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        let (name, value) = auth.header_for(&gh("https://github.com/acme/pkg")).unwrap();
        assert_eq!(name, AUTHORIZATION);
        assert_eq!(value, "token home-token");
    }

    #[test]
    fn project_auth_json_overrides_home_for_same_host() {
        let home_dir = tempfile::tempdir().unwrap();
        write(
            home_dir.path(),
            r#"{"github-oauth": {"github.com": "home-token"}}"#,
        );
        let project = tempfile::tempdir().unwrap();
        write(
            project.path(),
            r#"{"github-oauth": {"github.com": "project-token"}}"#,
        );
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", Some(home_dir.path().to_str().unwrap())),
            ("COMPOSER_AUTH", None),
        ]);
        let auth = Auth::load(project.path()).unwrap();
        let (_, value) = auth.header_for(&gh("https://github.com/acme/pkg")).unwrap();
        assert_eq!(value, "token project-token");
    }

    #[test]
    fn composer_auth_env_overrides_both_files() {
        let home_dir = tempfile::tempdir().unwrap();
        write(
            home_dir.path(),
            r#"{"github-oauth": {"github.com": "home-token"}}"#,
        );
        let project = tempfile::tempdir().unwrap();
        write(
            project.path(),
            r#"{"github-oauth": {"github.com": "project-token"}}"#,
        );
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", Some(home_dir.path().to_str().unwrap())),
            (
                "COMPOSER_AUTH",
                Some(r#"{"github-oauth": {"github.com": "env-token"}}"#),
            ),
        ]);
        let auth = Auth::load(project.path()).unwrap();
        let (_, value) = auth.header_for(&gh("https://github.com/acme/pkg")).unwrap();
        assert_eq!(value, "token env-token");
    }

    #[test]
    fn github_token_applies_to_api_and_codeload_hosts() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(r#"{"github-oauth": {"github.com": "tok"}}"#),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        for url in [
            "https://github.com/acme/pkg",
            "https://api.github.com/repos/acme/pkg/zipball/abc",
            "https://codeload.github.com/acme/pkg/legacy.zip/abc",
        ] {
            let (name, value) = auth.header_for(&gh(url)).unwrap_or_else(|| panic!("{url}"));
            assert_eq!(name, AUTHORIZATION);
            assert_eq!(value, "token tok");
        }
    }

    #[test]
    fn http_basic_header_value_is_correct() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(
                    r#"{"http-basic": {"example.com": {"username": "user", "password": "pass"}}}"#,
                ),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        let (name, value) = auth.header_for(&gh("https://example.com/pkg.zip")).unwrap();
        assert_eq!(name, AUTHORIZATION);
        // echo -n user:pass | base64
        assert_eq!(value, "Basic dXNlcjpwYXNz");
    }

    #[test]
    fn bearer_header_value_is_correct() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(r#"{"bearer": {"example.com": "my-bearer-token"}}"#),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        let (name, value) = auth.header_for(&gh("https://example.com/pkg.zip")).unwrap();
        assert_eq!(name, AUTHORIZATION);
        assert_eq!(value, "Bearer my-bearer-token");
    }

    #[test]
    fn gitlab_oauth_sends_bearer() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(r#"{"gitlab-oauth": {"gitlab.com": "gl-tok"}}"#),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        let (name, value) = auth.header_for(&gh("https://gitlab.com/acme/pkg")).unwrap();
        assert_eq!(name, AUTHORIZATION);
        assert_eq!(value, "Bearer gl-tok");
    }

    #[test]
    fn gitlab_token_string_sends_private_token() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(r#"{"gitlab-token": {"gitlab.com": "pat-123"}}"#),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        let (name, value) = auth.header_for(&gh("https://gitlab.com/acme/pkg")).unwrap();
        assert_eq!(name, "private-token");
        assert_eq!(value, "pat-123");
    }

    #[test]
    fn gitlab_token_object_with_oauth2_marker_sends_bearer() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(
                    r#"{"gitlab-token": {"gitlab.com": {"username": "oauth2", "token": "gl-tok"}}}"#,
                ),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        let (name, value) = auth.header_for(&gh("https://gitlab.com/acme/pkg")).unwrap();
        assert_eq!(name, AUTHORIZATION);
        assert_eq!(value, "Bearer gl-tok");
    }

    #[test]
    fn gitlab_token_object_with_private_token_marker_sends_private_token() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(
                    r#"{"gitlab-token": {"gitlab.com": {"username": "private-token", "token": "pat-123"}}}"#,
                ),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        let (name, value) = auth.header_for(&gh("https://gitlab.com/acme/pkg")).unwrap();
        assert_eq!(name, "private-token");
        assert_eq!(value, "pat-123");
    }

    #[test]
    fn gitlab_token_object_with_real_username_falls_back_to_basic() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(
                    r#"{"gitlab-token": {"gitlab.com": {"username": "deploy", "token": "secret"}}}"#,
                ),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        let (name, value) = auth.header_for(&gh("https://gitlab.com/acme/pkg")).unwrap();
        assert_eq!(name, AUTHORIZATION);
        // echo -n deploy:secret | base64
        assert_eq!(value, "Basic ZGVwbG95OnNlY3JldA==");
    }

    #[test]
    fn bitbucket_oauth_sends_no_header() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(
                    r#"{"bitbucket-oauth": {"bitbucket.org": {"consumer-key": "k", "consumer-secret": "s"}}}"#,
                ),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        assert!(
            auth.header_for(&gh("https://bitbucket.org/acme/pkg.zip"))
                .is_none()
        );
    }

    #[test]
    fn bitbucket_unexpired_access_token_from_auth_json_sends_bearer() {
        let expiration = now_unix() + 3600;
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(&format!(
                    r#"{{"bitbucket-oauth": {{"bitbucket.org": {{"consumer-key": "k", "consumer-secret": "s", "access-token": "cached-tok", "access-token-expiration": {expiration}}}}}}}"#
                )),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        let (name, value) = auth
            .header_for(&gh("https://bitbucket.org/acme/pkg.zip"))
            .unwrap();
        assert_eq!(name, AUTHORIZATION);
        assert_eq!(value, "Bearer cached-tok");
        // api.bitbucket.org shares the same bitbucket.org entry.
        let (_, value) = auth
            .header_for(&gh("https://api.bitbucket.org/2.0/repositories/acme/pkg"))
            .unwrap();
        assert_eq!(value, "Bearer cached-tok");
    }

    #[test]
    fn bitbucket_expired_access_token_from_auth_json_sends_no_header() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(
                    r#"{"bitbucket-oauth": {"bitbucket.org": {"consumer-key": "k", "consumer-secret": "s", "access-token": "stale-tok", "access-token-expiration": 1}}}"#,
                ),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        assert!(
            auth.header_for(&gh("https://bitbucket.org/acme/pkg.zip"))
                .is_none()
        );
    }

    #[test]
    fn bitbucket_public_download_sends_no_header_even_with_a_fresh_token() {
        let expiration = now_unix() + 3600;
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(&format!(
                    r#"{{"bitbucket-oauth": {{"bitbucket.org": {{"consumer-key": "k", "consumer-secret": "s", "access-token": "cached-tok", "access-token-expiration": {expiration}}}}}}}"#
                )),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        assert!(
            auth.header_for(&gh("https://bitbucket.org/acme/pkg/downloads/pkg.zip"))
                .is_none()
        );
        assert!(
            auth.header_for(&gh(BITBUCKET_TOKEN_URL)).is_none(),
            "the token endpoint itself never gets the bearer token"
        );
    }

    #[test]
    fn bitbucket_canonical_host_matches_api_and_plain_hosts_only() {
        assert_eq!(
            bitbucket_canonical_host("bitbucket.org"),
            Some("bitbucket.org")
        );
        assert_eq!(
            bitbucket_canonical_host("api.bitbucket.org"),
            Some("bitbucket.org")
        );
        assert_eq!(bitbucket_canonical_host("example.com"), None);
    }

    #[test]
    fn is_public_bitbucket_download_matches_composer() {
        assert!(is_public_bitbucket_download(&gh(
            "https://bitbucket.org/acme/pkg/downloads/pkg.zip"
        )));
        assert!(!is_public_bitbucket_download(&gh(
            "https://bitbucket.org/acme/pkg"
        )));
        assert!(!is_public_bitbucket_download(&gh(
            "https://bitbucket.org/acme/pkg/src/main.php"
        )));
    }

    #[test]
    fn token_expiry_boundary() {
        assert!(token_is_fresh(100, 100), "valid through the expiry second");
        assert!(!token_is_fresh(100, 101));
    }

    #[test]
    fn parse_bitbucket_token_response_reads_access_token_and_expiry() {
        let (token, expires_in) =
            parse_bitbucket_token_response(br#"{"access_token": "tok", "expires_in": 7200}"#)
                .unwrap();
        assert_eq!(token, "tok");
        assert_eq!(expires_in, 7200);
    }

    #[test]
    fn parse_bitbucket_token_response_rejects_unexpected_shape() {
        assert!(parse_bitbucket_token_response(br#"{"error": "invalid_client"}"#).is_err());
    }

    #[test]
    fn custom_headers_are_parsed_per_host() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(
                    r#"{"custom-headers": {"example.com": ["X-Auth-Token: secret", "X-Extra: 1"]}}"#,
                ),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        let headers = auth.custom_headers_for(&gh("https://example.com/pkg.zip"));
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].0, "x-auth-token");
        assert_eq!(headers[0].1, "secret");
        assert_eq!(headers[1].0, "x-extra");
        assert_eq!(headers[1].1, "1");
        assert!(
            auth.custom_headers_for(&gh("https://other.example/pkg.zip"))
                .is_empty()
        );
    }

    #[test]
    fn custom_headers_skips_a_malformed_line() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(r#"{"custom-headers": {"example.com": ["no-colon-here", "X-Ok: yes"]}}"#),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        let headers = auth.custom_headers_for(&gh("https://example.com/pkg.zip"));
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, "x-ok");
        assert_eq!(headers[0].1, "yes");
    }

    #[test]
    fn unknown_host_yields_none() {
        let _env = EnvGuard::set(&[
            ("COMPOSER_HOME", None),
            (
                "COMPOSER_AUTH",
                Some(r#"{"github-oauth": {"github.com": "tok"}}"#),
            ),
        ]);
        let project = tempfile::tempdir().unwrap();
        let auth = Auth::load(project.path()).unwrap();
        assert!(
            auth.header_for(&gh("https://example.com/pkg.zip"))
                .is_none()
        );
    }

    #[test]
    fn malformed_json_error_names_the_path() {
        let _env = EnvGuard::set(&[("COMPOSER_HOME", None), ("COMPOSER_AUTH", None)]);
        let project = tempfile::tempdir().unwrap();
        write(project.path(), "not json");
        let err = Auth::load(project.path()).unwrap_err().to_string();
        assert!(
            err.contains(project.path().join("auth.json").to_str().unwrap()),
            "{err}"
        );
    }

    #[test]
    fn malformed_composer_auth_env_names_it() {
        let _env = EnvGuard::set(&[("COMPOSER_HOME", None), ("COMPOSER_AUTH", Some("not json"))]);
        let project = tempfile::tempdir().unwrap();
        let err = Auth::load(project.path()).unwrap_err().to_string();
        assert!(err.contains("COMPOSER_AUTH"), "{err}");
    }
}
