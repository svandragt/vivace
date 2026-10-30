//! `viv php install`/`viv php list` (#337): a local server stands in for
//! `dl.static-php.dev` (`VIV_PHP_DIST_URL`, test-only), serving a `bulk/`
//! directory listing and a tiny tar.gz whose one entry is a `php` file.

mod common;

use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};

use common::{TestContext, install_fake_pinned_php, write_fake_php};
use predicates::prelude::PredicateBooleanExt as _;
use sha2::{Digest, Sha256};

/// A single-entry (`php`) tar.gz, gzip-compressed the same way
/// `flate2`/`tar` (both already dependencies) build one.
fn build_php_tarball(content: &[u8]) -> Vec<u8> {
    let mut header = tar::Header::new_gnu();
    header.set_path("php").unwrap();
    header.set_size(content.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        builder.append(&header, content).unwrap();
        builder.finish().unwrap();
    }
    let mut gz_bytes = Vec::new();
    {
        let mut encoder =
            flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap();
    }
    gz_bytes
}

/// Answers every request on a loop (`listing_path` -> `listing_html`,
/// `tarball_path` -> `tarball_bytes`, anything else -> 404), and records
/// each request's path — `spawn_dist_server` in
/// `tests/plugins_private_c3.rs` is the same raw-`TcpListener` shape for one
/// request; this loops instead of returning after the first, since a real
/// install makes two (listing, then tarball) and re-pinning the same
/// floating minor legitimately re-checks the listing (#337's issue: "a
/// second project pinning 8.4 downloads nothing", not "makes no request at
/// all") while the tarball itself must not be re-fetched.
fn spawn_php_dist_server(
    listing_path: &'static str,
    listing_html: String,
    tarball_path: String,
    tarball_bytes: Vec<u8>,
) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&request);
            let path = text
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("")
                .to_string();
            recorded.lock().unwrap().push(path.clone());
            let (status, body): (&str, &[u8]) = if path == listing_path {
                ("200 OK", listing_html.as_bytes())
            } else if path == tarball_path {
                ("200 OK", tarball_bytes.as_slice())
            } else {
                ("404 Not Found", b"")
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    (addr, requests)
}

#[test]
fn install_downloads_extracts_pins_and_is_cached_on_a_second_run() {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let tarball_bytes = build_php_tarball(b"#!/bin/sh\necho fake-php\n");
    let listing_html = format!(
        "<a href=\"php-8.4.5-cli-{os}-{arch}.tar.gz\">php-8.4.5-cli-{os}-{arch}.tar.gz</a>\n\
         <a href=\"php-8.4.17-cli-{os}-{arch}.tar.gz\">php-8.4.17-cli-{os}-{arch}.tar.gz</a>\n"
    );
    let tarball_path = format!("/php-8.4.17-cli-{os}-{arch}.tar.gz");
    let (addr, requests) =
        spawn_php_dist_server("/", listing_html, tarball_path.clone(), tarball_bytes);
    let dist_url = format!("http://{addr}");

    let ctx = TestContext::new();
    fs_err::write(
        ctx.project.path().join("composer.json"),
        b"{\n    \"name\": \"acme/pkg\",\n    \"config\": { \"platform\": { \"php\": \"8.1.0\" } },\n    \"extra\": { \"viv\": { \"isolate\": [\"acme/plugin\"] } }\n}\n",
    )
    .unwrap();

    ctx.viv()
        .env("VIV_PHP_DIST_URL", &dist_url)
        .arg("php")
        .arg("install")
        .arg("8.4")
        .assert()
        .success()
        .stdout(predicates::str::contains(format!(
            "installed php 8.4.17 ({os}-{arch}) to"
        )));

    let tarball_requests = |requests: &Mutex<Vec<String>>| {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|path| **path == tarball_path)
            .count()
    };
    assert_eq!(tarball_requests(&requests), 1, "one tarball download");

    let version_dir = ctx
        .cache
        .path()
        .join("php-v0")
        .join(format!("8.4.17-{os}-{arch}"));
    let php_path = version_dir.join("php");
    assert!(php_path.is_file(), "{}", php_path.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs_err::metadata(&php_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    let recorded_sha256 = fs_err::read_to_string(version_dir.join(".sha256")).unwrap();
    // Re-derive the tarball the server sent, byte for byte (same content,
    // so the same bytes), instead of hard-coding a digest that would
    // silently go stale the moment `build_php_tarball` changes.
    let digest = Sha256::digest(build_php_tarball(b"#!/bin/sh\necho fake-php\n"));
    let expected_sha256 = digest.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    });
    assert_eq!(recorded_sha256, expected_sha256);
    assert!(version_dir.join(".ok").is_file());

    let composer_json: serde_json::Value =
        serde_json::from_slice(&fs_err::read(ctx.project.path().join("composer.json")).unwrap())
            .unwrap();
    // #397: the version goes in `extra.viv.php`, not `config.platform.php`
    // (which keeps whatever the project resolves for), and the other
    // `extra.viv` keys stay.
    assert_eq!(composer_json["extra"]["viv"]["php"], "8.4");
    assert_eq!(composer_json["config"]["platform"]["php"], "8.1.0");
    assert_eq!(
        composer_json["extra"]["viv"]["isolate"],
        serde_json::json!(["acme/plugin"])
    );

    // Second run: no `VERSION` needed thanks to the pin just written
    // ("8.4", a floating minor — re-checking the listing for its current
    // newest patch is expected and cheap); the tarball itself must not be
    // downloaded again.
    ctx.viv()
        .env("VIV_PHP_DIST_URL", &dist_url)
        .arg("php")
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("php 8.4.17 already installed"));
    assert_eq!(
        tarball_requests(&requests),
        1,
        "a second project pinning 8.4 downloads nothing (issue #337's own done-when)"
    );

    let list_output = ctx.viv().arg("php").arg("list").output().unwrap();
    assert!(list_output.status.success());
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert_eq!(stdout.trim(), format!("8.4.17 {os}-{arch}"));
}

#[test]
fn install_with_no_version_and_no_pin_errors() {
    let ctx = TestContext::new();
    fs_err::write(
        ctx.project.path().join("composer.json"),
        b"{\n    \"name\": \"acme/pkg\",\n    \"config\": { \"platform\": { \"php\": \"8.4\" } }\n}\n",
    )
    .unwrap();

    ctx.viv().arg("php").arg("install").assert().failure().stderr(
        predicates::str::contains(
            "no PHP version set: set extra.viv.php in composer.json or run viv php install <version>",
        ),
    );
}

/// #385's setup: a 8.4 build in the cache whose probe reports `dom`, plus a
/// different `php` first on `PATH` that does not. The returned value is the
/// `PATH` to run `viv` with.
fn build_with_dom_and_a_path_php_without(ctx: &TestContext) -> std::ffi::OsString {
    install_fake_pinned_php(ctx.cache.path(), "8.4.7", r#"{"dom":"8.4.7"}"#);

    let path_bin = ctx.cache.path().join("path-bin");
    write_fake_php(&path_bin.join("php"), "8.3.1", "{}");
    let mut paths = vec![path_bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap()
}

/// `php` as set through `extra.viv.php` (#397), which selects the build.
const VIV_PHP: &str = r#""extra": { "viv": { "php": "8.4" } }"#;
/// `php` as set through `config.platform.php`, which only feeds the solver.
const PLATFORM_PHP: &str = r#""config": { "platform": { "php": "8.4" } }"#;

/// `ext-dom` required at the root, `php_setting` (one of the constants above)
/// and no packages, so no network is needed.
fn write_ext_dom_project(project: &std::path::Path, php_setting: &str) {
    fs_err::write(
        project.join("composer.json"),
        format!(
            "{{\n    \"require\": {{ \"ext-dom\": \"*\" }},\n    {php_setting},\n    \
             \"repositories\": [{{ \"packagist.org\": false }}]\n}}\n"
        ),
    )
    .unwrap();
}

/// The `ext-*` check of `viv update` reads the extensions of the build
/// `extra.viv.php` selects, not the `php` on `PATH` (#385, #397).
#[test]
fn update_checks_extensions_against_the_extra_viv_php_build() {
    let ctx = TestContext::new();
    let path = build_with_dom_and_a_path_php_without(&ctx);
    write_ext_dom_project(ctx.project.path(), VIV_PHP);

    ctx.viv().env("PATH", path).arg("update").assert().success();
    assert!(ctx.project.path().join("composer.lock").is_file());
}

/// Same for the lock-verify step of `viv install` (#385, #397).
#[test]
fn install_checks_extensions_against_the_extra_viv_php_build() {
    let ctx = TestContext::new();
    let path = build_with_dom_and_a_path_php_without(&ctx);
    write_ext_dom_project(ctx.project.path(), VIV_PHP);
    ctx.viv()
        .env("PATH", &path)
        .args(["update", "--ignore-platform-reqs"])
        .assert()
        .success();

    ctx.viv()
        .env("PATH", path)
        .arg("install")
        .assert()
        .success();
}

/// With neither key, the check keeps reading the `PATH` php: the same project
/// minus the setting still refuses `ext-dom` (#385 must not change that).
#[test]
fn update_without_a_php_setting_still_checks_the_path_php() {
    let ctx = TestContext::new();
    let path = build_with_dom_and_a_path_php_without(&ctx);
    fs_err::write(
        ctx.project.path().join("composer.json"),
        "{\n    \"require\": { \"ext-dom\": \"*\" },\n    \"repositories\": [{ \"packagist.org\": \
         false }]\n}\n",
    )
    .unwrap();

    ctx.viv()
        .env("PATH", path)
        .arg("update")
        .assert()
        .failure()
        .stderr(predicates::str::contains("ext-dom"))
        .stderr(predicates::str::contains(
            "Platform packages were read from the php on PATH.",
        ))
        .stderr(predicates::str::contains("extra.viv.php").not());
}

/// #397, the Lamb case: `config.platform.php` only feeds the solver, so
/// `update` refuses `ext-dom` against the `PATH` php, which lacks it, even
/// with a build that has it installed, and says nothing about a build.
#[test]
fn update_reads_the_path_php_for_config_platform_php() {
    let ctx = TestContext::new();
    let path = build_with_dom_and_a_path_php_without(&ctx);
    write_ext_dom_project(ctx.project.path(), PLATFORM_PHP);

    ctx.viv()
        .env("PATH", path)
        .arg("update")
        .assert()
        .failure()
        .stderr(predicates::str::contains("ext-dom"))
        .stderr(predicates::str::contains(
            "Platform packages were read from the php on PATH.",
        ))
        .stderr(predicates::str::contains("extra.viv.php").not());
}

/// With neither key and no `php` on `PATH`, the refusal says how to get one
/// (#397).
#[test]
fn update_with_no_php_and_no_php_setting_hints_at_extra_viv_php() {
    let ctx = TestContext::new();
    let empty_path = tempfile::tempdir().unwrap();
    fs_err::write(
        ctx.project.path().join("composer.json"),
        "{\n    \"require\": { \"ext-dom\": \"*\" },\n    \"repositories\": [{ \"packagist.org\": \
         false }]\n}\n",
    )
    .unwrap();

    ctx.viv()
        .env("PATH", empty_path.path())
        .arg("update")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "No php binary was found on PATH, so viv assumed php 8.3.0 with no extensions. Set \
             extra.viv.php or run `viv php install <version>`.",
        ));
}

/// A dist server (`VIV_PHP_DIST_URL`) offering php 8.4.7, whose probe reports
/// `dom`: the build `viv` has to download when `extra.viv.php` names it.
fn spawn_fake_php_dist_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    // Reads stdin with `read`, not `cat`: the test runs it with no tools on `PATH`.
    let tarball = build_php_tarball(
        b"#!/bin/sh\nif [ \"$1\" = \"-r\" ]; then printf 8.4.7; exit 0; fi\n\
          while read -r _; do :; done\n\
          printf '{\"php_version\":\"8.4.7\",\"int_size\":8,\"extensions\":{\"dom\":\"8.4.7\"}}'\n",
    );
    let listing = format!(
        "<a href=\"php-8.4.7-cli-{os}-{arch}.tar.gz\">php-8.4.7-cli-{os}-{arch}.tar.gz</a>\n"
    );
    let (addr, requests) = spawn_php_dist_server(
        "/",
        listing,
        format!("/php-8.4.7-cli-{os}-{arch}.tar.gz"),
        tarball,
    );
    (format!("http://{addr}"), requests)
}

/// One inline package, `acme/widget`, that needs `ext-dom`, `pin` as
/// `extra.viv.php`, and a lock written without any platform check (a partial
/// update needs one); no network is needed to solve it.
fn write_widget_project(ctx: &TestContext, pin: &str) {
    fs_err::write(
        ctx.project.path().join("composer.json"),
        format!(
            r#"{{
    "require": {{ "acme/widget": "1.0.0" }},
    "extra": {{ "viv": {{ "php": "{pin}" }} }},
    "repositories": [
        {{ "packagist.org": false }},
        {{ "type": "package", "package": {{
            "name": "acme/widget", "version": "1.0.0", "require": {{ "ext-dom": "*" }},
            "dist": {{ "type": "zip", "url": "https://example.invalid/widget.zip" }}
        }} }}
    ]
}}
"#
        ),
    )
    .unwrap();
    ctx.viv()
        .args([
            "update",
            "--no-install",
            "--offline",
            "--ignore-platform-reqs",
        ])
        .assert()
        .success();
}

/// #392: with no `php` on `PATH` and the `extra.viv.php` build not yet
/// downloaded, `viv update <pkg>` installs the build (as `viv run` does) and
/// checks `ext-*` against it, instead of assuming a php with no extensions.
#[test]
fn update_installs_a_missing_extra_viv_php_build_and_checks_extensions_against_it() {
    let (dist_url, _requests) = spawn_fake_php_dist_server();
    let ctx = TestContext::new();
    let empty_path = tempfile::tempdir().unwrap();
    write_widget_project(&ctx, "8.4");

    ctx.viv()
        .env("PATH", empty_path.path())
        .env("VIV_PHP_DIST_URL", &dist_url)
        .args(["update", "acme/widget", "--no-install"])
        .assert()
        .success()
        .stderr(predicates::str::contains("installing php 8.4.7"));

    let marker = ctx
        .cache
        .path()
        .join("php-v0")
        .join(format!(
            "8.4.7-{}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
        .join(".ok");
    assert!(marker.is_file(), "{}", marker.display());
    assert!(ctx.project.path().join("composer.lock").is_file());
}

/// #392: `--offline` never installs the build, so the check keeps falling
/// back to `PATH` and refuses, naming how to get the build.
#[test]
fn update_offline_does_not_install_a_missing_extra_viv_php_build() {
    let (dist_url, requests) = spawn_fake_php_dist_server();
    let ctx = TestContext::new();
    let empty_path = tempfile::tempdir().unwrap();
    write_widget_project(&ctx, "8.4");

    let output = ctx
        .viv()
        .env("PATH", empty_path.path())
        .env("VIV_PHP_DIST_URL", &dist_url)
        .args(["update", "acme/widget", "--no-install", "--offline"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("ext-dom"), "{stderr}");
    assert!(
        stderr.contains("extra.viv.php names php 8.4, which is not installed"),
        "{stderr}"
    );
    assert!(!stderr.contains("installing php"), "{stderr}");
    assert!(
        requests.lock().unwrap().is_empty(),
        "no request to the dist server"
    );
}

/// #392: a build that cannot be downloaded fails the command and names the
/// version, instead of checking against a different `php` than the project
/// asked for.
#[test]
fn update_fails_naming_the_version_when_the_extra_viv_php_build_cannot_be_installed() {
    let ctx = TestContext::new();
    let path = build_with_dom_and_a_path_php_without(&ctx);
    write_widget_project(&ctx, "8.5");

    let output = ctx
        .viv()
        .env("PATH", path)
        .env("VIV_PHP_DIST_URL", "http://127.0.0.1:9")
        .args(["update", "acme/widget", "--no-install"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("could not install php 8.5, which extra.viv.php names"),
        "{stderr}"
    );
    assert!(!stderr.contains("Platform packages were read"), "{stderr}");
}

/// With `--offline`, an `extra.viv.php` with no installed build (here 8.5;
/// only 8.4 is cached) neither installs anything nor fails the check: it reads
/// the `PATH` php and names how to get the build.
#[test]
fn update_offline_with_an_uninstalled_extra_viv_php_uses_the_path_php_and_installs_nothing() {
    let ctx = TestContext::new();
    let path = build_with_dom_and_a_path_php_without(&ctx);
    write_ext_dom_project(ctx.project.path(), VIV_PHP);
    let composer_json = fs_err::read_to_string(ctx.project.path().join("composer.json")).unwrap();
    fs_err::write(
        ctx.project.path().join("composer.json"),
        composer_json.replace("\"8.4\"", "\"8.5\""),
    )
    .unwrap();

    let output = ctx
        .viv()
        .env("PATH", path)
        .env("VIV_PHP_DIST_URL", "http://127.0.0.1:9")
        .args(["update", "--offline"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("ext-dom"), "{stderr}");
    assert!(!stderr.contains("installing php"), "{stderr}");
    // The refusal names the binary it probed and how to get the build.
    assert!(
        stderr.contains("Platform packages were read from the php on PATH."),
        "{stderr}"
    );
    assert!(
        stderr.contains(
            "extra.viv.php names php 8.5, which is not installed. Run `viv php install 8.5` or \
             `viv run php -v` to install it."
        ),
        "{stderr}"
    );
}

/// A refused requirement names the php build the check probed (#385, #397),
/// both for `viv update` and for the lock-verify step of `viv install`.
#[test]
fn a_refusal_names_the_php_build_it_probed() {
    let ctx = TestContext::new();
    let path = build_with_dom_and_a_path_php_without(&ctx);
    write_ext_dom_project(ctx.project.path(), VIV_PHP);
    let composer_json = fs_err::read_to_string(ctx.project.path().join("composer.json")).unwrap();
    fs_err::write(
        ctx.project.path().join("composer.json"),
        composer_json.replace("ext-dom", "ext-viv-absent"),
    )
    .unwrap();
    let build_php = ctx
        .cache
        .path()
        .join("php-v0")
        .join(format!(
            "8.4.7-{}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
        .join("php");
    let note = format!(
        "Platform packages were read from {}, the php build set by extra.viv.php.",
        build_php.display()
    );

    let update = ctx.viv().env("PATH", &path).arg("update").output().unwrap();
    let stderr = String::from_utf8_lossy(&update.stderr);
    assert!(!update.status.success(), "{stderr}");
    assert!(stderr.contains("ext-viv-absent"), "{stderr}");
    assert!(stderr.contains(&note), "{stderr}");

    ctx.viv()
        .env("PATH", &path)
        .args(["update", "--ignore-platform-reqs"])
        .assert()
        .success();
    let install = ctx
        .viv()
        .env("PATH", &path)
        .arg("install")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&install.stderr);
    assert!(!install.status.success(), "{stderr}");
    assert!(stderr.contains(&note), "{stderr}");
}

/// `viv diagnose` names the build `extra.viv.php` selects, not the `php` on
/// `PATH` (#385, #397).
#[test]
fn diagnose_reports_the_extra_viv_php_build() {
    let ctx = TestContext::new();
    let path = build_with_dom_and_a_path_php_without(&ctx);
    write_ext_dom_project(ctx.project.path(), VIV_PHP);

    ctx.viv()
        .env("PATH", path)
        .arg("diagnose")
        .assert()
        .success()
        .stdout(predicates::str::contains("php: 8.4.7"));
}

/// #397: with only `config.platform.php`, `viv diagnose` names the `PATH` php.
#[test]
fn diagnose_reports_the_path_php_for_config_platform_php() {
    let ctx = TestContext::new();
    let path = build_with_dom_and_a_path_php_without(&ctx);
    write_ext_dom_project(ctx.project.path(), PLATFORM_PHP);

    ctx.viv()
        .env("PATH", path)
        .arg("diagnose")
        .assert()
        .success()
        .stdout(predicates::str::contains("php: 8.3.1"));
}

/// #368: a `php` that never answers the platform probe is killed after
/// `VIV_PHP_PROBE_TIMEOUT` seconds, and the command fails with a message
/// instead of blocking for as long as the `php` runs.
#[test]
fn update_kills_a_php_that_never_answers_the_probe() {
    use std::os::unix::fs::PermissionsExt as _;

    let ctx = TestContext::new();
    let bin = ctx.cache.path().join("hung-bin");
    fs_err::create_dir_all(&bin).unwrap();
    let php = bin.join("php");
    let pid_file = ctx.cache.path().join("php.pid");
    fs_err::write(
        &php,
        format!(
            "#!/bin/sh\necho $$ > {}\nexec sleep 20\n",
            pid_file.display()
        ),
    )
    .unwrap();
    fs_err::set_permissions(&php, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    fs_err::write(
        ctx.project.path().join("composer.json"),
        "{\n    \"repositories\": [{ \"packagist.org\": false }]\n}\n",
    )
    .unwrap();

    let started = std::time::Instant::now();
    let output = ctx
        .viv()
        .env("PATH", path)
        .env("VIV_PHP_PROBE_TIMEOUT", "1")
        .arg("update")
        .output()
        .expect("failed to run viv");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "a 1 second probe timeout must end a run blocked on php: took {:?}, stderr: {stderr}",
        started.elapsed()
    );
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains(&php.display().to_string())
            && stderr.contains("did not answer")
            && stderr.contains("1s")
            && stderr.contains("VIV_PHP_PROBE_TIMEOUT"),
        "stderr must name the php, the timeout and the env var: {stderr}"
    );
    let pid = fs_err::read_to_string(&pid_file).unwrap();
    let alive = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive, "the hung php (pid {}) must be killed", pid.trim());
}
