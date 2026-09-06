#![no_main]

//! Zip extraction, path sanitisation and the single-top-directory strip
//! (`Store::extract_archive`/`extract_zip`/`sanitise`, all private) fuzzed
//! through the nearest public entry point, `Store::add_archive`. A fixed
//! `Package` supplies the `dist.type = "zip"` the store needs to pick an
//! extractor; only the archive bytes vary per input.
//!
//! No size caps exist yet on the extractor (`extract_zip`'s own doc comment:
//! "no cap on inflated size"), so the input itself is capped at 1 MiB here
//! to keep a zip-bomb corpus entry from filling the fuzzing disk.

use libfuzzer_sys::fuzz_target;
use serde_json::json;
use vivace::lock::Package;
use vivace::store::Store;

const MAX_INPUT: usize = 1 << 20; // 1 MiB

fn fixed_package() -> Package {
    serde_json::from_value(json!({
        "name": "fuzz/pkg",
        "version": "1.0.0",
        "dist": { "type": "zip", "url": "file:///fuzz.zip", "reference": null },
    }))
    .expect("fixed fixture package parses")
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT {
        return;
    }
    // A fresh store per run: `add_archive` keys extracted trees by the
    // archive's content hash, so re-running the same corpus never collides,
    // but a shared root across the whole fuzzing run would grow unbounded.
    let Ok(root) = tempfile::tempdir() else {
        return;
    };
    let Ok(store) = Store::open(root.path()) else {
        return;
    };
    let pkg = fixed_package();
    let _ = store.add_archive(&pkg, data);
});
