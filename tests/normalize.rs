//! `viv normalize`: golden pairs against `tests/fixtures/normalize/` (each
//! `<name>.json` normalizing byte-for-byte to `<name>.expected.json`, per
//! the real `ergebnis/composer-normalize`), plus the CLI surface (`--check`,
//! exit codes, no-op).

#[macro_use]
mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/normalize")
}

/// Every `<name>.json`/`<name>.expected.json` pair under `tests/fixtures/normalize/`.
fn golden_names() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(fixtures())
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(".expected.json").map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

#[test]
fn goldens_normalize_byte_for_byte() {
    let names = golden_names();
    assert!(
        !names.is_empty(),
        "no fixtures found under {}",
        fixtures().display()
    );
    for name in names {
        let ctx = TestContext::new();
        fs::copy(
            fixtures().join(format!("{name}.json")),
            ctx.project.path().join("composer.json"),
        )
        .unwrap();
        let mut cmd = ctx.viv();
        let output = cmd.arg("normalize").output().expect("failed to run viv");
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let expected = fs::read(fixtures().join(format!("{name}.expected.json"))).unwrap();
        let actual = fs::read(ctx.project.path().join("composer.json")).unwrap();
        assert!(
            actual == expected,
            "{name}: normalized output did not match the golden"
        );
    }
}

#[test]
fn normalize_is_idempotent() {
    let ctx = TestContext::new();
    fs::copy(
        fixtures().join("one.expected.json"),
        ctx.project.path().join("composer.json"),
    )
    .unwrap();
    let mut cmd = ctx.viv();
    cmd.arg("normalize");
    viv_snapshot!(ctx, cmd);

    let content = fs::read(ctx.project.path().join("composer.json")).unwrap();
    let expected = fs::read(fixtures().join("one.expected.json")).unwrap();
    assert_eq!(
        content, expected,
        "an already-normalized file must not change"
    );
}

#[test]
fn check_passes_on_an_already_normalized_file() {
    let ctx = TestContext::new();
    fs::copy(
        fixtures().join("one.expected.json"),
        ctx.project.path().join("composer.json"),
    )
    .unwrap();
    let mut cmd = ctx.viv();
    cmd.args(["normalize", "--check"]);
    viv_snapshot!(ctx, cmd);
}

#[test]
fn check_fails_with_a_diff_on_a_messy_file_and_does_not_write() {
    let ctx = TestContext::new();
    fs::copy(
        fixtures().join("one.json"),
        ctx.project.path().join("composer.json"),
    )
    .unwrap();
    let before = fs::read(ctx.project.path().join("composer.json")).unwrap();

    let mut cmd = ctx.viv();
    cmd.args(["normalize", "--check"]);
    viv_snapshot!(ctx, cmd);

    let after = fs::read(ctx.project.path().join("composer.json")).unwrap();
    assert_eq!(before, after, "--check must not write");
}

/// #375: the rewritten `composer.json` carries the umask mode, not the `0600`
/// of the temp file it is renamed from.
#[test]
fn rewritten_composer_json_carries_the_umask_mode() {
    use std::os::unix::fs::PermissionsExt as _;

    let Some(want) = common::umask_file_mode() else {
        return;
    };
    let ctx = TestContext::new();
    let path = ctx.project.path().join("composer.json");
    fs::copy(fixtures().join("one.json"), &path).unwrap();

    ctx.viv().arg("normalize").assert().success();

    let got = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(got, want, "composer.json: {got:o} vs umask {want:o}");
}

/// park#405: like Composer's `file_put_contents`, a rewrite keeps the mode
/// of the `composer.json` it replaces.
#[test]
fn rewritten_composer_json_keeps_its_existing_mode() {
    use std::os::unix::fs::PermissionsExt as _;

    let ctx = TestContext::new();
    let path = ctx.project.path().join("composer.json");
    fs::copy(fixtures().join("one.json"), &path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

    ctx.viv().arg("normalize").assert().success();

    let got = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(got, 0o640, "composer.json: {got:o}");
}

#[test]
fn indent_size_is_configurable() {
    let ctx = TestContext::new();
    fs::write(
        ctx.project.path().join("composer.json"),
        r#"{"name":"a/b"}"#,
    )
    .unwrap();
    let mut cmd = ctx.viv();
    cmd.args(["normalize", "--indent-size", "2"]);
    let output = cmd.output().expect("failed to run viv");
    assert!(output.status.success());

    let content = fs::read_to_string(ctx.project.path().join("composer.json")).unwrap();
    assert_eq!(content, "{\n  \"name\": \"a/b\"\n}\n");
}
