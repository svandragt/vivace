//! `viv man <dir>` writes roff man pages, one per command (#335).

use assert_cmd::Command;

#[test]
fn writes_a_page_for_viv_and_one_per_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    // A directory that does not exist yet: `viv man` creates it.
    let out = dir.path().join("man");
    Command::cargo_bin("viv")
        .unwrap()
        .arg("man")
        .arg(&out)
        .assert()
        .success();
    let viv = std::fs::read_to_string(out.join("viv.1")).unwrap();
    assert!(viv.contains(".TH viv"));
    assert!(out.join("viv-install.1").is_file());
}

#[test]
fn man_is_hidden_from_help() {
    let help = Command::cargo_bin("viv")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(help).unwrap();
    assert!(!help.lines().any(|l| l.trim_start().starts_with("man ")));
}
