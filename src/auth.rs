//! Composer-compatible credentials for dist downloads.
//!
//! Composer reads `auth.json` from two places plus the `COMPOSER_AUTH`
//! environment variable, merging them in ascending precedence: composer
//! home, then the project directory, then the environment. See
//! <https://getcomposer.org/doc/articles/http-basic-authentication.md>.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use reqwest::Url;
use reqwest::header::{AUTHORIZATION, HeaderName, HeaderValue};
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
    /// consumer-key, consumer-secret; loaded so a lock with bitbucket dists
    /// is recognised, never turned into a header (see `header_for`).
    bitbucket_oauth: HashMap<String, (String, String)>,
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
        f.debug_struct("Auth")
            .field("github_oauth_hosts", &hosts(&self.github_oauth))
            .field("http_basic_hosts", &hosts(&self.http_basic))
            .field("bearer_hosts", &hosts(&self.bearer))
            .field("gitlab_oauth_hosts", &hosts(&self.gitlab_oauth))
            .field("gitlab_token_hosts", &hosts(&self.gitlab_token))
            .field("bitbucket_oauth_hosts", &hosts(&self.bitbucket_oauth))
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
}

#[derive(Deserialize)]
struct RawBasic {
    username: String,
    password: String,
}

/// Composer stores more fields here (`access-token`,
/// `access-token-expiration`) once it has exchanged the consumer key/secret
/// for a token; vivace doesn't do that exchange, so it only reads the two
/// fields it can act on. Unknown fields are ignored by serde's default.
#[derive(Deserialize)]
struct RawBitbucketOauth {
    #[serde(rename = "consumer-key")]
    consumer_key: String,
    #[serde(rename = "consumer-secret")]
    consumer_secret: String,
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
        for host in raw.bitbucket_oauth.keys() {
            tracing::debug!(
                host,
                "bitbucket-oauth found in auth.json; vivace does not exchange the \
                 consumer-key/consumer-secret for an access token, so requests to this host \
                 send no Authorization header"
            );
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
        if self.bitbucket_oauth.contains_key(&host) {
            // ponytail: no Authorization header until the OAuth2 exchange
            // (POST consumer-key/consumer-secret to
            // bitbucket.org/site/oauth2/access_token) is implemented; `None`
            // here is deliberate so `credential_hint` still fires on the
            // resulting 401/403.
            return None;
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
