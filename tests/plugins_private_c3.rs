//! #98 (`ffraenz/private-composer-installer`) and #126 (`codeception/c3`).
//!
//! #98's adapter lives entirely in `fetch::Fetcher` (dist-URL placeholder
//! substitution right before the download request), so it's exercised
//! directly against `Fetcher` here, the same way
//! `tests/repository.rs`'s `metadata_request_carries_the_configured_authorization_header`
//! drives the real transport against a local server rather than the CLI.
//! Wiring `Fetcher::private_installer` into `install.rs`'s own `Fetcher`
//! construction — the last step that makes this reachable from `viv
//! install` — is out of scope here; see this crate's own return for that
//! blocker. #126's adapter has no such gap (it only ever touches
//! `install.rs` through the existing `apply_pre_autoload_dump` seam), so it
//! is driven through the CLI end-to-end, byte-diffed against real
//! Composer's own output, mirroring `tests/plugins_yii2_craft.rs`.

mod common;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use common::TestContext;
use serde_json::json;
use vivace::auth::Auth;
use vivace::fetch::Fetcher;
use vivace::lock::{Package, Root};
use vivace::plugins::private_installer::Env;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/plugins")
        .join(name)
}

// --- #126: codeception/c3 ---------------------------------------------

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// `codeception/c3` copies its bundled `c3.php` into the project root on
/// install — a local `path` repository package, so this needs no network.
#[test]
fn c3_copies_bundled_file_matching_composer() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    let fixture_dir = fixture("c3");
    std::fs::copy(
        fixture_dir.join("composer.json"),
        project.join("composer.json"),
    )
    .unwrap();
    std::fs::copy(
        fixture_dir.join("composer.lock"),
        project.join("composer.lock"),
    )
    .unwrap();
    copy_tree(&fixture_dir.join("packages"), &project.join("packages"));

    ctx.viv().arg("install").assert().success();

    let want = std::fs::read_to_string(fixture_dir.join("expected/c3.php")).unwrap();
    let got = std::fs::read_to_string(project.join("c3.php")).unwrap();
    assert_eq!(got, want);
}

// --- #98: ffraenz/private-composer-installer ---------------------------

/// A minimal package with a `zip` dist at `url`, for the [`Fetcher`] tests
/// below; every field `fetch` never reads is left at a cheap default (see
/// `fetch.rs`'s own `dist_package` test helper, mirrored here since it's
/// `pub(crate)` there, not reachable from an integration test).
fn dist_package(name: &str, version: &str, url: &str) -> Package {
    Package {
        name: name.to_string(),
        version: version.to_string(),
        dist: Some(vivace::lock::Dist {
            r#type: "zip".to_string(),
            url: url.to_string(),
            reference: None,
            shasum: None,
        }),
        source: None,
        transport_options: vivace::lock::TransportOptions::default(),
        autoload: None,
        require: serde_json::Map::new(),
        provide: serde_json::Map::new(),
        replace: serde_json::Map::new(),
        r#type: "library".to_string(),
        target_dir: None,
        include_path: Vec::new(),
        bin: Vec::new(),
        dev: false,
        raw: serde_json::Value::Null,
        install_dir: None,
        install_from_source: false,
    }
}

fn root(extra: &serde_json::Value) -> Root {
    serde_json::from_value(json!({"extra": extra})).unwrap()
}

/// Accepts exactly one connection, answers with a fixed zip-ish body no
/// matter the request path, and reports the exact path (with query) it
/// received — so a test can assert the *substituted* secret reached the
/// wire without the server needing to know it in advance. Mirrors
/// `tests/repository.rs`'s `spawn_recording_server`: a raw `TcpListener` is
/// enough for one request/response, no mock-server dependency needed.
fn spawn_dist_server(
    body: &'static [u8],
) -> (std::net::SocketAddr, std::sync::mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
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
            .unwrap_or_default()
            .to_string();
        let _ = tx.send(path);

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
    });
    (addr, rx)
}

/// Process env wins: `VIV_TEST_ACME_KEY` resolves the dist URL's
/// `{%VIV_TEST_ACME_KEY}` placeholder without needing a `.env` at all, and
/// `installed.json`/the lock never see the resolved value —
/// `Package::dist.url` is untouched by `fetch`, only the URL actually
/// requested carries it.
#[tokio::test]
#[allow(unsafe_code, reason = "a var name unique to this test, restored below")]
async fn fetch_substitutes_from_process_env() {
    let (addr, requests) = spawn_dist_server(b"PK\x03\x04zip-bytes");
    let url = format!("http://{addr}/dist-{{%VIV_TEST_ACME_KEY}}.zip");
    let pkg = dist_package("acme/pro-plugin", "1.0.0", &url);

    // SAFETY: name unique to this test, restored before it returns.
    unsafe {
        std::env::set_var("VIV_TEST_ACME_KEY", "secret-from-process");
    }
    let env = Env::load(&root(&json!({})), Path::new("/nonexistent"));
    let fetcher = Fetcher::new(Auth::default())
        .unwrap()
        .secure_http(false)
        .private_installer(env);
    let temp = tempfile::tempdir().unwrap();
    let result = fetcher.fetch(&pkg, temp.path()).await;
    // SAFETY: matches the set_var above.
    unsafe {
        std::env::remove_var("VIV_TEST_ACME_KEY");
    }
    result.unwrap();

    let requested_path = requests.recv().unwrap();
    assert!(
        requested_path.contains("secret-from-process"),
        "{requested_path}"
    );
    assert!(
        !requested_path.contains("VIV_TEST_ACME_KEY"),
        "{requested_path}"
    );
    assert_eq!(
        pkg.dist.as_ref().unwrap().url,
        url,
        "lock's own dist.url must keep the literal placeholder"
    );
}

/// `.env` fills the gap when the process environment has nothing: the
/// checked-in `tests/fixtures/plugins/private-installer/dotenv-fixture`
/// (named off `.env` so a global `.gitignore` for `.env` files can't drop
/// it) sets `ACME_KEY`, found via `extra.private-composer-installer.dotenv-name`.
#[tokio::test]
async fn fetch_falls_back_to_a_configured_dotenv_file() {
    let (addr, requests) = spawn_dist_server(b"PK\x03\x04zip-bytes");
    let url = format!("http://{addr}/dist-{{%ACME_KEY}}.zip");
    let pkg = dist_package("acme/pro-plugin", "1.0.0", &url);

    let project_dir = fixture("private-installer");
    let root = root(&json!({
        "private-composer-installer": {"dotenv-name": "dotenv-fixture"}
    }));
    let env = Env::load(&root, &project_dir);
    let fetcher = Fetcher::new(Auth::default())
        .unwrap()
        .secure_http(false)
        .private_installer(env);
    let temp = tempfile::tempdir().unwrap();
    fetcher.fetch(&pkg, temp.path()).await.unwrap();

    let requested_path = requests.recv().unwrap();
    assert!(
        requested_path.contains("from-dotenv-file"),
        "{requested_path}"
    );
}

/// A dist URL placeholder with nothing set anywhere (no process env, no
/// `.env`) is a hard error naming the variable, not a blank substitution.
#[tokio::test]
async fn fetch_errors_naming_a_missing_variable() {
    let pkg = dist_package(
        "acme/pro-plugin",
        "1.0.0",
        "http://127.0.0.1:1/dist-{%VIV_TEST_MISSING_KEY}.zip",
    );
    let env = Env::load(&root(&json!({})), Path::new("/nonexistent"));
    let fetcher = Fetcher::new(Auth::default())
        .unwrap()
        .secure_http(false)
        .private_installer(env);
    let temp = tempfile::tempdir().unwrap();
    let err = fetcher.fetch(&pkg, temp.path()).await.err().unwrap();
    // `{:#}` walks anyhow's whole context chain; the variable name is named
    // by `private_installer::resolve`'s own error, one level below the
    // `with_context` `fetch` wraps it in.
    let err = format!("{err:#}");
    assert!(err.contains("VIV_TEST_MISSING_KEY"), "{err}");
}
