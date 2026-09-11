#![no_main]

//! `expand_minified` (private in `repository.rs`, `MetadataMinifier::expand`
//! port) fuzzed through the nearest public entry point: a real
//! `Repository<T>` loaded against a fake `Transport` that serves a `p2`
//! provider file built from an arbitrary list of version diffs, with
//! `"minified": "composer/2.0"` set so `load_package` routes through
//! `expand_minified` before `PackageVersion::from_value`.

use anyhow::Result;
use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use reqwest::Url;
use serde_json::json;
use vivace::fetch::Conditional;
use vivace::repository::{DevAcceptance, Repository, Transport};

struct FuzzTransport {
    provider_body: Vec<u8>,
}

impl Transport for FuzzTransport {
    async fn get(&self, url: &Url, _if_modified_since: Option<&str>) -> Result<Conditional> {
        let body = if url.path().ends_with("packages.json") {
            br#"{"metadata-url":"/p2/%package%.json"}"#.to_vec()
        } else {
            self.provider_body.clone()
        };
        Ok(Conditional::Fresh {
            body,
            last_modified: None,
        })
    }
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let len = u.int_in_range(0..=6).unwrap_or(0);
    let mut versions = Vec::with_capacity(len);
    for _ in 0..len {
        let Ok(value) = vivace_fuzz::arbitrary_value(&mut u, 2) else {
            return;
        };
        versions.push(value);
    }
    let provider_body = json!({
        "packages": { "fuzz/pkg": versions },
        "minified": "composer/2.0",
    })
    .to_string()
    .into_bytes();

    let Ok(cache_root) = tempfile::tempdir() else {
        return;
    };
    // A Tokio runtime, not `futures::executor::block_on`: the parse this
    // target is aimed at runs on `spawn_blocking` (`parse_provider_versions`),
    // which panics with "there is no reactor running" under any other
    // executor — on every input, empty ones included.
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().build() else {
        return;
    };
    runtime.block_on(async {
        let transport = FuzzTransport { provider_body };
        let Ok(repo) = Repository::load("https://fuzz.example/", cache_root.path(), transport)
            .await
        else {
            return;
        };
        let _ = repo.load_package("fuzz/pkg", DevAcceptance::NonDevOnly).await;
    });
});
