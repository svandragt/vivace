//! `viv diagnose`: environment and configuration report (#81). Machine- and
//! host-dependent lines (PHP/git/Composer versions, cache byte counts) rule
//! out a snapshot, so these assert on the handful of lines the report
//! promises rather than the whole output.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn phpstan_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/phpstan")
}

#[test]
fn empty_project_reports_project_none_and_the_environment_sections() {
    let ctx = TestContext::new();
    let mut cmd = ctx.viv();
    cmd.arg("diagnose");
    let output = cmd.output().expect("failed to run viv diagnose");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("project: none"), "{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("viv: ")), "{stdout}");
    assert!(
        stdout.lines().any(|l| l.starts_with("cache-dir: ")),
        "{stdout}"
    );
    assert!(stdout.lines().any(|l| l.starts_with("php: ")), "{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("git: ")), "{stdout}");
    assert!(
        stdout.lines().any(|l| l.starts_with("composer: ")),
        "{stdout}"
    );
}

/// A project with a `composer-plugin` in its lock lists the decision viv
/// would make for it, and an `auth.json` token planted in the project
/// directory shows up only as a host name, never the token value.
#[test]
fn project_with_a_plugin_reports_its_decision_and_never_leaks_a_token() {
    let ctx = TestContext::new();
    for name in ["composer.json", "composer.lock"] {
        fs::copy(phpstan_fixture().join(name), ctx.project.path().join(name)).unwrap();
    }
    fs::write(
        ctx.project.path().join("auth.json"),
        r#"{"github-oauth": {"github.com": "sekrit-token"}}"#,
    )
    .unwrap();

    let mut cmd = ctx.viv();
    cmd.arg("diagnose");
    let output = cmd.output().expect("failed to run viv diagnose");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("plugins: phpstan/extension-installer:")),
        "{stdout}"
    );
    assert!(!stdout.contains("sekrit-token"), "{stdout}");
    assert!(stdout.contains("github.com"), "{stdout}");
}

/// The hidden `--adapters` flag (#127 part 3's drift workflow,
/// `.github/workflows/adapter-drift.yml`): one `<name>\t<version>` line per
/// plugin name in `NATIVE_ADAPTERS` registration order, and nothing else.
#[test]
fn diagnose_adapters_prints_every_native_adapter_and_nothing_else() {
    let ctx = TestContext::new();
    let mut cmd = ctx.viv();
    cmd.args(["diagnose", "--adapters"]);
    let output = cmd.output().expect("failed to run viv diagnose --adapters");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let names: Vec<&str> = stdout
        .lines()
        .map(|line| {
            let (name, version) = line
                .split_once('\t')
                .unwrap_or_else(|| panic!("not a name\\tversion line: {line}"));
            assert!(!version.is_empty(), "{line}");
            name
        })
        .collect();

    assert_eq!(
        names,
        [
            "composer/installers",
            "johnpbloch/wordpress-core-installer",
            "roots/wordpress-core-installer",
            "dealerdirect/phpcodesniffer-composer-installer",
            "phpstan/extension-installer",
            "tbachert/spi",
            "cweagans/composer-patches",
            "php-http/discovery",
            "yiisoft/yii2-composer",
            "craftcms/plugin-installer",
            "ffraenz/private-composer-installer",
            "codeception/c3",
            "drupal/core-composer-scaffold",
            "symfony/runtime",
        ]
    );
}
