#![no_main]

//! The hand-written balanced-JSON scanner behind `viv require`/`viv remove`
//! (`require.rs`'s `manipulator` module: `value_end`, `span_of_key`,
//! `top_level_entries`, all `pub(super)` or private) fuzzed through the
//! nearest public entry point, [`vivace::require::run_require`], with
//! `--no-update` so it only reads, edits and rewrites `composer.json` (no
//! network, no solver).
//!
//! Property: whenever `run_require` succeeds, the rewritten file still
//! parses as JSON, and every top-level key other than `require`/
//! `require-dev` (the two the manipulator is allowed to touch) round-trips
//! unchanged — a scanner bug would corrupt or drop an untouched key without
//! ever panicking, so this checks output shape, not just "didn't crash".

use libfuzzer_sys::fuzz_target;
use serde_json::Value;
use vivace::require::{RequireArgs, run_require};

fuzz_target!(|data: &[u8]| {
    // A non-object or invalid-JSON input can't get past `run_require`'s own
    // `serde_json::from_str` before the manipulator ever runs; skip early
    // rather than pay for a tempdir on every such draw.
    let Ok(original) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(before) = serde_json::from_str::<Value>(original) else {
        return;
    };
    if !before.is_object() {
        return;
    }

    let Ok(project_dir) = tempfile::tempdir() else {
        return;
    };
    let composer_json_path = project_dir.path().join("composer.json");
    if fs_err::write(&composer_json_path, data).is_err() {
        return;
    }

    let args = RequireArgs {
        packages: vec!["fuzz/pkg:1.0.0".to_string()],
        dev: false,
        no_update: true,
        sort_packages: false,
        prefer_lowest: false,
        prefer_stable: false,
        project_dir: project_dir.path().to_path_buf(),
    };
    if run_require(&args, None).is_err() {
        return;
    }

    let after_bytes = fs_err::read(&composer_json_path).expect("run_require wrote the file back");
    let after: Value =
        serde_json::from_slice(&after_bytes).expect("run_require must leave valid JSON behind");
    let before_obj = before.as_object().expect("checked above");
    let after_obj = after.as_object().expect("run_require must keep a JSON object");
    for (key, value) in before_obj {
        if key == "require" || key == "require-dev" {
            continue;
        }
        assert_eq!(
            after_obj.get(key),
            Some(value),
            "run_require changed untouched key {key:?}"
        );
    }
});
