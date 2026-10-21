//! #388: an inline `package` repository whose `dist.url` is a local file,
//! as a path relative to the project root, an absolute path or a `file://`
//! URL, which Composer extracts straight from disk. Offline: the project
//! disables Packagist and every archive is written into the temp project.

mod common;

use std::fs;
use std::io::Write as _;
use std::path::Path;

use common::TestContext;
use serde_json::json;
use zip::write::SimpleFileOptions;

const RELATIVE_URL: &str = "./private/composer-packages/private-plugin-1.0.0.zip";

fn write_zip(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    writer
        .start_file("private-plugin/plugin.php", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(body.as_bytes()).unwrap();
    writer.finish().unwrap();
}

fn write_project(project: &Path, dist: &serde_json::Value) {
    let composer_json = json!({
        "repositories": [
            {"packagist.org": false},
            {"type": "package", "package": {
                "name": "acme/private-plugin",
                "version": "1.0.0",
                "dist": dist,
            }},
        ],
        "require": {"acme/private-plugin": "1.0.0"},
    });
    fs::write(
        project.join("composer.json"),
        serde_json::to_vec_pretty(&composer_json).unwrap(),
    )
    .unwrap();
}

fn update(ctx: &TestContext) {
    ctx.viv()
        .args(["update", "--no-install", "--ignore-platform-reqs"])
        .assert()
        .success();
}

fn update_and_install(ctx: &TestContext) {
    update(ctx);
    install(ctx);
}

fn install(ctx: &TestContext) {
    ctx.viv()
        .args(["install", "--ignore-platform-reqs"])
        .assert()
        .success();
}

fn installed(ctx: &TestContext) -> String {
    fs::read_to_string(
        ctx.project
            .path()
            .join("vendor/acme/private-plugin/plugin.php"),
    )
    .unwrap()
}

#[test]
fn install_extracts_a_zip_at_a_path_relative_to_the_project_root() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_zip(&project.join(RELATIVE_URL), "<?php // v1");
    write_project(project, &json!({"type": "zip", "url": RELATIVE_URL}));

    update_and_install(&ctx);

    assert_eq!(installed(&ctx), "<?php // v1");
}

#[test]
fn install_resolves_the_relative_path_against_the_project_dir_not_the_cwd() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_zip(&project.join(RELATIVE_URL), "<?php // v1");
    write_project(project, &json!({"type": "zip", "url": RELATIVE_URL}));
    update(&ctx);

    let elsewhere = tempfile::tempdir().unwrap();
    ctx.viv()
        .current_dir(elsewhere.path())
        .args(["install", "--ignore-platform-reqs", "-d"])
        .arg(project)
        .assert()
        .success();

    assert_eq!(installed(&ctx), "<?php // v1");
}

#[test]
fn install_extracts_a_zip_at_an_absolute_path_or_a_file_url() {
    for scheme in ["", "file://"] {
        let ctx = TestContext::new();
        let project = ctx.project.path();
        let zip = project.join("elsewhere/private-plugin.zip");
        write_zip(&zip, "<?php // v1");
        let url = format!("{scheme}{}", zip.display());
        write_project(project, &json!({"type": "zip", "url": url}));

        update_and_install(&ctx);

        assert_eq!(installed(&ctx), "<?php // v1", "dist url {url}");
    }
}

#[test]
fn install_does_not_serve_a_stale_archive_after_the_local_zip_changes() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_zip(&project.join(RELATIVE_URL), "<?php // v1");
    write_project(project, &json!({"type": "zip", "url": RELATIVE_URL}));
    update_and_install(&ctx);

    // Same URL, same version, same lock; only the zip's bytes differ.
    write_zip(&project.join(RELATIVE_URL), "<?php // v2");
    fs::remove_dir_all(project.join("vendor")).unwrap();
    install(&ctx);

    assert_eq!(installed(&ctx), "<?php // v2");
}

#[test]
fn install_rejects_a_local_zip_that_does_not_match_the_locked_shasum() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_zip(&project.join(RELATIVE_URL), "<?php // v1");
    write_project(
        project,
        &json!({"type": "zip", "url": RELATIVE_URL, "shasum": "0".repeat(40)}),
    );
    update(&ctx);

    ctx.viv()
        .args(["install", "--ignore-platform-reqs"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("sha1 mismatch"));
}

#[test]
fn install_names_a_missing_local_zip() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_project(project, &json!({"type": "zip", "url": RELATIVE_URL}));
    update(&ctx);

    ctx.viv()
        .args(["install", "--ignore-platform-reqs"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("private-plugin-1.0.0.zip"))
        .stderr(predicates::str::contains("No such file"));
}
