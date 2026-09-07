//! Concurrent dist downloads with sha1 verification.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::stream::{Stream, StreamExt};
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{StatusCode, Url};
use serde_json::Value;
use sha1::{Digest, Sha1};

use crate::auth::Auth;
use crate::lock::Package;
use crate::store::hex;

/// Redirect hops to follow before giving up (GitHub's API zipball redirect
/// is one hop; this leaves headroom without looping forever on a bad host).
const MAX_REDIRECTS: u8 = 10;

/// Downloads at or under this size stay in memory, exactly as before #21;
/// larger ones spill to a temp file so `CONCURRENCY` (64) downloads in
/// flight at once don't each hold their own multi-hundred-MB `Vec<u8>`.
const STREAM_THRESHOLD_BYTES: u64 = 8 * 1024 * 1024;

/// One package's downloaded dist archive: buffered in memory when small, or
/// spilled to a temp file (in the store's temp area, so the eventual
/// `add_archive_from_file` rename stays on one filesystem) when larger than
/// `STREAM_THRESHOLD_BYTES`.
pub enum Downloaded {
    Bytes(Vec<u8>),
    File(tempfile::TempPath),
}

impl Downloaded {
    fn len(&self) -> Result<u64> {
        Ok(match self {
            Downloaded::Bytes(bytes) => bytes.len() as u64,
            Downloaded::File(path) => fs_err::metadata(path)?.len(),
        })
    }
}

/// Attempts after the first for a transient failure (connection error,
/// timeout, or a 429/5xx response); Composer and Riff both retry dist
/// downloads up to this many times.
const MAX_RETRIES: u32 = 3;

/// Outcome of [`Fetcher::get_conditional`].
pub enum Conditional {
    /// A body arrived (first fetch, or the cache was stale).
    Fresh {
        body: Vec<u8>,
        last_modified: Option<String>,
    },
    /// The server confirmed the cached body is still current.
    NotModified,
    /// The resource does not exist (a 404, not a transport error).
    NotFound,
}

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
    /// Composer's `config.secure-http` (default `true`): reject a plain
    /// `http://` dist URL or redirect target rather than following it.
    secure_http: bool,
    /// `--offline`/`COMPOSER_DISABLE_NETWORK` (#23): mirrors
    /// `HttpDownloader`'s `disabled` flag, checked before every request
    /// instead of connecting and letting it time out or fail. Composer's own
    /// `startJob` special-cases a conditional (`If-Modified-Since`) request
    /// while disabled by synthesizing a 304 rather than erroring, so a warm
    /// disk cache still serves normally; only a request with nothing cached
    /// to fall back on is rejected. `get_conditional` mirrors that; `fetch`
    /// (dist downloads, never conditional) always rejects.
    offline: bool,
}

impl Fetcher {
    pub fn new(auth: Auth) -> Result<Fetcher> {
        Ok(Fetcher {
            client: client()?,
            auth,
            backoff,
            hop_timings: Mutex::new(HashMap::new()),
            secure_http: true,
            offline: false,
        })
    }

    /// Composer's `config.secure-http`. install.rs wires this to the root
    /// `composer.json`'s `config.secure-http` key; that wiring is not done
    /// here.
    #[must_use]
    pub fn secure_http(mut self, secure_http: bool) -> Self {
        self.secure_http = secure_http;
        self
    }

    /// `--offline`/`COMPOSER_DISABLE_NETWORK` (#23).
    #[must_use]
    pub fn offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
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

    /// Download one package's dist archive and check its `shasum` when set.
    /// `temp_dir` is where a download larger than `STREAM_THRESHOLD_BYTES`
    /// spills to (#21), rather than growing an ever-larger `Vec<u8>`.
    pub async fn fetch(&self, pkg: &Package, temp_dir: &Path) -> Result<Downloaded> {
        pkg.validate_dist()?;
        let dist = pkg.dist.as_ref().expect("validate_dist checked");
        let url = Url::parse(&dist.url)
            .with_context(|| format!("{}: invalid dist URL {}", pkg.name, dist.url))?;
        if self.offline {
            bail!(
                "{}: Network disabled, request canceled: {}",
                pkg.name,
                redact(&url)
            );
        }
        let started = std::time::Instant::now();
        let (downloaded, actual_sha1) = self
            .get(&pkg.name, url.clone(), temp_dir)
            .await
            .with_context(|| format!("{}: downloading {}", pkg.name, redact(&url)))?;
        verify_shasum(
            &pkg.name,
            dist.shasum.as_deref().unwrap_or(""),
            &actual_sha1,
        )?;
        tracing::debug!(
            package = %pkg.name,
            bytes = downloaded.len()?,
            elapsed_ms = started.elapsed().as_millis(),
            "downloaded dist"
        );
        Ok(downloaded)
    }

    /// Download every package with at most `concurrency` requests in
    /// flight, yielding each result as it completes (not in input order).
    pub fn fetch_all<'a>(
        &'a self,
        packages: impl IntoIterator<Item = &'a Package> + 'a,
        concurrency: usize,
        temp_dir: &'a Path,
    ) -> impl Stream<Item = (&'a Package, Result<Downloaded>)> + 'a {
        futures::stream::iter(packages)
            .map(move |pkg| async move { (pkg, self.fetch(pkg, temp_dir).await) })
            .buffer_unordered(concurrency)
    }

    /// Outcome of a conditional GET (`If-Modified-Since`), for the
    /// repository client's HTTP cache: a fresh body to cache, confirmation
    /// the cached body is still current (304), or confirmation the
    /// resource is genuinely absent (404, which the repository client
    /// treats as "no versions", not an error).
    pub async fn get_conditional(
        &self,
        label: &str,
        url: &Url,
        if_modified_since: Option<&str>,
    ) -> Result<Conditional> {
        require_https(label, url, self.secure_http)?;
        if self.offline {
            // Composer's `HttpDownloader::startJob` while `disabled`: a
            // conditional request (a `Last-Modified` cached to revalidate
            // with) is answered with a synthetic 304 instead of erroring, so
            // a warm cache still works offline; nothing to revalidate from
            // means there's nothing to serve, so that case errors.
            return if if_modified_since.is_some() {
                Ok(Conditional::NotModified)
            } else {
                bail!(
                    "{label}: Network disabled, request canceled: {}",
                    redact(url)
                )
            };
        }
        let mut headers = Vec::new();
        if let Some(since) = if_modified_since {
            headers.push((
                reqwest::header::IF_MODIFIED_SINCE,
                HeaderValue::from_str(since).context("invalid If-Modified-Since value")?,
            ));
        }
        let response = self.send_with_retries(label, url, &headers).await?;
        match response.status() {
            StatusCode::NOT_MODIFIED => Ok(Conditional::NotModified),
            StatusCode::NOT_FOUND => Ok(Conditional::NotFound),
            _ => {
                let response = response
                    .error_for_status()
                    .with_context(|| format!("fetching {}", redact(url)))?;
                let last_modified = response
                    .headers()
                    .get(reqwest::header::LAST_MODIFIED)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let body = response.bytes().await?.to_vec();
                Ok(Conditional::Fresh {
                    body,
                    last_modified,
                })
            }
        }
    }

    /// `POST url` with `packages[]=<name>` form fields, for
    /// `audit::run`'s single call to `packagist.org/api/security-advisories/`
    /// (`ComposerRepository::getSecurityAdvisories`'s plain, non-lazy path).
    /// Retries the same way a dist download does; no redirects are expected
    /// from this endpoint, so unlike `get` this doesn't follow any.
    pub async fn post_json(&self, label: &str, url: &Url, packages: &[String]) -> Result<Value> {
        require_https(label, url, self.secure_http)?;
        if self.offline {
            bail!(
                "{label}: Network disabled, request canceled: {}",
                redact(url)
            );
        }
        let form: Vec<(&str, &str)> = packages
            .iter()
            .map(|name| ("packages[]", name.as_str()))
            .collect();
        let response = self.send_post_with_retries(label, url, &form).await?;
        let response = response
            .error_for_status()
            .with_context(|| format!("posting to {}", redact(url)))?;
        let body = response.bytes().await?;
        serde_json::from_slice(&body).with_context(|| format!("parsing JSON from {}", redact(url)))
    }

    /// `POST url` once, retrying a transient failure the same way
    /// `send_with_retries` does for a `GET`.
    async fn send_post_with_retries(
        &self,
        label: &str,
        url: &Url,
        form: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        let mut attempt = 0u32;
        loop {
            let mut request = self.client.post(url.clone()).form(form);
            if let Some((name, value)) = self.auth.header_for(url) {
                request = request.header(name, value);
            }
            for (name, value) in self.auth.custom_headers_for(url) {
                request = request.header(name, value);
            }
            let outcome = request.send().await;
            let retryable = match &outcome {
                Ok(response) => should_retry(Ok(response.status())),
                Err(err) => (err.is_connect() || err.is_timeout()) && should_retry(Err(())),
            };
            if !retryable || attempt >= MAX_RETRIES {
                return outcome.map_err(|err| {
                    anyhow::anyhow!("{} posting to {}", err.without_url(), redact(url))
                });
            }
            attempt += 1;
            let delay = outcome
                .as_ref()
                .ok()
                .and_then(|response| retry_after(response.headers()))
                .unwrap_or_else(|| (self.backoff)(attempt));
            tracing::debug!(
                label,
                url = %redact(url),
                attempt,
                delay_ms = delay.as_millis(),
                "retrying advisories POST"
            );
            tokio::time::sleep(delay).await;
        }
    }

    /// `GET start_url`, following redirects by hand so each hop gets the
    /// credential for *its* host rather than reusing (or losing) the first
    /// hop's.
    async fn get(
        &self,
        pkg_name: &str,
        start_url: Url,
        temp_dir: &Path,
    ) -> Result<(Downloaded, String)> {
        require_https(pkg_name, &start_url, self.secure_http)?;
        let mut url = start_url;
        let mut hops = 0u8;
        loop {
            if redirect_budget_exhausted(hops) {
                bail!("{pkg_name}: too many redirects fetching {}", redact(&url));
            }
            hops += 1;
            let host = url.host_str().unwrap_or("").to_string();
            let hop_started = std::time::Instant::now();
            let response = self.send_with_retries(pkg_name, &url, &[]).await?;
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
                require_https(pkg_name, &url, self.secure_http)?;
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
                // `err`'s own Display embeds the request URL verbatim
                // (reqwest's `Error::fmt`); strip it so a URL with
                // credentials never reaches this message unredacted.
                anyhow::anyhow!("{} fetching {}{hint}", err.without_url(), redact(&url))
            })?;
            // Timed through the body read (not just headers), so this hop's
            // number is comparable to the redirect hop above and reflects
            // what actually holds up the fetch: a GitHub zipball's headers
            // arrive quickly, the archive bytes behind them do not.
            let (downloaded, sha1_hex) = read_body(response, temp_dir)
                .await
                .with_context(|| format!("reading response body from {}", redact(&url)))?;
            let elapsed = hop_started.elapsed();
            self.record_hop(&host, elapsed);
            tracing::debug!(
                package = %pkg_name,
                host,
                hop = hops,
                status = status.as_u16(),
                bytes = downloaded.len()?,
                elapsed_ms = elapsed.as_millis(),
                "fetch hop (body complete)"
            );
            return Ok((downloaded, sha1_hex));
        }
    }

    /// `GET url` once, retrying a transient failure (connection error,
    /// timeout, or 429/5xx) up to `MAX_RETRIES` times with backoff; a
    /// redirect response is returned as-is, since `get` needs to decide the
    /// next hop's URL before it can be retried.
    async fn send_with_retries(
        &self,
        pkg_name: &str,
        url: &Url,
        extra_headers: &[(HeaderName, HeaderValue)],
    ) -> Result<reqwest::Response> {
        // Best-effort: a failed exchange (bad consumer key/secret, network
        // hiccup) leaves the bitbucket-oauth cache empty rather than failing
        // the whole fetch here; `header_for` then sends no Authorization
        // header, and `credential_hint` on the resulting 401/403 covers it.
        if let Some(host) = url.host_str()
            && let Err(err) = self.auth.ensure_bitbucket_token(host).await
        {
            tracing::debug!(host, error = %err, "bitbucket OAuth token exchange failed");
        }
        let mut attempt = 0u32;
        loop {
            let mut request = self.client.get(url.clone());
            if let Some((name, value)) = self.auth.header_for(url) {
                request = request.header(name, value);
            }
            for (name, value) in self.auth.custom_headers_for(url) {
                request = request.header(name, value);
            }
            for (name, value) in extra_headers {
                request = request.header(name, value);
            }
            let outcome = request.send().await;
            let retryable = match &outcome {
                Ok(response) => should_retry(Ok(response.status())),
                Err(err) => (err.is_connect() || err.is_timeout()) && should_retry(Err(())),
            };
            if !retryable || attempt >= MAX_RETRIES {
                return outcome.map_err(|err| {
                    // Same reasoning as the error_for_status case in `get`:
                    // don't let reqwest's own URL-embedding Display leak a
                    // credential here.
                    anyhow::anyhow!("{} fetching {}", err.without_url(), redact(url))
                });
            }
            attempt += 1;
            let delay = outcome
                .as_ref()
                .ok()
                .and_then(|response| retry_after(response.headers()))
                .unwrap_or_else(|| (self.backoff)(attempt));
            tracing::debug!(
                package = %pkg_name,
                url = %redact(url),
                attempt,
                delay_ms = delay.as_millis(),
                "retrying dist download"
            );
            tokio::time::sleep(delay).await;
        }
    }
}

/// Read a successful response's body, hashing it (sha1, for the `shasum`
/// check) as bytes arrive rather than re-reading it afterward. Spills to a
/// temp file in `temp_dir` once the body turns out larger than
/// [`STREAM_THRESHOLD_BYTES`] — checked against `Content-Length` up front
/// when the server sent one, or against the running total as chunks arrive
/// otherwise (a `Content-Length`-less or lying response still gets caught,
/// just after buffering the first few MiB instead of before the first byte).
///
/// ponytail: each chunk is written with a blocking `std::io::Write` call on
/// the async task rather than `spawn_blocking`; only large downloads spill
/// to a file at all, and archives at Composer's typical sizes stay well
/// under a few hundred writes. Move the writes to a blocking task if a
/// profile ever shows this contending with other in-flight downloads.
async fn read_body(response: reqwest::Response, temp_dir: &Path) -> Result<(Downloaded, String)> {
    let spill_now = response
        .content_length()
        .is_some_and(|len| len > STREAM_THRESHOLD_BYTES);
    let mut hasher = Sha1::new();
    let mut stream = response.bytes_stream();

    if spill_now {
        let mut file = tempfile::Builder::new()
            .prefix(".dl-")
            .tempfile_in(temp_dir)?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("reading response body")?;
            hasher.update(&chunk);
            file.write_all(&chunk)?;
        }
        return Ok((
            Downloaded::File(file.into_temp_path()),
            hex(hasher.finalize()),
        ));
    }

    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading response body")?;
        hasher.update(&chunk);
        buf.extend_from_slice(&chunk);
        if buf.len() as u64 > STREAM_THRESHOLD_BYTES {
            let mut file = tempfile::Builder::new()
                .prefix(".dl-")
                .tempfile_in(temp_dir)?;
            file.write_all(&buf)?;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.context("reading response body")?;
                hasher.update(&chunk);
                file.write_all(&chunk)?;
            }
            return Ok((
                Downloaded::File(file.into_temp_path()),
                hex(hasher.finalize()),
            ));
        }
    }
    Ok((Downloaded::Bytes(buf), hex(hasher.finalize())))
}

/// Resolve a `Location` header (relative or absolute) against the URL that
/// produced it.
fn redirect_target(current: &Url, location: &str) -> Result<Url> {
    current
        .join(location)
        .with_context(|| format!("invalid redirect Location {location:?}"))
}

/// Composer's `secure-http` check: reject a plain `http://` dist URL or
/// redirect target unless the caller opted out (`config.secure-http: false`,
/// wired to [`Fetcher::secure_http`]). A dist host redirecting to `http`
/// would otherwise silently drop back to a channel a MITM can tamper with.
fn require_https(pkg_name: &str, url: &Url, secure_http: bool) -> Result<()> {
    if !secure_http || url.scheme() == "https" {
        return Ok(());
    }
    bail!(
        "{pkg_name}: refusing to fetch {} over {} (set config.secure-http to false to allow this)",
        redact(url),
        url.scheme()
    );
}

/// `url` with any embedded HTTP Basic credentials masked, for error messages
/// and log lines: a dist URL or a redirect target can carry
/// `https://user:pass@host/...`, and this must never reach a log or an error
/// unredacted.
fn redact(url: &Url) -> String {
    let mut redacted = url.clone();
    if !redacted.username().is_empty() {
        let _ = redacted.set_username("***");
    }
    if redacted.password().is_some() {
        let _ = redacted.set_password(Some("***"));
    }
    redacted.to_string()
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
fn verify_shasum(name: &str, expected: &str, actual: &str) -> Result<()> {
    if expected.is_empty() {
        return Ok(());
    }
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
        verify_shasum("acme/pkg", HELLO, &sha1_hex(b"hello")).unwrap();
    }

    #[test]
    fn mismatching_shasum_names_package_and_digests() {
        let err = verify_shasum("acme/pkg", HELLO, &sha1_hex(b"hell0"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("acme/pkg"), "{err}");
        assert!(err.contains(HELLO), "{err}");
        assert!(err.contains(&sha1_hex(b"hell0")), "{err}");
    }

    #[test]
    fn empty_shasum_skips_check() {
        verify_shasum("acme/pkg", "", &sha1_hex(b"anything")).unwrap();
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
    fn require_https_rejects_http() {
        let url = reqwest::Url::parse("http://example.test/a.zip").unwrap();
        let err = require_https("acme/pkg", &url, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("acme/pkg"), "{err}");
        assert!(err.contains("config.secure-http"), "{err}");
    }

    #[test]
    fn require_https_accepts_https() {
        let url = reqwest::Url::parse("https://example.test/a.zip").unwrap();
        require_https("acme/pkg", &url, true).unwrap();
    }

    #[test]
    fn require_https_disabled_allows_http() {
        let url = reqwest::Url::parse("http://example.test/a.zip").unwrap();
        require_https("acme/pkg", &url, false).unwrap();
    }

    #[test]
    fn redirect_to_http_is_rejected() {
        // Pure function on the Location target: no live server involved.
        let current = reqwest::Url::parse("https://example.test/a.zip").unwrap();
        let target = redirect_target(&current, "http://example.test/legacy.zip").unwrap();
        assert!(require_https("acme/pkg", &target, true).is_err());
    }

    #[test]
    fn redact_masks_username_and_password() {
        let url = reqwest::Url::parse("https://user:pass@example.test/a.zip").unwrap();
        assert_eq!(redact(&url), "https://***:***@example.test/a.zip");
    }

    #[test]
    fn redact_masks_password_only_username() {
        let url = reqwest::Url::parse("https://tok@example.test/a.zip").unwrap();
        assert_eq!(redact(&url), "https://***@example.test/a.zip");
    }

    #[test]
    fn redact_is_a_no_op_without_credentials() {
        let url = reqwest::Url::parse("https://example.test/a.zip?x=1").unwrap();
        assert_eq!(redact(&url), "https://example.test/a.zip?x=1");
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

    /// A minimal package with a dist, for the offline tests below; every
    /// field `fetch` never reads is left at a cheap default.
    fn dist_package(name: &str, url: &str) -> Package {
        Package {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            dist: Some(crate::lock::Dist {
                r#type: "zip".to_string(),
                url: url.to_string(),
                reference: None,
                shasum: None,
            }),
            source: None,
            transport_options: crate::lock::TransportOptions::default(),
            autoload: None,
            require: serde_json::Map::new(),
            provide: serde_json::Map::new(),
            replace: serde_json::Map::new(),
            r#type: "library".to_string(),
            target_dir: None,
            include_path: Vec::new(),
            bin: Vec::new(),
            dev: false,
            raw: serde_json::Value::Null,
            install_dir: None,
            install_from_source: false,
        }
    }

    #[tokio::test]
    async fn offline_fetch_fails_fast_and_names_the_package() {
        let fetcher = Fetcher::new(Auth::default()).unwrap().offline(true);
        let pkg = dist_package("acme/pkg", "https://example.test/a.zip");
        let temp = tempfile::tempdir().unwrap();
        let err = fetcher
            .fetch(&pkg, temp.path())
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("acme/pkg"), "{err}");
        assert!(err.contains("Network disabled, request canceled"), "{err}");
        assert!(err.contains("example.test/a.zip"), "{err}");
    }

    #[tokio::test]
    async fn offline_get_conditional_without_a_cached_body_errors() {
        let fetcher = Fetcher::new(Auth::default()).unwrap().offline(true);
        let url = Url::parse("https://repo.packagist.org/p2/acme/pkg.json").unwrap();
        let err = fetcher
            .get_conditional("acme/pkg", &url, None)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("Network disabled, request canceled"), "{err}");
        assert!(err.contains("repo.packagist.org"), "{err}");
    }

    #[tokio::test]
    async fn offline_get_conditional_with_a_cached_last_modified_serves_not_modified() {
        let fetcher = Fetcher::new(Auth::default()).unwrap().offline(true);
        let url = Url::parse("https://repo.packagist.org/p2/acme/pkg.json").unwrap();
        let result = fetcher
            .get_conditional("acme/pkg", &url, Some("Mon, 01 Jan 2024 00:00:00 GMT"))
            .await
            .unwrap();
        assert!(matches!(result, Conditional::NotModified));
    }
}
