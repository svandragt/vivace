//! `viv x`/`viv run`/`viv exec` (#85, `src/tool.rs`): `run`/`exec` are
//! hermetic (a fixture `composer.json` and a hand-written `vendor/bin`
//! proxy, no network); `viv x` needs a real Packagist resolve and dist
//! download for its first run, gated on `VIVACE_TEST_NETWORK=1` like
//! `tests/install_e2e.rs`, and asserts the second run touches neither.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not app logging"
)]

mod common;

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use common::{FixtureTransport, TestContext, fixtures_root};
use predicates::prelude::*;
use vivace::repository::Repository;
use vivace::store::Store;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tool/run-exec")
}

fn setup(project: &Path) {
    std::fs::copy(
        fixture().join("composer.json"),
        project.join("composer.json"),
    )
    .unwrap();
}

#[test]
fn run_list_prints_every_script_in_declaration_order() {
    let ctx = TestContext::new();
    setup(ctx.project.path());

    ctx.viv()
        .args(["run", "--list"])
        .assert()
        .success()
        .stdout("build\nechoargs\n");
}

#[test]
fn run_executes_a_named_script_and_appends_trailing_args() {
    let ctx = TestContext::new();
    setup(ctx.project.path());

    // No `--` before the trailing args (#340): `command` is one positional
    // now, so nothing after the script name needs shielding from viv's own
    // flag parsing any more.
    ctx.viv()
        .args(["run", "echoargs", "extra1", "extra2"])
        .assert()
        .success()
        .stdout("hi extra1 extra2\n");
}

/// Not a declared script, not in `vendor/bin` (the fixture project has
/// none), and not on `PATH` either: `viv run`'s three-way fallback chain
/// (#338) replaces the old plain "Script is not defined" error with one
/// naming all three lookups it tried.
#[test]
fn run_an_unknown_script_fails_naming_all_three_lookups() {
    let ctx = TestContext::new();
    setup(ctx.project.path());

    ctx.viv()
        .args(["run", "nope"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "\"nope\": not a script in composer.json, not in vendor/bin, and not on PATH",
        ));
}

/// `vendor/bin/mytool` is a plain shell script written by the test, not the
/// fixture: `run`/`exec` never care how a bin got there, and a hand-written
/// script keeps the fixture free of a committed executable bit.
fn write_vendor_bin_tool(project: &Path) {
    let bin_dir = project.join("vendor/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = "#!/bin/sh\necho \"first:$(echo $PATH | cut -d: -f1)\"\necho \"args:$@\"\n";
    let path = bin_dir.join("mytool");
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn exec_prepends_vendor_bin_to_path_and_passes_args_through() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project);
    write_vendor_bin_tool(project);

    let expected_bin_dir = std::fs::canonicalize(project).unwrap().join("vendor/bin");
    // No `--` before the trailing args (#340), same as `run` above.
    ctx.viv()
        .args(["exec", "mytool", "foo", "bar"])
        .assert()
        .success()
        .stdout(format!(
            "first:{}\nargs:foo bar\n",
            expected_bin_dir.display()
        ));
}

#[test]
fn exec_an_unknown_bin_fails_naming_the_path() {
    let ctx = TestContext::new();
    setup(ctx.project.path());

    ctx.viv()
        .args(["exec", "nope"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("nope"));
}

/// `vendor/bin/echoargs` prints each of its own argv entries on its own
/// line, so a run/exec that let one of them leak into viv's own flag
/// parsing (#340) shows up as a missing or reordered line rather than a
/// silent hang.
fn write_vendor_bin_echoargs(project: &Path) {
    let bin_dir = project.join("vendor/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\"; done\n";
    let path = bin_dir.join("echoargs");
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// #340: `viv run echoargs -v -d x -h --list` used to hand `-v`/`-d x`/`-h`
/// to viv's own global/`run` flags instead of `echoargs`, since clap
/// matched a global flag before `command`'s trailing var-arg positional had
/// captured any value at all. No `scripts` entry named `echoargs` here
/// (unlike the fixture project), so this exercises the `vendor/bin`
/// fallback, not `scripts::Runner`.
#[test]
fn run_passes_every_flag_looking_argument_through_to_the_script() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    std::fs::write(project.join("composer.json"), "{}").unwrap();
    write_vendor_bin_echoargs(project);

    ctx.viv()
        .args(["run", "echoargs", "-v", "-d", "x", "-h", "--list"])
        .assert()
        .success()
        .stdout("-v\n-d\nx\n-h\n--list\n");
}

/// Same bug, `exec`'s own positional.
#[test]
fn exec_passes_a_flag_looking_argument_through_to_the_bin() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    std::fs::write(project.join("composer.json"), "{}").unwrap();
    write_vendor_bin_echoargs(project);

    ctx.viv()
        .args(["exec", "echoargs", "-v"])
        .assert()
        .success()
        .stdout("-v\n");
}

/// Composer's `run-script test -- --filter X` idiom: one leading `--`
/// after the name is dropped, a second one reaches the tool.
#[test]
fn run_drops_one_leading_double_dash_after_the_name() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    std::fs::write(project.join("composer.json"), "{}").unwrap();
    write_vendor_bin_echoargs(project);

    ctx.viv()
        .args(["run", "echoargs", "--", "-v", "--"])
        .assert()
        .success()
        .stdout("-v\n--\n");
}

/// `<cache>/php-v0/<version>-<os>-<arch>/php` (#338): a fake install
/// mirroring `php::install_dir`'s own naming, no real download — an
/// executable shell script that echoes an unmistakable marker plus its own
/// args, so a script/bin that runs `php`/`@php` can prove it hit *this*
/// binary and not whatever real `php` sits on the system PATH.
fn fake_php_install(cache_dir: &Path, version: &str) {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let dir = cache_dir
        .join("php-v0")
        .join(format!("{version}-{os}-{arch}"));
    std::fs::create_dir_all(&dir).unwrap();
    let php_path = dir.join("php");
    std::fs::write(
        &php_path,
        format!("#!/bin/sh\necho \"FAKE-PHP {version} $@\"\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&php_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(dir.join(".ok"), b"").unwrap();
}

fn write_executable(path: &Path, script: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A `config.platform.php`-pinned project's `composer.json`: `pin` is
/// either a minor (`"8.4"`) or an exact (`"8.5.0"`) version, matching
/// `config.platform.php`'s own two accepted shapes.
fn write_pinned_composer_json(project: &Path, pin: &str) {
    std::fs::write(
        project.join("composer.json"),
        format!(
            "{{\n    \"config\": {{ \"platform\": {{ \"php\": \"{pin}\" }} }},\n    \
             \"scripts\": {{ \"v\": \"php -v\" }}\n}}\n"
        ),
    )
    .unwrap();
}

#[test]
fn run_uses_the_pinned_php_for_a_script() {
    let ctx = TestContext::new();
    fake_php_install(ctx.cache.path(), "8.4.17");
    write_pinned_composer_json(ctx.project.path(), "8.4");

    ctx.viv()
        .args(["run", "v"])
        .assert()
        .success()
        .stdout(predicates::str::contains("FAKE-PHP"));
}

#[test]
fn run_falls_back_to_vendor_bin_and_the_pinned_php_wins_on_path() {
    let ctx = TestContext::new();
    fake_php_install(ctx.cache.path(), "8.4.17");
    write_pinned_composer_json(ctx.project.path(), "8.4");
    write_executable(
        &ctx.project.path().join("vendor/bin/tool"),
        "#!/bin/sh\necho TOOL\nphp -v\n",
    );

    ctx.viv()
        .args(["run", "tool"])
        .assert()
        .success()
        .stdout(predicates::str::contains("TOOL"))
        .stdout(predicates::str::contains("FAKE-PHP"));
}

#[test]
fn run_falls_back_to_path_for_a_binary_not_a_script_or_vendor_bin() {
    let ctx = TestContext::new();
    setup(ctx.project.path());

    ctx.viv().args(["run", "true"]).assert().success();
}

/// Converted from a plain "not installed is an error" test (#340's own
/// predecessor) now that a miss installs the pin instead: `--offline` is the
/// one case that still has to error, since there's nothing left to fall
/// back to without a network request.
#[test]
fn run_and_exec_error_when_offline_and_the_pinned_php_is_not_installed() {
    let ctx = TestContext::new();
    write_pinned_composer_json(ctx.project.path(), "8.5.0");
    write_executable(
        &ctx.project.path().join("vendor/bin/tool"),
        "#!/bin/sh\necho TOOL\n",
    );

    let not_installed = "php 8.5.0 is pinned in composer.json but not installed, and --offline \
                          is set; run viv php install";
    ctx.viv()
        .args(["--offline", "run", "v"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(not_installed));
    ctx.viv()
        .args(["--offline", "exec", "tool"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(not_installed));
}

/// A single-entry (`php`) tar.gz, duplicated from `tests/php.rs`'s own
/// `build_php_tarball` rather than shared: each integration test file is its
/// own crate, and this is small.
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

/// Same shape as `tests/php.rs`'s own `spawn_php_dist_server`, duplicated
/// here for the same reason `build_php_tarball` is: answers every request on
/// a loop (`listing_path` -> `listing_html`, `tarball_path` ->
/// `tarball_bytes`, anything else -> 404) and records each request's path so
/// a warm second run can assert it made none.
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

/// `viv run` on a project pinning a `php` minor with nothing installed for
/// it (#340's own follow-up to #338): a miss now installs it, prints one
/// line to stderr before the download and nothing else, and a warm second
/// run touches neither the network nor stdout/stderr for it.
#[test]
fn run_installs_the_pinned_php_when_missing_and_only_stderr_reports_it() {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let tarball_bytes = build_php_tarball(b"#!/bin/sh\necho fake-php\n");
    let listing_html = format!(
        "<a href=\"php-8.4.7-cli-{os}-{arch}.tar.gz\">php-8.4.7-cli-{os}-{arch}.tar.gz</a>\n"
    );
    let tarball_path = format!("/php-8.4.7-cli-{os}-{arch}.tar.gz");
    let (addr, requests) = spawn_php_dist_server("/", listing_html, tarball_path, tarball_bytes);
    let dist_url = format!("http://{addr}");

    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_pinned_composer_json(project, "8.4");
    write_vendor_bin_echoargs(project);

    ctx.viv()
        .env("VIV_PHP_DIST_URL", &dist_url)
        .args(["run", "echoargs", "-v"])
        .assert()
        .success()
        .stdout("-v\n")
        .stderr(predicates::str::contains("installing php 8.4."));

    let ok_marker = ctx
        .cache
        .path()
        .join("php-v0")
        .join(format!("8.4.7-{os}-{arch}"))
        .join(".ok");
    assert!(ok_marker.is_file(), "{}", ok_marker.display());

    let requests_after_install = requests.lock().unwrap().len();
    ctx.viv()
        .env("VIV_PHP_DIST_URL", &dist_url)
        .args(["run", "echoargs", "-v"])
        .assert()
        .success()
        .stdout("-v\n");
    assert_eq!(
        requests.lock().unwrap().len(),
        requests_after_install,
        "a warm run makes no request to the dist server"
    );
}

/// Same miss, `exec`'s own resolution path.
#[test]
fn exec_installs_the_pinned_php_when_missing() {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let tarball_bytes = build_php_tarball(b"#!/bin/sh\necho fake-php\n");
    let listing_html = format!(
        "<a href=\"php-8.4.7-cli-{os}-{arch}.tar.gz\">php-8.4.7-cli-{os}-{arch}.tar.gz</a>\n"
    );
    let tarball_path = format!("/php-8.4.7-cli-{os}-{arch}.tar.gz");
    let (addr, _requests) = spawn_php_dist_server("/", listing_html, tarball_path, tarball_bytes);
    let dist_url = format!("http://{addr}");

    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_pinned_composer_json(project, "8.4");
    write_vendor_bin_echoargs(project);

    ctx.viv()
        .env("VIV_PHP_DIST_URL", &dist_url)
        .args(["exec", "echoargs", "-v"])
        .assert()
        .success()
        .stdout("-v\n")
        .stderr(predicates::str::contains("installing php 8.4."));
}

#[test]
fn run_uses_the_path_php_when_nothing_is_pinned() {
    if skip_without_php() {
        return;
    }
    let ctx = TestContext::new();
    std::fs::write(
        ctx.project.path().join("composer.json"),
        "{\n    \"scripts\": { \"v\": \"php -v\" }\n}\n",
    )
    .unwrap();

    ctx.viv().args(["run", "v"]).assert().success();
}

/// `viv x`'s tiny recorded-fixture package: no dependencies beyond `php`/
/// `ext-json`, a single `bin` entry, so a first run only ever needs one dist
/// download.
const TOOL_PACKAGE: &str = "php-parallel-lint/php-parallel-lint";

fn skip_without_network() -> bool {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping viv x test: set VIVACE_TEST_NETWORK=1 to resolve and fetch {TOOL_PACKAGE} \
             over the network"
        );
        return true;
    }
    false
}

fn skip_without_php() -> bool {
    if Command::new("php").arg("--version").output().is_err() {
        eprintln!("skipping viv x test: php is not on PATH");
        return true;
    }
    false
}

/// First run resolves and installs over the network (`Installed 1 packages`
/// in stdout) and execs the bin; a second run of the same spec, passed
/// `--offline`, must still succeed and produce the same output — the only
/// way that's possible is the cache hit skipping resolve/install entirely,
/// since `--offline` would otherwise error naming every package not already
/// fetched.
#[test]
fn x_installs_once_then_execs_from_cache_without_any_network() {
    if skip_without_network() || skip_without_php() {
        return;
    }

    let ctx = TestContext::new();
    // `viv x` never reads the project's own composer.json/lock, but it does
    // still need a project dir to run in.
    std::fs::create_dir_all(ctx.project.path()).unwrap();

    // No `--` before `--version` (#340): `command`'s trailing var-arg
    // positional passes a hyphenated argument through untouched once the
    // package name has already filled it, `--` would now be captured
    // literally rather than stripped as a separator.
    ctx.viv()
        .args(["x", TOOL_PACKAGE, "--version"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 1 packages"))
        .stdout(predicates::str::contains("PHP Parallel Lint version"));

    ctx.viv()
        .args(["--offline", "x", TOOL_PACKAGE, "--version"])
        .assert()
        .success()
        .stdout(predicates::str::contains("PHP Parallel Lint version"))
        .stdout(predicates::str::contains("Installed").not());

    ctx.viv()
        .args(["x", "--list"])
        .assert()
        .success()
        .stdout(predicates::str::contains(TOOL_PACKAGE));

    ctx.viv()
        .args(["x", "--uninstall", TOOL_PACKAGE])
        .assert()
        .success();
    ctx.viv()
        .args(["x", "--list"])
        .assert()
        .success()
        .stdout(predicates::str::contains(TOOL_PACKAGE).not());
}

/// #353: `viv x` resolves a tool against the project's pinned PHP, not
/// whatever `php` (if any) is on `PATH`. `<cache>/php-v0/<version>-<os>-
/// <arch>/php`-shaped, like `fake_php_install` above, but answering *both*
/// shapes `ensure_tool_env`'s resolve path asks a `php` binary: `-r 'echo
/// PHP_VERSION;'` (`tool::detect_php_version`'s own cache-key probe) and no
/// args with the solver's platform-probe script piped to stdin
/// (`solver::platform::run_probe`), which needs a `Probe`-shaped JSON back,
/// not just a bare version string.
fn fake_probing_php_install(cache_dir: &Path, version: &str) {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let dir = cache_dir
        .join("php-v0")
        .join(format!("{version}-{os}-{arch}"));
    std::fs::create_dir_all(&dir).unwrap();
    let php_path = dir.join("php");
    std::fs::write(
        &php_path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"-r\" ]; then\n  printf '%s' '{version}'\n  exit 0\nfi\ncat \
             > /dev/null\nprintf '{{\"php_version\":\"{version}\",\"int_size\":8,\"extensions\":{{}}}}'\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&php_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(dir.join(".ok"), b"").unwrap();
}

/// `tests/fixtures/packagist/repo.packagist.org/p2/h/tool.json`'s own
/// fixture package: two versions, one needing `php` `>=8.2`, the other
/// `php` `>=8.0`, so whichever `php` the solver's platform probe reports
/// decides which one `viv x` resolves.
const H_TOOL: &str = "h/tool";

/// Warms `cache_dir`'s on-disk Packagist cache from the local fixture
/// corpus (`tests/update.rs`'s own `offline_partial_update_context`
/// pattern) so a real `viv` subprocess can resolve `h/tool` with
/// `--offline`, no network at all. `Repository::load` alone only fetches
/// the root `packages.json`; a provider file like `p2/h/tool.json` is only
/// ever requested once something actually asks for that name, so this
/// drives one throwaway `solve_update` naming it, purely for that fetch's
/// caching side effect (the resolved version here is never used).
async fn warm_h_tool_metadata(project_dir: &Path, cache_dir: &Path) {
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache_dir, &transport)
        .await
        .unwrap();
    let root = serde_json::json!({"name": "warm/root", "require": {H_TOOL: "*"}});
    vivace::solver::solve_update(&repo, &root, project_dir, false, false)
        .await
        .unwrap();
}

/// A zip containing the declared `bin/tool` entry plus a sibling root file:
/// a *single* top-level directory (`bin/` alone) would trip
/// `store::strip_single_top_dir`'s own "wrapper directory" heuristic and
/// hoist `tool` out of `bin/` — a second root entry, same as any real
/// package's own `composer.json`, keeps it from firing. `bin/tool` itself
/// is a plain `sh` script (no `#!/usr/bin/env php`), so viv's own
/// bin-linking (`tests/bin_goldens.rs`) writes an `sh` proxy into
/// `vendor/bin/tool` rather than a PHP one — this test execs the resolved
/// bin to prove the install finished, and the sandbox this runs in may have
/// no real `php` on `PATH` at all.
fn h_tool_zip() -> Vec<u8> {
    use std::io::Write as _;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("composer.json", options).unwrap();
    writer.write_all(b"{}").unwrap();
    writer
        .start_file("bin/tool", options.unix_permissions(0o755))
        .unwrap();
    writer.write_all(b"#!/bin/sh\necho ok\n").unwrap();
    writer.finish().unwrap().into_inner()
}

/// A minimal `vivace::lock::Package` for `h/tool`'s own dist-by-reference
/// store key (`Store::add_zip`/`lookup` only ever read `name`/`dist`,
/// `tests/update.rs`'s own `offline_partial_update_context` pattern).
fn h_tool_package(version: &str, reference: &str) -> vivace::lock::Package {
    serde_json::from_value(serde_json::json!({
        "name": H_TOOL,
        "version": version,
        "dist": {
            "type": "zip",
            "url": format!("https://example.invalid/dist/h-tool-{version}.zip"),
            "reference": reference,
            "shasum": "",
        },
    }))
    .unwrap()
}

/// Every resolved version of `h/tool` found across every tool env `viv x`
/// has ever cached under `cache_dir` (`tool::list_tools`'s own walk,
/// `tools-v0/<vendor>/<name>/<key>/vendor/composer/installed.json`) — proof
/// of *which* version a run resolved, since `viv x --list` alone only
/// names the package, not its version.
fn cached_tool_versions(cache_dir: &Path, vendor: &str, short_name: &str) -> Vec<String> {
    let base = cache_dir.join("tools-v0").join(vendor).join(short_name);
    let Ok(keys) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    keys.flatten()
        .filter_map(|entry| {
            let installed = entry.path().join("vendor/composer/installed.json");
            let bytes = std::fs::read(&installed).ok()?;
            let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            value
                .get("packages")?
                .as_array()?
                .iter()
                .find(|p| p.get("name").and_then(serde_json::Value::as_str) == Some(H_TOOL))?
                .get("version")?
                .as_str()
                .map(str::to_string)
        })
        .collect()
}

#[tokio::test]
async fn x_resolves_against_the_projects_pinned_php() {
    let ctx = TestContext::new();
    warm_h_tool_metadata(ctx.project.path(), ctx.cache.path()).await;
    let store = Store::open(ctx.cache.path()).unwrap();
    store
        .add_zip(&h_tool_package("1.0.0", "h-tool-100"), &h_tool_zip())
        .unwrap();
    store
        .add_zip(&h_tool_package("2.0.0", "h-tool-200"), &h_tool_zip())
        .unwrap();
    drop(store);

    fake_probing_php_install(ctx.cache.path(), "8.1.0");
    std::fs::write(
        ctx.project.path().join("composer.json"),
        "{\n    \"config\": { \"platform\": { \"php\": \"8.1.0\" } }\n}\n",
    )
    .unwrap();

    ctx.viv()
        .args(["--offline", "x", H_TOOL])
        .assert()
        .success();
    assert_eq!(
        cached_tool_versions(ctx.cache.path(), "h", "tool"),
        vec!["1.0.0".to_string()],
        "php 8.1.0 satisfies only the >=8.0 version"
    );

    // Changing the pin to a PHP the first version never needed re-resolves
    // (the cache key folds in the PHP version, #353) rather than reusing
    // the 8.1.0-keyed env: the newer version's own `php >=8.2` is now
    // satisfied too, and the solver prefers the newest match.
    fake_probing_php_install(ctx.cache.path(), "8.3.0");
    std::fs::write(
        ctx.project.path().join("composer.json"),
        "{\n    \"config\": { \"platform\": { \"php\": \"8.3.0\" } }\n}\n",
    )
    .unwrap();

    ctx.viv()
        .args(["--offline", "x", H_TOOL])
        .assert()
        .success();
    let mut versions = cached_tool_versions(ctx.cache.path(), "h", "tool");
    versions.sort();
    assert_eq!(
        versions,
        vec!["1.0.0".to_string(), "2.0.0".to_string()],
        "the 8.1.0-keyed env from the first run stays cached; the new pin resolves the \
         >=8.2 version into its own new env"
    );
}
