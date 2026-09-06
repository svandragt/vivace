#![no_main]

//! `parse_constraint` is public and takes a plain `&str`; fuzzed directly.

use libfuzzer_sys::fuzz_target;
use vivace::semver::parse_constraint;

fuzz_target!(|data: &str| {
    let _ = parse_constraint(data);
});
