//! `viv completions <shell>` prints a completion script to stdout (#336).

use assert_cmd::Command;

fn completions(shell: &str) -> String {
    let out = Command::cargo_bin("viv")
        .unwrap()
        .args(["completions", shell])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).unwrap()
}

#[test]
fn zsh_script_names_viv_and_its_subcommands() {
    let script = completions("zsh");
    assert!(script.contains("#compdef viv"));
    assert!(script.contains("update"));
}

#[test]
fn bash_script_defines_the_viv_function() {
    assert!(completions("bash").contains("_viv"));
}

#[test]
fn fish_script_registers_viv() {
    assert!(completions("fish").contains("complete -c viv"));
}
