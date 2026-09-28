//! `viv php install`/`viv php list` (#337): a local server stands in for
//! `dl.static-php.dev` (`VIV_PHP_DIST_URL`, test-only), serving a `bulk/`
//! directory listing and a tiny tar.gz whose one entry is a `php` file.

mod common;

use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};

use common::TestContext;
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
        b"{\n    \"name\": \"acme/pkg\"\n}\n",
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
    assert_eq!(composer_json["config"]["platform"]["php"], "8.4");

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
        b"{\n    \"name\": \"acme/pkg\"\n}\n",
    )
    .unwrap();

    ctx.viv().arg("php").arg("install").assert().failure().stderr(
        predicates::str::contains(
            "no PHP pinned: set config.platform.php in composer.json or run viv php install <version>",
        ),
    );
}
