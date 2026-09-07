//! `viv audit` (#68): a native `Auditor`/`AuditCommand` port, byte-diffed
//! against Composer 2.10.2's own `audit --no-ansi` output. Hermetic against
//! a recorded `packagist.org/api/security-advisories/` response under
//! `tests/fixtures/audit/<scenario>/`, the same `Transport`-seam shape
//! `tests/repository.rs` uses for `/p2/` metadata.

use std::path::{Path, PathBuf};

use serde_json::Value;
use vivace::audit::{AdvisoriesTransport, AuditArgs, Format, audit, load_packages};
use vivace::lock::Config;

fn fixture(scenario: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audit")
        .join(scenario)
}

/// Serves a scenario's recorded `advisories-response.json` instead of
/// posting to Packagist, and counts calls so a test can assert exactly one
/// POST is made regardless of how many packages are audited.
struct FixtureTransport {
    body: Value,
    calls: std::sync::Mutex<usize>,
}

impl FixtureTransport {
    fn new(scenario: &str) -> Self {
        let body =
            fs_err::read_to_string(fixture(scenario).join("advisories-response.json")).unwrap();
        FixtureTransport {
            body: serde_json::from_str(&body).unwrap(),
            calls: std::sync::Mutex::new(0),
        }
    }
}

impl AdvisoriesTransport for FixtureTransport {
    // The fixture transport reads an in-memory body synchronously; no
    // `.await` is needed, but the trait signature is async for production.
    #[allow(clippy::unused_async_trait_impl)]
    async fn post_advisories(&self, _packages: &[String]) -> anyhow::Result<Value> {
        *self.calls.lock().unwrap() += 1;
        Ok(self.body.clone())
    }
}

fn args(locked: bool, format: Format) -> AuditArgs {
    AuditArgs {
        no_dev: false,
        format,
        locked,
        abandoned: None,
        ignore_severity: Vec::new(),
        project_dir: PathBuf::from("."),
    }
}

fn expected(scenario: &str, subdir: &str, format: &str) -> String {
    fs_err::read_to_string(fixture(scenario).join(subdir).join(format)).unwrap()
}

/// One vulnerable package (`monolog/monolog` 1.10.0, `PKSA-dmw8-jd8k-q3c6`)
/// reproduces Composer's exact bytes and exit code in every format.
#[test]
fn vulnerable_lock_matches_composer_every_format() {
    let project_dir = fixture("vulnerable");
    let config = Config::default();
    let packages = load_packages(&args(true, Format::Table), &project_dir, &config, true).unwrap();
    assert_eq!(packages.len(), 2, "monolog/monolog + psr/log");

    for (format, name) in [
        (Format::Table, "table.out"),
        (Format::Plain, "plain.out"),
        (Format::Json, "json.out"),
        (Format::Summary, "summary.out"),
    ] {
        let transport = FixtureTransport::new("vulnerable");
        let cli_args = args(true, format);
        let (status, rendered) =
            futures::executor::block_on(audit(&cli_args, &packages, &config, &transport)).unwrap();
        assert_eq!(status, 1, "{name}: an active advisory exits 1");
        assert_eq!(
            format!("{rendered}\n"),
            expected("vulnerable", "expected", name),
            "{name}"
        );
        assert_eq!(*transport.calls.lock().unwrap(), 1, "{name}: one POST");
    }
}

/// `config.audit.ignore` (a map with a reason) turns the same advisory into
/// an ignored one: exit 0, and an "Ignore reason" row/field in every format.
#[test]
fn config_audit_ignore_exits_clean_with_reason() {
    let project_dir = fixture("vulnerable");
    let mut config = Config::default();
    config.audit.ignore = vivace::lock::AuditIgnore::Map(
        [(
            "PKSA-dmw8-jd8k-q3c6".to_string(),
            Some("we accept this risk".to_string()),
        )]
        .into_iter()
        .collect(),
    );
    let packages = load_packages(&args(true, Format::Table), &project_dir, &config, true).unwrap();

    for (format, name) in [
        (Format::Table, "table.out"),
        (Format::Plain, "plain.out"),
        (Format::Json, "json.out"),
        (Format::Summary, "summary.out"),
    ] {
        let transport = FixtureTransport::new("vulnerable");
        let cli_args = args(true, format);
        let (status, rendered) =
            futures::executor::block_on(audit(&cli_args, &packages, &config, &transport)).unwrap();
        assert_eq!(status, 0, "{name}: ignored-only exits 0");
        assert_eq!(
            format!("{rendered}\n"),
            expected("vulnerable", "expected-ignored", name),
            "{name}"
        );
    }
}

/// `--ignore-severity` ignores by severity instead of by ID, with its own
/// "<severity> severity is ignored" reason.
#[test]
fn ignore_severity_flag_ignores_by_severity() {
    let project_dir = fixture("vulnerable");
    let config = Config::default();
    let packages = load_packages(&args(true, Format::Plain), &project_dir, &config, true).unwrap();
    let transport = FixtureTransport::new("vulnerable");
    let mut cli_args = args(true, Format::Plain);
    cli_args.ignore_severity = vec!["low".to_string()];
    let (status, rendered) =
        futures::executor::block_on(audit(&cli_args, &packages, &config, &transport)).unwrap();
    assert_eq!(status, 0);
    assert_eq!(
        format!("{rendered}\n"),
        expected("vulnerable", "expected-ignored-severity", "plain.out")
    );
}

/// No advisory matches (`psr/log` 1.1.4 has none): exit 0, "No security
/// vulnerability advisories found." in every non-JSON format, `[]` in JSON.
#[test]
fn clean_lock_reports_no_advisories() {
    let project_dir = fixture("clean");
    let config = Config::default();
    let packages = load_packages(&args(true, Format::Table), &project_dir, &config, true).unwrap();
    assert_eq!(packages.len(), 1);

    for (format, name) in [
        (Format::Table, "table.out"),
        (Format::Plain, "plain.out"),
        (Format::Json, "json.out"),
        (Format::Summary, "summary.out"),
    ] {
        let transport = FixtureTransport::new("clean");
        let cli_args = args(true, format);
        let (status, rendered) =
            futures::executor::block_on(audit(&cli_args, &packages, &config, &transport)).unwrap();
        assert_eq!(status, 0, "{name}");
        assert_eq!(
            format!("{rendered}\n"),
            expected("clean", "expected", name),
            "{name}"
        );
    }
}

/// An abandoned package (`swiftmailer/swiftmailer` -> `symfony/mailer`)
/// fails by default (`config.audit.abandoned` defaults to `fail`), and can
/// be silenced with `--abandoned=ignore`.
#[test]
fn abandoned_package_fails_by_default_and_can_be_ignored() {
    let project_dir = fixture("abandoned");
    let config = Config::default();
    let packages = load_packages(&args(true, Format::Table), &project_dir, &config, true).unwrap();

    for (format, name) in [
        (Format::Table, "table.out"),
        (Format::Plain, "plain.out"),
        (Format::Json, "json.out"),
        (Format::Summary, "summary.out"),
    ] {
        let transport = FixtureTransport::new("abandoned");
        let cli_args = args(true, format);
        let (status, rendered) =
            futures::executor::block_on(audit(&cli_args, &packages, &config, &transport)).unwrap();
        assert_eq!(status, 1, "{name}: abandoned fails by default");
        assert_eq!(
            format!("{rendered}\n"),
            expected("abandoned", "expected", name),
            "{name}"
        );
    }

    let transport = FixtureTransport::new("abandoned");
    let mut cli_args = args(true, Format::Plain);
    cli_args.abandoned = Some("ignore".to_string());
    let (status, rendered) =
        futures::executor::block_on(audit(&cli_args, &packages, &config, &transport)).unwrap();
    assert_eq!(status, 0, "--abandoned=ignore silences it");
    assert_eq!(rendered, "No security vulnerability advisories found.");
}
