#![no_main]

//! `find_classes` parses one PHP file's bytes without executing it
//! (composer/class-map-generator's `PhpFileParser`/`PhpFileCleaner` port).
//! It's a public, self-contained entry point (no filesystem, no state), so
//! it's fuzzed directly rather than through the (also public) `scan_paths`.

use libfuzzer_sys::fuzz_target;
use vivace::autoload::classmap::find_classes;

fuzz_target!(|data: &[u8]| {
    let _ = find_classes(data);
});
