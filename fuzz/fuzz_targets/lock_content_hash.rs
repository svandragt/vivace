#![no_main]

//! `content_hash`/`php_json_encode` (both private in `lock.rs`) fuzzed
//! through the nearest public entry point, [`vivace::lock::is_fresh`]:
//! it calls `content_hash(root_json)` whenever the `Lock` carries a
//! `content-hash` to compare against, which `php_json_encode`'s an
//! arbitrary `serde_json::Value` nested under `extra` (one of the keys
//! `content_hash` folds into the encoded blob).

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use serde_json::json;
use vivace::lock::{Lock, is_fresh};

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let Ok(value) = vivace_fuzz::arbitrary_value(&mut u, 4) else {
        return;
    };
    let root_json = json!({ "extra": value }).to_string();
    let lock = Lock {
        content_hash: Some("fuzz-placeholder".to_string()),
        packages: Vec::new(),
        aliases: Vec::new(),
    };
    let _ = is_fresh(&lock, root_json.as_bytes());
});
