//! Concurrent dist downloads with sha1 verification.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::stream::{Stream, StreamExt};
use sha1::{Digest, Sha1};

use crate::lock::Package;
use crate::store::hex;

/// A client with vivace's User-Agent and a per-request timeout long enough
/// for a large zip on a slow link.
pub fn client() -> Result<reqwest::Client> {
    // ponytail: no retries and no auth in v0.1; honour COMPOSER_AUTH
    // (github-oauth) and add a retry loop when GitHub rate limits bite.
    Ok(reqwest::Client::builder()
        .user_agent(concat!("vivace/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_mins(5))
        .build()?)
}

/// Download one package's dist zip and check its `shasum` when set.
pub async fn fetch(client: &reqwest::Client, pkg: &Package) -> Result<Vec<u8>> {
    pkg.validate_dist()?;
    let dist = pkg.dist.as_ref().expect("validate_dist checked");
    // ponytail: the whole zip lives in memory at once (fine at Composer's
    // typical archive sizes); stream to a temp file if that stops being true.
    let bytes = client
        .get(&dist.url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("{}: downloading {}", pkg.name, dist.url))?
        .bytes()
        .await
        .with_context(|| format!("{}: reading {}", pkg.name, dist.url))?;
    verify_shasum(&pkg.name, dist.shasum.as_deref().unwrap_or(""), &bytes)?;
    Ok(bytes.to_vec())
}

/// Download every package with at most `concurrency` requests in flight,
/// yielding each result as it completes (not in input order).
pub fn fetch_all<'a>(
    client: &'a reqwest::Client,
    packages: impl IntoIterator<Item = &'a Package> + 'a,
    concurrency: usize,
) -> impl Stream<Item = (&'a Package, Result<Vec<u8>>)> + 'a {
    futures::stream::iter(packages)
        .map(move |pkg| async move { (pkg, fetch(client, pkg).await) })
        .buffer_unordered(concurrency)
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
}
