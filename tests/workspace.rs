//! `viv workspace list` (#276): discovers workspace members from
//! `extra.viv.workspace.members` and reports which other members each one
//! requires. `src/workspace.rs`'s own `tests` module covers every refusal
//! case directly against `discover`; this only exercises the CLI surface
//! against `tests/fixtures/workspace/basic`, a root with three members, one
//! requiring another.

use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace/basic")
}

fn run() -> Output {
    Command::new(cargo_bin("viv"))
        .arg("workspace")
        .arg("list")
        .arg("-d")
        .arg(fixture())
        .output()
        .expect("failed to run viv")
}

#[test]
fn lists_every_member_and_the_inter_member_edge() {
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "expected all three members: {stdout}");

    // Sorted by name: acme/bar, acme/baz, acme/foo.
    assert_eq!(lines[0], "acme/bar 1.2.0 packages/bar acme/foo");
    assert_eq!(lines[1], "acme/baz 0.9.0 packages/baz -");
    assert_eq!(lines[2], "acme/foo 1.0.0 packages/foo -");
}
