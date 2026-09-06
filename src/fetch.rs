//! Concurrent dist downloads with sha1 verification.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::stream::{Stream, StreamExt};
use reqwest::{StatusCode, Url};
use sha1::{Digest, Sha1};

use crate::auth::Auth;
use crate::lock::Package;
use crate::store::hex;

/// Redirect hops to follow before giving up (GitHub's API zipball redirect
/// is one hop; this leaves headroom without looping forever on a bad host).
const MAX_REDIRECTS: u8 = 10;

/// A client with vivace's User-Agent and a per-request timeout long enough
/// for a large zip on a slow link.
///
/// Redirects are followed manually by `Fetcher::fetch` instead of by
/// reqwest, because reqwest's default `redirect::Policy` strips the
/// `Authorization` header on any cross-host hop (see
/// `remove_sensitive_headers` in reqwest's `redirect.rs`) — exactly the
/// api.github.com → codeload.github.com hop a private repo's zipball takes,
/// and codeload needs the same token api.github.com did.
fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("vivace/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_mins(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

/// A download client paired with the credentials to send per host.
pub struct Fetcher {
    client: reqwest::Client,
    auth: Auth,
}

impl Fetcher {
    pub fn new(auth: Auth) -> Result<Fetcher> {
        Ok(Fetcher {
            client: client()?,
            auth,
        })
    }

    /// Download one package's dist zip and check its `shasum` when set.
    pub async fn fetch(&self, pkg: &Package) -> Result<Vec<u8>> {
        pkg.validate_dist()?;
        let dist = pkg.dist.as_ref().expect("validate_dist checked");
        // ponytail: the whole zip lives in memory at once (fine at
        // Composer's typical archive sizes); stream to a temp file if that
        // stops being true.
        let bytes = self
            .get(&pkg.name, &dist.url)
            .await
            .with_context(|| format!("{}: downloading {}", pkg.name, dist.url))?;
        verify_shasum(&pkg.name, dist.shasum.as_deref().unwrap_or(""), &bytes)?;
        Ok(bytes)
    }

    /// Download every package with at most `concurrency` requests in
    /// flight, yielding each result as it completes (not in input order).
    pub fn fetch_all<'a>(
        &'a self,
        packages: impl IntoIterator<Item = &'a Package> + 'a,
        concurrency: usize,
    ) -> impl Stream<Item = (&'a Package, Result<Vec<u8>>)> + 'a {
        futures::stream::iter(packages)
            .map(move |pkg| async move { (pkg, self.fetch(pkg).await) })
            .buffer_unordered(concurrency)
    }

    /// `GET start_url`, following redirects by hand so each hop gets the
    /// credential for *its* host rather than reusing (or losing) the first
    /// hop's.
    async fn get(&self, pkg_name: &str, start_url: &str) -> Result<Vec<u8>> {
        let mut url = Url::parse(start_url)
            .with_context(|| format!("{pkg_name}: invalid dist URL {start_url}"))?;
        for _ in 0..MAX_REDIRECTS {
            let mut request = self.client.get(url.clone());
            if let Some((name, value)) = self.auth.header_for(&url) {
                request = request.header(name, value);
            }
            let response = request.send().await?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .context("redirect response has no Location header")?
                    .to_str()
                    .context("redirect Location header is not valid UTF-8")?
                    .to_string();
                url = redirect_target(&url, &location)?;
                continue;
            }
            let status = response.status();
            let response = response.error_for_status().map_err(|err| {
                let hint = credential_hint(
                    status.as_u16(),
                    url.host_str().unwrap_or(""),
                    self.auth.header_for(&url).is_some(),
                )
                .unwrap_or_default();
                anyhow::anyhow!("{err}{hint}")
            })?;
            return Ok(response.bytes().await?.to_vec());
        }
        bail!("{pkg_name}: too many redirects fetching {start_url}");
    }
}

/// Resolve a `Location` header (relative or absolute) against the URL that
/// produced it.
fn redirect_target(current: &Url, location: &str) -> Result<Url> {
    current
        .join(location)
        .with_context(|| format!("invalid redirect Location {location:?}"))
}

/// A trailer to append to a download error when the host rejected the
/// request and vivace never had a credential to send it: 401/403 are
/// unambiguous, and a private repo's 404 from GitHub looks identical to a
/// missing one.
fn credential_hint(status: u16, host: &str, has_credential: bool) -> Option<String> {
    let status = StatusCode::from_u16(status).ok()?;
    if has_credential
        || !matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        )
    {
        return None;
    }
    Some(format!(
        "; no credentials found for {host}; vivace reads github-oauth and http-basic from \
         auth.json and COMPOSER_AUTH"
    ))
}

/// Composer's `dist.shasum` is the sha1 of the archive, or `""` (GitHub
/// zipballs), in which case there is nothing to check.
fn verify_shasum(name: &str, expected: &str, bytes: &[u8]) -> Result<()> {
    if expected.is_empty() {
        return Ok(());
    }
    let actual = hex(Sha1::digest(bytes));
    if actual != expected {
        bail!("{name}: sha1 mismatch, lock says {expected} but the download is {actual}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // sha1("hello")
    const HELLO: &str = "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d";

    fn sha1_hex(bytes: &[u8]) -> String {
        hex(Sha1::digest(bytes))
    }

    #[test]
    fn matching_shasum_passes() {
        verify_shasum("acme/pkg", HELLO, b"hello").unwrap();
    }

    #[test]
    fn mismatching_shasum_names_package_and_digests() {
        let err = verify_shasum("acme/pkg", HELLO, b"hell0")
            .unwrap_err()
            .to_string();
        assert!(err.contains("acme/pkg"), "{err}");
        assert!(err.contains(HELLO), "{err}");
        assert!(err.contains(&sha1_hex(b"hell0")), "{err}");
    }

    #[test]
    fn empty_shasum_skips_check() {
        verify_shasum("acme/pkg", "", b"anything").unwrap();
    }

    #[test]
    fn redirect_target_resolves_relative_location() {
        let current =
            reqwest::Url::parse("https://api.github.com/repos/acme/pkg/zipball/abc").unwrap();
        let next = redirect_target(&current, "/acme/pkg/legacy.zip/abc").unwrap();
        assert_eq!(
            next.as_str(),
            "https://api.github.com/acme/pkg/legacy.zip/abc"
        );
    }

    #[test]
    fn redirect_target_resolves_absolute_location() {
        let current =
            reqwest::Url::parse("https://api.github.com/repos/acme/pkg/zipball/abc").unwrap();
        let next = redirect_target(
            &current,
            "https://codeload.github.com/acme/pkg/legacy.zip/abc",
        )
        .unwrap();
        assert_eq!(next.host_str(), Some("codeload.github.com"));
    }

    #[test]
    fn redirect_target_rejects_invalid_location() {
        let current = reqwest::Url::parse("https://api.github.com/x").unwrap();
        assert!(redirect_target(&current, "http://exa mple.com/y").is_err());
    }

    #[test]
    fn credential_hint_added_for_unauthenticated_404() {
        let hint = credential_hint(404, "api.github.com", false).unwrap();
        assert!(hint.contains("api.github.com"), "{hint}");
        assert!(hint.contains("auth.json"), "{hint}");
        assert!(hint.contains("COMPOSER_AUTH"), "{hint}");
    }

    #[test]
    fn credential_hint_added_for_401_and_403() {
        assert!(credential_hint(401, "example.com", false).is_some());
        assert!(credential_hint(403, "example.com", false).is_some());
    }

    #[test]
    fn credential_hint_absent_when_credential_already_sent() {
        assert!(credential_hint(404, "api.github.com", true).is_none());
    }

    #[test]
    fn credential_hint_absent_for_other_statuses() {
        assert!(credential_hint(500, "example.com", false).is_none());
        assert!(credential_hint(200, "example.com", false).is_none());
    }
}
