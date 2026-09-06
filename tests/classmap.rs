//! Port of `class-map-generator`'s `ClassMapGeneratorTest`: table-driven
//! directory scans plus the ambiguity, missing-directory, backtrack-limit
//! and exclusion cases. Fixtures are the upstream corpus copied verbatim
//! into `tests/fixtures/composer/classmap/`.

use std::path::{Path, PathBuf};

use regex::Regex;
use vivace::autoload::classmap::scan_paths;

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/composer/classmap")
}

/// Scan `dir` (relative to the classmap fixture root) and assert its class
/// map equals `expected`, as `(class, path relative to the fixture root)`.
fn assert_map(dir: &str, expected: &[(&str, &str)]) {
    let root = fixtures_root().join(dir);
    let result = scan_paths(&root, None).expect("scan should succeed");

    let mut actual: Vec<(String, String)> = result
        .map
        .iter()
        .map(|(class, path)| {
            let relative = path
                .strip_prefix(fixtures_root())
                .expect("scanned path should be under the fixture root")
                .to_string_lossy()
                .replace('\\', "/");
            (class.clone(), relative)
        })
        .collect();
    actual.sort();

    let mut expected: Vec<(String, String)> = expected
        .iter()
        .map(|(class, path)| ((*class).to_string(), (*path).to_string()))
        .collect();
    expected.sort();

    assert_eq!(actual, expected, "scanning {dir}");
}

#[test]
fn namespaced() {
    assert_map(
        "Namespaced",
        &[
            ("Namespaced\\Bar", "Namespaced/Bar.inc"),
            ("Namespaced\\Foo", "Namespaced/Foo.php"),
            ("Namespaced\\Baz", "Namespaced/Baz.php"),
        ],
    );
}

#[test]
fn namespace_collision() {
    assert_map(
        "beta/NamespaceCollision",
        &[
            (
                "NamespaceCollision\\A\\B\\Bar",
                "beta/NamespaceCollision/A/B/Bar.php",
            ),
            (
                "NamespaceCollision\\A\\B\\Foo",
                "beta/NamespaceCollision/A/B/Foo.php",
            ),
        ],
    );
}

#[test]
fn pearlike() {
    assert_map(
        "Pearlike",
        &[
            ("Pearlike_Foo", "Pearlike/Foo.php"),
            ("Pearlike_Bar", "Pearlike/Bar.php"),
            ("Pearlike_Baz", "Pearlike/Baz.php"),
        ],
    );
}

#[test]
fn classmap_directory() {
    assert_map(
        "classmap",
        &[
            ("Foo\\Bar\\A", "classmap/sameNsMultipleClasses.php"),
            ("Foo\\Bar\\B", "classmap/sameNsMultipleClasses.php"),
            ("Alpha\\A", "classmap/multipleNs.php"),
            ("Alpha\\B", "classmap/multipleNs.php"),
            ("A", "classmap/multipleNs.php"),
            ("Be\\ta\\A", "classmap/multipleNs.php"),
            ("Be\\ta\\B", "classmap/multipleNs.php"),
            ("ClassMap\\SomeInterface", "classmap/SomeInterface.php"),
            ("ClassMap\\SomeParent", "classmap/SomeParent.php"),
            ("ClassMap\\SomeClass", "classmap/SomeClass.php"),
            ("ClassMap\\LongString", "classmap/LongString.php"),
            ("Foo\\LargeClass", "classmap/LargeClass.php"),
            ("Foo\\LargeGap", "classmap/LargeGap.php"),
            ("Foo\\MissingSpace", "classmap/MissingSpace.php"),
            ("Foo\\StripNoise", "classmap/StripNoise.php"),
            ("Foo\\First", "classmap/StripNoise.php"),
            ("Foo\\Second", "classmap/StripNoise.php"),
            ("Foo\\Third", "classmap/StripNoise.php"),
            ("Foo\\SlashedA", "classmap/BackslashLineEndingString.php"),
            ("Foo\\SlashedB", "classmap/BackslashLineEndingString.php"),
            ("Unicode\\\u{2191}\\\u{2191}", "classmap/Unicode.php"),
            ("ShortOpenTag", "classmap/ShortOpenTag.php"),
            (
                "Smarty_Internal_Compile_Block",
                "classmap/InvalidUnicode.php",
            ),
            (
                "Smarty_Internal_Compile_Blockclose",
                "classmap/InvalidUnicode.php",
            ),
            ("ShortOpenTagDocblock", "classmap/ShortOpenTagDocblock.php"),
        ],
    );
}

#[test]
fn template_directory_has_no_classes() {
    assert_map("template", &[]);
}

#[test]
fn php54_traits() {
    assert_map(
        "php5.4",
        &[
            ("TFoo", "php5.4/traits.php"),
            ("CFoo", "php5.4/traits.php"),
            ("Foo\\TBar", "php5.4/traits.php"),
            ("Foo\\IBar", "php5.4/traits.php"),
            ("Foo\\TFooBar", "php5.4/traits.php"),
            ("Foo\\CBar", "php5.4/traits.php"),
        ],
    );
}

#[test]
fn php70_anonymous_classes_are_skipped() {
    assert_map(
        "php7.0",
        &[("Dummy\\Test\\AnonClassHolder", "php7.0/anonclass.php")],
    );
}

#[test]
fn php81_enums() {
    assert_map(
        "php8.1",
        &[
            ("RolesBasicEnum", "php8.1/enum_basic.php"),
            ("RolesBackedEnum", "php8.1/enum_backed.php"),
            ("RolesClassLikeEnum", "php8.1/enum_class_semantics.php"),
            (
                "Foo\\Bar\\RolesClassLikeNamespacedEnum",
                "php8.1/enum_namespaced.php",
            ),
        ],
    );
}

#[test]
fn ambiguous_reference_is_recorded_and_first_wins() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("other")).unwrap();
    std::fs::write(dir.path().join("A.php"), "<?php\nclass A {}").unwrap();
    std::fs::write(dir.path().join("other/A.php"), "<?php\nclass A {}").unwrap();

    let result = scan_paths(dir.path(), None).expect("scan should succeed");

    assert_eq!(result.map.len(), 1);
    assert!(result.map.contains_key("A"));
    assert_eq!(result.ambiguous.len(), 1);
    assert_eq!(result.ambiguous[0].0, "A");
}

#[test]
fn create_map_throws_when_directory_does_not_exist() {
    let missing = fixtures_root().join("no-file.no-folder");
    let error = scan_paths(&missing, None).expect_err("missing path should error");
    assert!(
        error
            .to_string()
            .contains("Could not scan for classes inside"),
        "unexpected error: {error}"
    );
}

#[test]
fn does_not_hit_regex_backtrace_limit() {
    assert_map(
        "pcrebacktracelimit",
        &[
            ("Foo\\StripNoise", "pcrebacktracelimit/StripNoise.php"),
            (
                "Foo\\VeryLongHeredoc",
                "pcrebacktracelimit/VeryLongHeredoc.php",
            ),
            (
                "Foo\\ClassAfterLongHereDoc",
                "pcrebacktracelimit/VeryLongHeredoc.php",
            ),
            (
                "Foo\\VeryLongPHP73Heredoc",
                "pcrebacktracelimit/VeryLongPHP73Heredoc.php",
            ),
            (
                "Foo\\VeryLongPHP73Nowdoc",
                "pcrebacktracelimit/VeryLongPHP73Nowdoc.php",
            ),
            (
                "Foo\\ClassAfterLongNowDoc",
                "pcrebacktracelimit/VeryLongPHP73Nowdoc.php",
            ),
            (
                "Foo\\VeryLongNowdoc",
                "pcrebacktracelimit/VeryLongNowdoc.php",
            ),
        ],
    );
}

#[test]
fn create_map_with_directory_excluded() {
    let root = fixtures_root().join("beta");
    let exclude = Regex::new("/NamespaceCollision(/|$)").unwrap();
    let result = scan_paths(&root, Some(&exclude)).expect("scan should succeed");

    let mut actual: Vec<(String, String)> = result
        .map
        .iter()
        .map(|(class, path)| {
            let relative = path
                .strip_prefix(fixtures_root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            (class.clone(), relative)
        })
        .collect();
    actual.sort();

    let mut expected = vec![
        (
            "PrefixCollision_A_B_Bar".to_string(),
            "beta/PrefixCollision/A/B/Bar.php".to_string(),
        ),
        (
            "PrefixCollision_A_B_Foo".to_string(),
            "beta/PrefixCollision/A/B/Foo.php".to_string(),
        ),
    ];
    expected.sort();

    assert_eq!(actual, expected);
}

#[cfg(unix)]
#[test]
fn exclude_matches_the_uncanonicalised_symlink_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::fs::create_dir(dir.path().join("proj")).unwrap();
    std::fs::write(dir.path().join("real/Foo.php"), "<?php\nclass Foo {}").unwrap();
    std::os::unix::fs::symlink("../real", dir.path().join("proj/linked")).unwrap();

    let proj = dir.path().join("proj");
    let prefix = regex::escape(&proj.to_string_lossy().replace('\\', "/"));
    let exclude = Regex::new(&format!("^{prefix}/linked/")).unwrap();
    let result = scan_paths(&proj, Some(&exclude)).expect("scan should succeed");

    assert!(result.map.is_empty(), "got {:?}", result.map);
}

#[cfg(unix)]
#[test]
fn symlink_cycle_terminates() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("loop/a/b")).unwrap();
    std::fs::write(dir.path().join("loop/a/A.php"), "<?php\nclass A {}").unwrap();
    std::os::unix::fs::symlink("../..", dir.path().join("loop/a/b/self")).unwrap();

    let result = scan_paths(&dir.path().join("loop"), None).expect("scan should succeed");

    assert!(result.map.contains_key("A"));
}

#[cfg(unix)]
#[test]
fn broken_symlink_is_skipped() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("Ok.php"), "<?php\nclass Ok {}").unwrap();
    std::os::unix::fs::symlink("/nonexistent", dir.path().join("Ghost.php")).unwrap();

    let result = scan_paths(dir.path(), None).expect("scan should succeed");

    assert!(result.map.contains_key("Ok"));
    assert_eq!(result.map.len(), 1);
}

#[test]
fn hidden_files_and_vcs_directories_are_skipped() {
    assert_map("hiddenDirectory", &[("A", "hiddenDirectory/visible/A.php")]);
}

#[test]
fn scanning_a_hidden_directory_directly_still_works() {
    assert_map(
        "hiddenDirectory/.hidden",
        &[("B", "hiddenDirectory/.hidden/B.php")],
    );
}
