//! Concurrent dist downloads with sha1 verification.

use std::collections::HashMap;
use std::sync::Mutex;
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

/// Attempts after the first for a transient failure (connection error,
/// timeout, or a 429/5xx response); Composer and Riff both retry dist
/// downloads up to this many times.
const MAX_RETRIES: u32 = 3;

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
    /// Delay before each retry; a field (not the `backoff` free function
    /// directly) so tests can swap in a zero-delay schedule instead of
    /// actually sleeping.
    backoff: fn(u32) -> Duration,
    /// Elapsed time of every hop (one entry per non-redirect and per
    /// redirect response), keyed by host, so `log_hop_summary` can report a
    /// distribution instead of only a total — #1 needed this to see whether
    /// the cold-path gap to Riff was one slow hop repeated or something
    /// systemic.
    hop_timings: Mutex<HashMap<String, Vec<Duration>>>,
}

impl Fetcher {
    pub fn new(auth: Auth) -> Result<Fetcher> {
        Ok(Fetcher {
            client: client()?,
            auth,
            backoff,
            hop_timings: Mutex::new(HashMap::new()),
        })
    }

    /// Log a per-host count/min/median/max of every hop timed since this
    /// `Fetcher` was created (debug level only; the map is cheap but not
    /// worth building outside a debug run).
    pub fn log_hop_summary(&self) {
        if !tracing::enabled!(tracing::Level::DEBUG) {
            return;
        }
        let timings = self.hop_timings.lock().expect("hop_timings mutex");
        for (host, durations) in timings.iter() {
            let mut ms: Vec<u128> = durations.iter().map(Duration::as_millis).collect();
            ms.sort_unstable();
            let count = ms.len();
            let min = ms.first().copied().unwrap_or(0);
            let max = ms.last().copied().unwrap_or(0);
            let median = ms.get(count / 2).copied().unwrap_or(0);
            tracing::debug!(
                host,
                count,
                min_ms = min,
                median_ms = median,
                max_ms = max,
                "hop timing summary"
            );
        }
    }

    /// Record one hop's elapsed time under its host, for `log_hop_summary`.
    fn record_hop(&self, host: &str, elapsed: Duration) {
        self.hop_timings
            .lock()
            .expect("hop_timings mutex")
            .entry(host.to_string())
            .or_default()
            .push(elapsed);
    }

    /// Download one package's dist zip and check its `shasum` when set.
    pub async fn fetch(&self, pkg: &Package) -> Result<Vec<u8>> {
        pkg.validate_dist()?;
        let dist = pkg.dist.as_ref().expect("validate_dist checked");
        let started = std::time::Instant::now();
        // ponytail: the whole zip lives in memory at once (fine at
        // Composer's typical archive sizes); stream to a temp file if that
        // stops being true.
        let bytes = self
            .get(&pkg.name, &dist.url)
            .await
            .with_context(|| format!("{}: downloading {}", pkg.name, dist.url))?;
        verify_shasum(&pkg.name, dist.shasum.as_deref().unwrap_or(""), &bytes)?;
        tracing::debug!(
            package = %pkg.name,
            bytes = bytes.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "downloaded dist"
        );
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
        let mut hops = 0u8;
        loop {
            if redirect_budget_exhausted(hops) {
                bail!("{pkg_name}: too many redirects fetching {start_url}");
            }
            hops += 1;
            let host = url.host_str().unwrap_or("").to_string();
            let hop_started = std::time::Instant::now();
            let response = self.send_with_retries(pkg_name, &url).await?;
            if response.status().is_redirection() {
                let elapsed = hop_started.elapsed();
                self.record_hop(&host, elapsed);
                tracing::debug!(
                    package = %pkg_name,
                    host,
                    hop = hops,
                    status = response.status().as_u16(),
                    elapsed_ms = elapsed.as_millis(),
                    "fetch hop (redirect)"
                );
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
            // Timed through the body read (not just headers), so this hop's
            // number is comparable to the redirect hop above and reflects
            // what actually holds up the fetch: a GitHub zipball's headers
            // arrive quickly, the archive bytes behind them do not.
            let bytes = response.bytes().await?.to_vec();
            let elapsed = hop_started.elapsed();
            self.record_hop(&host, elapsed);
            tracing::debug!(
                package = %pkg_name,
                host,
                hop = hops,
                status = status.as_u16(),
                bytes = bytes.len(),
                elapsed_ms = elapsed.as_millis(),
                "fetch hop (body complete)"
            );
            return Ok(bytes);
        }
    }

    /// `GET url` once, retrying a transient failure (connection error,
    /// timeout, or 429/5xx) up to `MAX_RETRIES` times with backoff; a
    /// redirect response is returned as-is, since `get` needs to decide the
    /// next hop's URL before it can be retried.
    async fn send_with_retries(&self, pkg_name: &str, url: &Url) -> Result<reqwest::Response> {
        let mut attempt = 0u32;
        loop {
            let mut request = self.client.get(url.clone());
            if let Some((name, value)) = self.auth.header_for(url) {
                request = request.header(name, value);
            }
            let outcome = request.send().await;
            let retryable = match &outcome {
                Ok(response) => should_retry(Ok(response.status())),
                Err(err) => (err.is_connect() || err.is_timeout()) && should_retry(Err(())),
            };
            if !retryable || attempt >= MAX_RETRIES {
                return Ok(outcome?);
            }
            attempt += 1;
            let delay = outcome
                .as_ref()
                .ok()
                .and_then(|response| retry_after(response.headers()))
                .unwrap_or_else(|| (self.backoff)(attempt));
            tracing::debug!(
                package = %pkg_name,
                %url,
                attempt,
                delay_ms = delay.as_millis(),
                "retrying dist download"
            );
            tokio::time::sleep(delay).await;
        }
    }
}

/// Resolve a `Location` header (relative or absolute) against the URL that
/// produced it.
fn redirect_target(current: &Url, location: &str) -> Result<Url> {
    current
        .join(location)
        .with_context(|| format!("invalid redirect Location {location:?}"))
}

/// Whether `hops` redirects already reached `MAX_REDIRECTS`, kept as a pure
/// check so the cap is testable without a live server.
fn redirect_budget_exhausted(hops: u8) -> bool {
    hops >= MAX_REDIRECTS
}

/// Whether a response status, or a transport failure (`Err`), should trigger
/// a retry. Composer and Riff both retry 429 and 5xx dist-download
/// responses; other 4xx (401/403/404/etc.) are never transient, so they
/// aren't retried. A transport failure only reaches this function after the
/// caller has already checked it's a connection error or a timeout (a
/// malformed URL or a decode error retrying wouldn't fix), so it's always
/// worth another attempt.
fn should_retry(outcome: Result<StatusCode, ()>) -> bool {
    match outcome {
        Ok(status) => matches!(
            status,
            StatusCode::TOO_MANY_REQUESTS
                | StatusCode::INTERNAL_SERVER_ERROR
                | StatusCode::BAD_GATEWAY
                | StatusCode::SERVICE_UNAVAILABLE
                | StatusCode::GATEWAY_TIMEOUT
        ),
        Err(()) => true,
    }
}

/// Delay before retry `attempt` (1-based): Composer and Riff both back off
/// 1s, 2s, 4s across three retries before giving up.
fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(1 << attempt.saturating_sub(1))
}

/// `Retry-After` as whole seconds, capped at 30s so a server's large value
/// can't stall a fetch for minutes. `None` when the header is missing or
/// isn't a plain integer (dist hosts don't send the HTTP-date form, so it
/// isn't worth parsing here); the caller falls back to `backoff` then.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    let secs: u64 = value.parse().ok()?;
    Some(Duration::from_secs(secs.min(30)))
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
    fn redirect_budget_caps_at_max_redirects() {
        assert!(!redirect_budget_exhausted(MAX_REDIRECTS - 1));
        assert!(redirect_budget_exhausted(MAX_REDIRECTS));
        assert!(redirect_budget_exhausted(MAX_REDIRECTS + 1));
    }

    #[test]
    fn should_retry_on_429_and_5xx() {
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            assert!(should_retry(Ok(status)), "{status}");
        }
    }

    #[test]
    fn should_retry_false_for_other_4xx() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
        ] {
            assert!(!should_retry(Ok(status)), "{status}");
        }
    }

    #[test]
    fn should_retry_false_for_success() {
        assert!(!should_retry(Ok(StatusCode::OK)));
    }

    #[test]
    fn should_retry_true_for_transport_error() {
        assert!(should_retry(Err(())));
    }

    #[test]
    fn backoff_schedule_is_1_2_4_seconds() {
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(2), Duration::from_secs(2));
        assert_eq!(backoff(3), Duration::from_secs(4));
    }

    #[test]
    fn retry_after_reads_small_values() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("5"),
        );
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(5)));
    }

    #[test]
    fn retry_after_caps_large_values_at_30s() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("3600"),
        );
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(30)));
    }

    #[test]
    fn retry_after_absent_when_header_missing() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after(&headers), None);
    }

    #[test]
    fn retry_after_absent_for_http_date_form() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("Wed, 21 Oct 2026 07:28:00 GMT"),
        );
        assert_eq!(retry_after(&headers), None);
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
