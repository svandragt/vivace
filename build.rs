//! Generates one `#[test]` per `tests/fixtures/composer/installer/*.test`
//! fixture (Riff's pattern), so a new upstream fixture shows up as a
//! (failing, `#[ignore]`d) test without editing Rust by hand. Fixtures not
//! yet listed in `ported.txt` stay `#[ignore]`d until the runner (or the
//! planner) supports them.

use std::collections::HashSet;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let fixtures_dir = Path::new(&manifest_dir).join("tests/fixtures/composer/installer");
    let ported_path = fixtures_dir.join("ported.txt");

    println!("cargo:rerun-if-changed={}", fixtures_dir.display());
    println!("cargo:rerun-if-changed={}", ported_path.display());

    let ported: HashSet<String> = fs::read_to_string(&ported_path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();

    let mut stems: Vec<String> = fs::read_dir(&fixtures_dir)
        .expect("reading tests/fixtures/composer/installer")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "test"))
        .map(|path| {
            path.file_stem()
                .expect("*.test file has a stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    stems.sort();

    let mut generated = String::new();
    for stem in stems {
        println!(
            "cargo:rerun-if-changed={}",
            fixtures_dir.join(format!("{stem}.test")).display()
        );
        let fn_name = format!("composer_{}", stem.replace(['-', '.'], "_"));
        let ignore = if ported.contains(&stem) {
            String::new()
        } else {
            "#[ignore = \"not yet ported, see ported.txt\"]\n".to_string()
        };
        let _ = write!(
            generated,
            "#[test]\n{ignore}fn {fn_name}() {{\n    \
             support::run(include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \
             \"/tests/fixtures/composer/installer/{stem}.test\")));\n}}\n\n"
        );
    }

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    fs::write(
        Path::new(&out_dir).join("installer_fixtures_generated.rs"),
        generated,
    )
    .expect("writing generated installer fixture tests");
}
