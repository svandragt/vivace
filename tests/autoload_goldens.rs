//! Port of Composer's `AutoloadGeneratorTest`: each case builds a root
//! package and its installed packages in a temp dir, runs the generator and
//! byte-compares the output with the goldens in
//! `tests/fixtures/composer/autoload/` (see `autoload-cases.md` there).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vivace::autoload::generator::{Input, Package, RootPackage, generate};

const DEFAULT_VENDOR: &str = "composer-test-autoload";

struct Pkg {
    name: &'static str,
    autoload: Value,
    requires: &'static [&'static str],
    replaces: &'static [&'static str],
    target_dir: Option<&'static str>,
    metapackage: bool,
    include_path: &'static [&'static str],
}

impl Pkg {
    fn new(name: &'static str, autoload: Value) -> Self {
        Self {
            name,
            autoload,
            requires: &[],
            replaces: &[],
            target_dir: None,
            metapackage: false,
            include_path: &[],
        }
    }
    fn requires(mut self, requires: &'static [&'static str]) -> Self {
        self.requires = requires;
        self
    }
    fn replaces(mut self, replaces: &'static [&'static str]) -> Self {
        self.replaces = replaces;
        self
    }
    fn target_dir(mut self, dir: &'static str) -> Self {
        self.target_dir = Some(dir);
        self
    }
    fn metapackage(mut self) -> Self {
        self.metapackage = true;
        self
    }
    fn include_path(mut self, paths: &'static [&'static str]) -> Self {
        self.include_path = paths;
        self
    }
}

/// Paths in `files`, `symlinks`, `working_dir` and `vendor_dir` are relative
/// to the temp dir. `{wd}` in autoload JSON strings expands to the absolute
/// working dir (Composer's tests use absolute paths in two cases).
#[expect(
    clippy::struct_excessive_bools,
    reason = "test fixture mirroring upstream AutoloadGeneratorTest's dump() flags"
)]
struct Case {
    root_name: &'static str,
    root_autoload: Value,
    root_autoload_dev: Value,
    root_target_dir: Option<&'static str>,
    root_requires: &'static [&'static str],
    packages: Vec<Pkg>,
    /// `(path, content)`; a path ending in `/` is a directory.
    files: &'static [(&'static str, &'static str)],
    /// `(link, target)`.
    symlinks: &'static [(&'static str, &'static str)],
    working_dir: &'static str,
    vendor_dir: &'static str,
    dev_mode: bool,
    scan_psr: bool,
    classmap_authoritative: bool,
    /// `None` off; `Some(None)` on with a generated prefix; `Some(Some(p))`
    /// on with a fixed prefix.
    #[allow(
        clippy::option_option,
        reason = "mirrors generator::Input::apcu_prefix"
    )]
    apcu: Option<Option<&'static str>>,
    use_include_path: bool,
    root_include_path: &'static [&'static str],
    suffix: &'static str,
    /// `(golden file name, generated file relative to vendor dir)`.
    goldens: &'static [(&'static str, &'static str)],
    /// `(generated file relative to vendor dir, exact expected content)`.
    inline: &'static [(&'static str, &'static str)],
    /// `(generated file relative to vendor dir, substring that must appear)`.
    contains: &'static [(&'static str, &'static str)],
    /// `(generated file relative to vendor dir, substring that must not appear)`.
    lacks: &'static [(&'static str, &'static str)],
    absent: &'static [&'static str],
    warning_contains: &'static [&'static str],
}

impl Default for Case {
    fn default() -> Self {
        Self {
            root_name: "root/a",
            root_autoload: Value::Null,
            root_autoload_dev: Value::Null,
            root_target_dir: None,
            root_requires: &[],
            packages: Vec::new(),
            files: &[],
            symlinks: &[],
            working_dir: "",
            vendor_dir: DEFAULT_VENDOR,
            dev_mode: false,
            scan_psr: false,
            classmap_authoritative: false,
            apcu: None,
            use_include_path: false,
            root_include_path: &[],
            suffix: "",
            goldens: &[],
            inline: &[],
            contains: &[],
            lacks: &[],
            absent: &[],
            warning_contains: &[],
        }
    }
}

fn goldens_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/composer/autoload")
}

fn read_normalised(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

fn expand(value: &Value, working_dir: &Path) -> Value {
    match value {
        Value::String(s) => Value::String(s.replace("{wd}", &working_dir.to_string_lossy())),
        Value::Array(items) => Value::Array(items.iter().map(|v| expand(v, working_dir)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), expand(v, working_dir)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn run(case: &Case) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tmp = tmp.path().canonicalize().expect("canonical tempdir");
    let working_dir = tmp.join(case.working_dir);
    let vendor_dir = tmp.join(case.vendor_dir);
    fs::create_dir_all(&working_dir).unwrap();
    fs::create_dir_all(vendor_dir.join("composer")).unwrap();

    for (path, content) in case.files {
        let full = tmp.join(path);
        if path.ends_with('/') {
            fs::create_dir_all(&full).unwrap();
        } else {
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(&full, content).unwrap();
        }
    }
    for (link, target) in case.symlinks {
        std::os::unix::fs::symlink(tmp.join(target), tmp.join(link)).unwrap();
    }

    let owned = |items: &[&str]| items.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    let packages = case
        .packages
        .iter()
        .map(|p| Package {
            name: p.name.to_string(),
            autoload: p.autoload.clone(),
            requires: owned(p.requires),
            replaces: owned(p.replaces),
            provides: Vec::new(),
            target_dir: p.target_dir.map(str::to_string),
            install_path: (!p.metapackage).then(|| {
                let mut path = vendor_dir.join(p.name);
                if let Some(target) = p.target_dir {
                    path.push(target);
                }
                path
            }),
            is_dev: false,
            include_path: owned(p.include_path),
            archive_dir: None,
        })
        .collect();

    let input = Input {
        root: RootPackage {
            name: case.root_name.to_string(),
            autoload: expand(&case.root_autoload, &working_dir),
            autoload_dev: expand(&case.root_autoload_dev, &working_dir),
            target_dir: case.root_target_dir.map(str::to_string),
            requires: owned(case.root_requires),
            include_path: owned(case.root_include_path),
        },
        packages,
        dev_mode: case.dev_mode,
        scan_psr: case.scan_psr,
        suffix: case.suffix.to_string(),
        vendor_dir: vendor_dir.clone(),
        base_dir: working_dir.clone(),
        platform_check: false,
        prepend_autoloader: true,
        classmap_authoritative: case.classmap_authoritative,
        apcu_prefix: case.apcu.map(|prefix| prefix.map(str::to_string)),
        use_include_path: case.use_include_path,
    };

    let generated = generate(&input).expect("generate");

    for (golden, actual) in case.goldens {
        let expected = read_normalised(&goldens_dir().join(golden));
        let got = read_normalised(&vendor_dir.join(actual));
        assert_eq!(got, expected, "{actual} differs from golden {golden}");
    }
    for (actual, expected) in case.inline {
        let got = read_normalised(&vendor_dir.join(actual));
        assert_eq!(got, *expected, "{actual} differs from inline expectation");
    }
    for (actual, needle) in case.contains {
        let got = read_normalised(&vendor_dir.join(actual));
        assert!(
            got.contains(needle),
            "{actual} should contain {needle:?}, got:\n{got}"
        );
    }
    for (actual, needle) in case.lacks {
        let got = read_normalised(&vendor_dir.join(actual));
        assert!(
            !got.contains(needle),
            "{actual} should not contain {needle:?}"
        );
    }
    for absent in case.absent {
        assert!(
            !vendor_dir.join(absent).exists(),
            "{absent} should not exist"
        );
    }
    for needle in case.warning_contains {
        assert!(
            generated.warnings.iter().any(|w| w.contains(needle)),
            "expected a warning containing {needle:?}, got {:?}",
            generated.warnings
        );
    }
}

const NS: &str = "composer/autoload_namespaces.php";
const PSR4: &str = "composer/autoload_psr4.php";
const CLASSMAP: &str = "composer/autoload_classmap.php";
const FILES: &str = "composer/autoload_files.php";
const STATIC: &str = "composer/autoload_static.php";
const REAL: &str = "composer/autoload_real.php";
const AUTOLOAD: &str = "autoload.php";
const INCLUDE_PATHS: &str = "composer/include_paths.php";

fn root_autoloading_case() -> Case {
    Case {
        root_autoload: json!({
            "psr-0": {"Main": "src/", "Lala": ["src/", "lib/"]},
            "psr-4": {"Acme\\Fruit\\": "src-fruit/", "Acme\\Cake\\": ["src-cake/", "lib-cake/"]},
            "classmap": ["composersrc/"],
        }),
        files: &[
            (
                "src/Lala/ClassMapMain.php",
                "<?php namespace Lala; class ClassMapMain {}",
            ),
            (
                "src/Lala/Test/ClassMapMainTest.php",
                "<?php namespace Lala\\Test; class ClassMapMainTest {}",
            ),
            (
                "src-cake/ClassMapBar.php",
                "<?php namespace Acme\\Cake; class ClassMapBar {}",
            ),
            ("composersrc/foo.php", "<?php class ClassMapFoo {}"),
            ("src-fruit/", ""),
            ("lib-cake/", ""),
            ("lib/", ""),
        ],
        scan_psr: true,
        suffix: "_1",
        goldens: &[
            ("autoload_main.php", NS),
            ("autoload_psr4.php", PSR4),
            ("autoload_classmap.php", CLASSMAP),
        ],
        ..Case::default()
    }
}

fn root_dev_autoloading_case() -> Case {
    Case {
        root_autoload: json!({"psr-0": {"Main": "src/"}}),
        root_autoload_dev: json!({"files": ["devfiles/foo.php"], "psr-0": {"Main": "tests/"}}),
        files: &[
            (
                "src/Main/ClassMain.php",
                "<?php namespace Main; class ClassMain {}",
            ),
            ("devfiles/foo.php", "<?php function devfoo() {}"),
        ],
        dev_mode: true,
        scan_psr: true,
        suffix: "_1",
        goldens: &[
            ("autoload_main5.php", NS),
            ("autoload_classmap7.php", CLASSMAP),
            ("autoload_files2.php", FILES),
        ],
        ..Case::default()
    }
}

fn root_dev_autoloading_disabled_by_default_case() -> Case {
    Case {
        root_autoload: json!({"psr-0": {"Main": "src/"}}),
        root_autoload_dev: json!({"files": ["devfiles/foo.php"]}),
        files: &[
            (
                "src/Main/ClassMain.php",
                "<?php namespace Main; class ClassMain {}",
            ),
            ("devfiles/foo.php", "<?php function devfoo() {}"),
        ],
        scan_psr: true,
        suffix: "_1",
        goldens: &[
            ("autoload_main4.php", NS),
            ("autoload_classmap7.php", CLASSMAP),
        ],
        absent: &[FILES],
        ..Case::default()
    }
}

fn vendor_dir_same_as_working_dir_case() -> Case {
    Case {
        root_autoload: json!({
            "psr-0": {"Main": "src/", "Lala": "src/"},
            "psr-4": {"Acme\\Fruit\\": "src-fruit/", "Acme\\Cake\\": ["src-cake/", "lib-cake/"]},
            "classmap": ["composersrc/"],
        }),
        vendor_dir: "",
        files: &[
            ("src/Main/Foo.php", "<?php namespace Main; class Foo {}"),
            ("composersrc/foo.php", "<?php class ClassMapFoo {}"),
        ],
        scan_psr: true,
        suffix: "_2",
        goldens: &[
            ("autoload_main3.php", NS),
            ("autoload_psr4_3.php", PSR4),
            ("autoload_classmap3.php", CLASSMAP),
        ],
        ..Case::default()
    }
}

fn alternative_vendor_dir_case() -> Case {
    Case {
        root_autoload: json!({
            "psr-0": {"Main": "src/", "Lala": "src/"},
            "psr-4": {"Acme\\Fruit\\": "src-fruit/", "Acme\\Cake\\": ["src-cake/", "lib-cake/"]},
            "classmap": ["composersrc/"],
        }),
        vendor_dir: "subdir/composer-test-autoload",
        files: &[("composersrc/foo.php", "<?php class ClassMapFoo {}")],
        suffix: "_3",
        goldens: &[
            ("autoload_main2.php", NS),
            ("autoload_psr4_2.php", PSR4),
            ("autoload_classmap2.php", CLASSMAP),
        ],
        ..Case::default()
    }
}

fn root_target_dir_case() -> Case {
    Case {
        root_autoload: json!({
            "psr-0": {"Main\\Foo": "", "Main\\Bar": ""},
            "classmap": ["Main/Foo/src", "lib"],
            "files": ["foo.php", "Main/Foo/bar.php"],
        }),
        root_target_dir: Some("Main/Foo/"),
        files: &[
            ("src/rootfoo.php", "<?php class ClassMapFoo {}"),
            ("lib/rootbar.php", "<?php class ClassMapBar {}"),
            ("foo.php", "<?php class FilesFoo {}"),
            ("bar.php", "<?php class FilesBar {}"),
        ],
        suffix: "TargetDir",
        goldens: &[
            ("autoload_target_dir.php", AUTOLOAD),
            ("autoload_real_target_dir.php", REAL),
            ("autoload_static_target_dir.php", STATIC),
            ("autoload_files_target_dir.php", FILES),
            ("autoload_classmap6.php", CLASSMAP),
        ],
        ..Case::default()
    }
}

fn duplicate_files_warning_case() -> Case {
    Case {
        root_autoload: json!({"files": ["foo.php", "bar.php", "./foo.php", "././foo.php"]}),
        files: &[
            ("foo.php", "<?php class FilesFoo {}"),
            ("bar.php", "<?php class FilesBar {}"),
        ],
        suffix: "FilesWarning",
        goldens: &[("autoload_files_duplicates.php", FILES)],
        warning_contains: &[
            "The following \"files\" autoload rules are included multiple times",
            "$baseDir . '/foo.php'",
        ],
        ..Case::default()
    }
}

fn vendors_autoloading_case() -> Case {
    Case {
        root_requires: &["a/a", "b/b"],
        packages: vec![
            Pkg::new("a/a", json!({"psr-0": {"A": "src/", "A\\B": "lib/"}})),
            Pkg::new("b/b", json!({"psr-0": {"B\\Sub\\Name": "src/"}})),
        ],
        files: &[
            ("composer-test-autoload/a/a/src/", ""),
            ("composer-test-autoload/a/a/lib/", ""),
            ("composer-test-autoload/b/b/src/", ""),
        ],
        suffix: "_5",
        goldens: &[("autoload_vendors.php", NS)],
        contains: &[(CLASSMAP, "return array(\n")],
        ..Case::default()
    }
}

fn vendors_autoloading_with_metapackages_case() -> Case {
    Case {
        root_requires: &["a/a"],
        packages: vec![
            Pkg::new("a/a", json!({"psr-0": {"A": "src/", "A\\B": "lib/"}}))
                .requires(&["b/b"])
                .metapackage(),
            Pkg::new("b/b", json!({"psr-0": {"B\\Sub\\Name": "src/"}})),
        ],
        files: &[
            ("composer-test-autoload/a/a/src/", ""),
            ("composer-test-autoload/a/a/lib/", ""),
            ("composer-test-autoload/b/b/src/", ""),
        ],
        suffix: "_5",
        goldens: &[("autoload_vendors_meta.php", NS)],
        contains: &[(CLASSMAP, "return array(\n")],
        ..Case::default()
    }
}

fn non_dev_exclusion_with_recursion_case() -> Case {
    Case {
        root_requires: &["a/a"],
        packages: vec![
            Pkg::new("a/a", json!({"psr-0": {"A": "src/", "A\\B": "lib/"}})).requires(&["b/b"]),
            Pkg::new("b/b", json!({"psr-0": {"B\\Sub\\Name": "src/"}})).requires(&["a/a"]),
        ],
        files: &[
            ("composer-test-autoload/a/a/src/", ""),
            ("composer-test-autoload/a/a/lib/", ""),
            ("composer-test-autoload/b/b/src/", ""),
        ],
        suffix: "_5",
        goldens: &[("autoload_vendors.php", NS)],
        ..Case::default()
    }
}

fn non_dev_includes_replaced_packages_case() -> Case {
    Case {
        root_requires: &["a/a"],
        packages: vec![
            Pkg::new("a/a", Value::Null).requires(&["b/c"]),
            Pkg::new("b/b", json!({"psr-4": {"B\\": "src/"}})).replaces(&["b/c"]),
        ],
        files: &[(
            "composer-test-autoload/b/b/src/C/C.php",
            "<?php namespace B\\C; class C {}",
        )],
        scan_psr: true,
        suffix: "_5",
        inline: &[(
            CLASSMAP,
            "<?php

// autoload_classmap.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    'B\\\\C\\\\C' => $vendorDir . '/b/b/src/C/C.php',
    'Composer\\\\InstalledVersions' => $vendorDir . '/composer/InstalledVersions.php',
);
",
        )],
        ..Case::default()
    }
}

fn non_dev_exclusion_with_recursion_replace_case() -> Case {
    Case {
        root_requires: &["a/a"],
        packages: vec![
            Pkg::new("a/a", json!({"psr-0": {"A": "src/", "A\\B": "lib/"}})).requires(&["c/c"]),
            Pkg::new("b/b", json!({"psr-0": {"B\\Sub\\Name": "src/"}})).replaces(&["c/c"]),
        ],
        files: &[
            ("composer-test-autoload/a/a/src/", ""),
            ("composer-test-autoload/a/a/lib/", ""),
            ("composer-test-autoload/b/b/src/", ""),
        ],
        suffix: "_5",
        goldens: &[("autoload_vendors.php", NS)],
        ..Case::default()
    }
}

fn non_dev_replaces_nested_requirements_case() -> Case {
    Case {
        root_requires: &["a/a"],
        packages: vec![
            Pkg::new("a/a", json!({"classmap": ["src/A.php"]})).requires(&["b/b"]),
            Pkg::new("b/b", json!({"classmap": ["src/B.php"]})).requires(&["e/e"]),
            Pkg::new("c/c", json!({"classmap": ["src/C.php"]}))
                .replaces(&["b/b"])
                .requires(&["d/d"]),
            Pkg::new("d/d", json!({"classmap": ["src/D.php"]})),
            Pkg::new("e/e", json!({"classmap": ["src/E.php"]})),
        ],
        files: &[
            ("composer-test-autoload/a/a/src/A.php", "<?php class A {}"),
            ("composer-test-autoload/b/b/src/B.php", "<?php class B {}"),
            ("composer-test-autoload/c/c/src/C.php", "<?php class C {}"),
            ("composer-test-autoload/d/d/src/D.php", "<?php class D {}"),
            ("composer-test-autoload/e/e/src/E.php", "<?php class E {}"),
        ],
        suffix: "_5",
        goldens: &[("autoload_classmap9.php", CLASSMAP)],
        ..Case::default()
    }
}

fn phar_autoload_case() -> Case {
    Case {
        root_requires: &["a/a"],
        root_autoload: json!({
            "psr-0": {"Foo": "foo.phar", "Bar": "dir/bar.phar/src"},
            "psr-4": {"Baz\\": "baz.phar", "Qux\\": "dir/qux.phar/src"},
        }),
        packages: vec![Pkg::new(
            "a/a",
            json!({
                "psr-0": {"Lorem": "lorem.phar", "Ipsum": "dir/ipsum.phar/src"},
                "psr-4": {"Dolor\\": "dolor.phar", "Sit\\": "dir/sit.phar/src"},
            }),
        )],
        scan_psr: true,
        suffix: "Phar",
        goldens: &[
            ("autoload_phar.php", NS),
            ("autoload_phar_psr4.php", PSR4),
            ("autoload_phar_static.php", STATIC),
        ],
        ..Case::default()
    }
}

fn psr_to_classmap_ignores_non_existing_dir_case() -> Case {
    Case {
        root_autoload: json!({
            "psr-0": {"Prefix": "foo/bar/non/existing/"},
            "psr-4": {"Prefix\\": "foo/bar/non/existing2/"},
        }),
        scan_psr: true,
        suffix: "_8",
        inline: &[(
            CLASSMAP,
            "<?php

// autoload_classmap.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    'Composer\\\\InstalledVersions' => $vendorDir . '/composer/InstalledVersions.php',
);
",
        )],
        ..Case::default()
    }
}

fn psr_to_classmap_ignores_non_psr_classes_case() -> Case {
    Case {
        root_autoload: json!({"psr-0": {"psr0_": "psr0/"}, "psr-4": {"psr4\\": "psr4/"}}),
        files: &[
            ("psr0/psr0/match.php", "<?php class psr0_match {}"),
            ("psr0/psr0/badfile.php", "<?php class psr0_badclass {}"),
            ("psr4/match.php", "<?php namespace psr4; class match {}"),
            (
                "psr4/badfile.php",
                "<?php namespace psr4; class badclass {}",
            ),
        ],
        scan_psr: true,
        suffix: "_1",
        inline: &[(
            CLASSMAP,
            "<?php

// autoload_classmap.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    'Composer\\\\InstalledVersions' => $vendorDir . '/composer/InstalledVersions.php',
    'psr0_match' => $baseDir . '/psr0/psr0/match.php',
    'psr4\\\\match' => $baseDir . '/psr4/match.php',
);
",
        )],
        ..Case::default()
    }
}

fn vendors_classmap_autoloading_case() -> Case {
    Case {
        root_requires: &["a/a", "b/b"],
        packages: vec![
            Pkg::new("a/a", json!({"classmap": ["src/"]})),
            Pkg::new("b/b", json!({"classmap": ["src/", "lib/"]})),
        ],
        files: &[
            (
                "composer-test-autoload/a/a/src/a.php",
                "<?php class ClassMapFoo {}",
            ),
            (
                "composer-test-autoload/b/b/src/b.php",
                "<?php class ClassMapBar {}",
            ),
            (
                "composer-test-autoload/b/b/lib/c.php",
                "<?php class ClassMapBaz {}",
            ),
        ],
        suffix: "_6",
        goldens: &[("autoload_classmap4.php", CLASSMAP)],
        ..Case::default()
    }
}

fn vendors_classmap_autoloading_with_target_dir_case() -> Case {
    Case {
        root_requires: &["a/a", "b/b"],
        packages: vec![
            Pkg::new("a/a", json!({"classmap": ["target/src/", "lib/"]})).target_dir("target"),
            Pkg::new("b/b", json!({"classmap": ["src/"]})),
        ],
        files: &[
            (
                "composer-test-autoload/a/a/target/src/a.php",
                "<?php class ClassMapFoo {}",
            ),
            (
                "composer-test-autoload/a/a/target/lib/b.php",
                "<?php class ClassMapBar {}",
            ),
            (
                "composer-test-autoload/b/b/src/c.php",
                "<?php class ClassMapBaz {}",
            ),
        ],
        suffix: "_6",
        inline: &[(
            CLASSMAP,
            "<?php

// autoload_classmap.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    'ClassMapBar' => $vendorDir . '/a/a/target/lib/b.php',
    'ClassMapBaz' => $vendorDir . '/b/b/src/c.php',
    'ClassMapFoo' => $vendorDir . '/a/a/target/src/a.php',
    'Composer\\\\InstalledVersions' => $vendorDir . '/composer/InstalledVersions.php',
);
",
        )],
        ..Case::default()
    }
}

fn classmap_empty_dir_and_exact_file_case() -> Case {
    Case {
        root_requires: &["a/a", "b/b", "c/c"],
        packages: vec![
            Pkg::new("a/a", json!({"classmap": [""]})),
            Pkg::new("b/b", json!({"classmap": ["test.php"]})),
            Pkg::new("c/c", json!({"classmap": ["./"]})),
        ],
        files: &[
            (
                "composer-test-autoload/a/a/src/a.php",
                "<?php class ClassMapFoo {}",
            ),
            (
                "composer-test-autoload/b/b/test.php",
                "<?php class ClassMapBar {}",
            ),
            (
                "composer-test-autoload/c/c/foo/test.php",
                "<?php class ClassMapBaz {}",
            ),
        ],
        suffix: "_7",
        goldens: &[("autoload_classmap5.php", CLASSMAP)],
        lacks: &[
            (REAL, "$loader->setClassMapAuthoritative(true);"),
            (REAL, "$loader->setApcuPrefix("),
        ],
        ..Case::default()
    }
}

fn files_autoload_generation_case() -> Case {
    Case {
        root_autoload: json!({"files": ["root.php"]}),
        root_requires: &["a/a", "b/b", "c/c"],
        packages: vec![
            Pkg::new("a/a", json!({"files": ["test.php"]})),
            Pkg::new("b/b", json!({"files": ["test2.php"]})),
            Pkg::new("c/c", json!({"files": ["test3.php", "foo/bar/test4.php"]}))
                .target_dir("foo/bar"),
        ],
        files: &[
            (
                "composer-test-autoload/a/a/test.php",
                "<?php function testFilesAutoloadGeneration1() {}",
            ),
            (
                "composer-test-autoload/b/b/test2.php",
                "<?php function testFilesAutoloadGeneration2() {}",
            ),
            (
                "composer-test-autoload/c/c/foo/bar/test3.php",
                "<?php function testFilesAutoloadGeneration3() {}",
            ),
            (
                "composer-test-autoload/c/c/foo/bar/test4.php",
                "<?php function testFilesAutoloadGeneration4() {}",
            ),
            (
                "root.php",
                "<?php function testFilesAutoloadGenerationRoot() {}",
            ),
        ],
        suffix: "FilesAutoload",
        goldens: &[
            ("autoload_functions.php", AUTOLOAD),
            ("autoload_real_functions.php", REAL),
            ("autoload_static_functions.php", STATIC),
            ("autoload_files_functions.php", FILES),
        ],
        ..Case::default()
    }
}

fn files_autoload_order_by_dependencies_case() -> Case {
    Case {
        root_autoload: json!({"files": ["root2.php"]}),
        root_requires: &["z/foo", "b/bar", "d/d", "e/e"],
        packages: vec![
            Pkg::new("z/foo", json!({"files": ["testA.php"]})).requires(&["c/lorem"]),
            Pkg::new("b/bar", json!({"files": ["testB.php"]})).requires(&["c/lorem", "d/d"]),
            Pkg::new("d/d", json!({"files": ["testD.php"]})).requires(&["c/lorem"]),
            Pkg::new("c/lorem", json!({"files": ["testC.php"]})),
            Pkg::new("e/e", json!({"files": ["testE.php"]})).requires(&["c/lorem"]),
        ],
        files: &[
            (
                "composer-test-autoload/z/foo/testA.php",
                "<?php function testFilesAutoloadOrderByDependency1() {}",
            ),
            (
                "composer-test-autoload/b/bar/testB.php",
                "<?php function testFilesAutoloadOrderByDependency2() {}",
            ),
            (
                "composer-test-autoload/c/lorem/testC.php",
                "<?php function testFilesAutoloadOrderByDependency3() {}",
            ),
            (
                "composer-test-autoload/d/d/testD.php",
                "<?php function testFilesAutoloadOrderByDependency4() {}",
            ),
            (
                "composer-test-autoload/e/e/testE.php",
                "<?php function testFilesAutoloadOrderByDependency5() {}",
            ),
            (
                "root2.php",
                "<?php function testFilesAutoloadOrderByDependencyRoot() {}",
            ),
        ],
        suffix: "FilesAutoloadOrder",
        goldens: &[
            ("autoload_functions_by_dependency.php", AUTOLOAD),
            ("autoload_real_files_by_dependency.php", REAL),
            ("autoload_static_files_by_dependency.php", STATIC),
        ],
        ..Case::default()
    }
}

fn override_vendors_autoloading_case() -> Case {
    Case {
        root_name: "root/z",
        root_autoload: json!({"psr-0": {"A\\B": "{wd}/lib"}, "classmap": ["{wd}/src"]}),
        root_requires: &["a/a", "b/b"],
        packages: vec![
            Pkg::new(
                "a/a",
                json!({"psr-0": {"A": "src/", "A\\B": "lib/"}, "classmap": ["classmap"]}),
            ),
            Pkg::new("b/b", json!({"psr-0": {"B\\Sub\\Name": "src/"}})),
        ],
        files: &[
            ("lib/A/B/C.php", "<?php namespace A\\B; class C {}"),
            ("src/classes.php", "<?php namespace Foo; class Bar {}"),
            (
                "composer-test-autoload/a/a/lib/A/B/C.php",
                "<?php namespace A\\B; class C {}",
            ),
            (
                "composer-test-autoload/a/a/classmap/classes.php",
                "<?php namespace Foo; class Bar {}",
            ),
            ("composer-test-autoload/a/a/src/", ""),
            ("composer-test-autoload/b/b/src/", ""),
        ],
        scan_psr: true,
        suffix: "_9",
        inline: &[
            (
                NS,
                "<?php

// autoload_namespaces.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    'B\\\\Sub\\\\Name' => array($vendorDir . '/b/b/src'),
    'A\\\\B' => array($baseDir . '/lib', $vendorDir . '/a/a/lib'),
    'A' => array($vendorDir . '/a/a/src'),
);
",
            ),
            (
                PSR4,
                "<?php

// autoload_psr4.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
);
",
            ),
            (
                CLASSMAP,
                "<?php

// autoload_classmap.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    'A\\\\B\\\\C' => $baseDir . '/lib/A/B/C.php',
    'Composer\\\\InstalledVersions' => $vendorDir . '/composer/InstalledVersions.php',
    'Foo\\\\Bar' => $baseDir . '/src/classes.php',
);
",
            ),
        ],
        ..Case::default()
    }
}

fn vendor_dir_excluded_from_working_dir_case() -> Case {
    Case {
        root_autoload: json!({
            "psr-0": {"Foo": "src"},
            "psr-4": {"Acme\\Foo\\": "src-psr4"},
            "classmap": ["classmap"],
            "files": ["test.php"],
        }),
        root_requires: &["b/b"],
        packages: vec![Pkg::new(
            "b/b",
            json!({
                "psr-0": {"Bar": "lib"},
                "psr-4": {"Acme\\Bar\\": "lib-psr4"},
                "classmap": ["classmaps"],
                "files": ["bootstrap.php"],
            }),
        )],
        working_dir: "composer-test-autoload/working-dir",
        vendor_dir: "composer-test-autoload/vendor",
        files: &[
            (
                "composer-test-autoload/working-dir/src/Foo/Bar.php",
                "<?php namespace Foo; class Bar {}",
            ),
            (
                "composer-test-autoload/working-dir/classmap/classes.php",
                "<?php namespace Foo; class Foo {}",
            ),
            (
                "composer-test-autoload/working-dir/test.php",
                "<?php class Foo {}",
            ),
            (
                "composer-test-autoload/vendor/b/b/lib/Bar/Foo.php",
                "<?php namespace Bar; class Foo {}",
            ),
            (
                "composer-test-autoload/vendor/b/b/classmaps/classes.php",
                "<?php namespace Bar; class Bar {}",
            ),
            (
                "composer-test-autoload/vendor/b/b/bootstrap.php",
                "<?php class Bar {}",
            ),
        ],
        scan_psr: true,
        suffix: "_13",
        inline: &[
            (
                NS,
                "<?php

// autoload_namespaces.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir).'/working-dir';

return array(
    'Foo' => array($baseDir . '/src'),
    'Bar' => array($vendorDir . '/b/b/lib'),
);
",
            ),
            (
                PSR4,
                "<?php

// autoload_psr4.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir).'/working-dir';

return array(
    'Acme\\\\Foo\\\\' => array($baseDir . '/src-psr4'),
    'Acme\\\\Bar\\\\' => array($vendorDir . '/b/b/lib-psr4'),
);
",
            ),
            (
                CLASSMAP,
                "<?php

// autoload_classmap.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir).'/working-dir';

return array(
    'Bar\\\\Bar' => $vendorDir . '/b/b/classmaps/classes.php',
    'Bar\\\\Foo' => $vendorDir . '/b/b/lib/Bar/Foo.php',
    'Composer\\\\InstalledVersions' => $vendorDir . '/composer/InstalledVersions.php',
    'Foo\\\\Bar' => $baseDir . '/src/Foo/Bar.php',
    'Foo\\\\Foo' => $baseDir . '/classmap/classes.php',
);
",
            ),
        ],
        contains: &[
            (FILES, "$vendorDir . '/b/b/bootstrap.php',\n"),
            (FILES, "$baseDir . '/test.php',\n"),
        ],
        ..Case::default()
    }
}

fn up_level_relative_paths_case() -> Case {
    Case {
        root_autoload: json!({
            "psr-0": {"Foo": "../path/../src"},
            "psr-4": {"Acme\\Foo\\": "../path/../src-psr4"},
            "classmap": ["../classmap", "../classmap2/subdir", "classmap3", "classmap4"],
            "files": ["../test.php"],
            "exclude-from-classmap": [
                "./../classmap/excluded",
                "../classmap2",
                "classmap3/classes.php",
                "classmap4/*/classes.php",
            ],
        }),
        working_dir: "working-dir",
        files: &[
            ("src/Foo/Bar.php", "<?php namespace Foo; class Bar {}"),
            ("classmap/classes.php", "<?php namespace Foo; class Foo {}"),
            (
                "classmap/excluded/classes.php",
                "<?php namespace Foo; class Boo {}",
            ),
            (
                "classmap2/subdir/classes.php",
                "<?php namespace Foo; class Boo2 {}",
            ),
            (
                "working-dir/classmap3/classes.php",
                "<?php namespace Foo; class Boo3 {}",
            ),
            (
                "working-dir/classmap4/foo/classes.php",
                "<?php namespace Foo; class Boo4 {}",
            ),
            ("test.php", "<?php class Foo {}"),
        ],
        scan_psr: true,
        suffix: "_14",
        inline: &[
            (
                NS,
                "<?php

// autoload_namespaces.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir).'/working-dir';

return array(
    'Foo' => array($baseDir . '/../src'),
);
",
            ),
            (
                PSR4,
                "<?php

// autoload_psr4.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir).'/working-dir';

return array(
    'Acme\\\\Foo\\\\' => array($baseDir . '/../src-psr4'),
);
",
            ),
            (
                CLASSMAP,
                "<?php

// autoload_classmap.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir).'/working-dir';

return array(
    'Composer\\\\InstalledVersions' => $vendorDir . '/composer/InstalledVersions.php',
    'Foo\\\\Bar' => $baseDir . '/../src/Foo/Bar.php',
    'Foo\\\\Foo' => $baseDir . '/../classmap/classes.php',
);
",
            ),
        ],
        contains: &[(FILES, "$baseDir . '/../test.php',\n")],
        ..Case::default()
    }
}

fn empty_paths_case() -> Case {
    Case {
        root_autoload: json!({"psr-0": {"Foo": ""}, "psr-4": {"Acme\\Foo\\": ""}, "classmap": [""]}),
        files: &[
            ("Foo/Bar.php", "<?php namespace Foo; class Bar {}"),
            ("class.php", "<?php namespace Classmap; class Foo {}"),
        ],
        scan_psr: true,
        suffix: "_15",
        inline: &[
            (
                NS,
                "<?php

// autoload_namespaces.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    'Foo' => array($baseDir . '/'),
);
",
            ),
            (
                PSR4,
                "<?php

// autoload_psr4.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    'Acme\\\\Foo\\\\' => array($baseDir . '/'),
);
",
            ),
            (
                CLASSMAP,
                "<?php

// autoload_classmap.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    'Classmap\\\\Foo' => $baseDir . '/class.php',
    'Composer\\\\InstalledVersions' => $vendorDir . '/composer/InstalledVersions.php',
    'Foo\\\\Bar' => $baseDir . '/Foo/Bar.php',
);
",
            ),
        ],
        ..Case::default()
    }
}

fn vendor_substring_path_case() -> Case {
    Case {
        root_autoload: json!({
            "psr-0": {"Foo": "composer-test-autoload-src/src"},
            "psr-4": {"Acme\\Foo\\": "composer-test-autoload-src/src-psr4"},
        }),
        files: &[("composer-test-autoload/a/", "")],
        suffix: "VendorSubstring",
        inline: &[
            (
                NS,
                "<?php

// autoload_namespaces.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    'Foo' => array($baseDir . '/composer-test-autoload-src/src'),
);
",
            ),
            (
                PSR4,
                "<?php

// autoload_psr4.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    'Acme\\\\Foo\\\\' => array($baseDir . '/composer-test-autoload-src/src-psr4'),
);
",
            ),
        ],
        ..Case::default()
    }
}

fn exclude_from_classmap_case() -> Case {
    Case {
        root_autoload: json!({
            "psr-0": {"Main": "src/", "Lala": ["src/", "lib/"]},
            "psr-4": {"Acme\\Fruit\\": "src-fruit/", "Acme\\Cake\\": ["src-cake/", "lib-cake/"]},
            "classmap": ["composersrc/"],
            "exclude-from-classmap": [
                "/composersrc/foo/bar/",
                "/composersrc/excludedTests/",
                "/composersrc/ClassToExclude.php",
                "/composersrc/*/excluded/excsubpath",
                "**/excsubpath",
                "composers",
                "/src-ca/",
            ],
        }),
        files: &[
            ("composer/", ""),
            (
                "src/Lala/ClassMapMain.php",
                "<?php namespace Lala; class ClassMapMain {}",
            ),
            (
                "src/Lala/Test/ClassMapMainTest.php",
                "<?php namespace Lala\\Test; class ClassMapMainTest {}",
            ),
            ("lib/", ""),
            ("src-fruit/", ""),
            ("lib-cake/", ""),
            (
                "src-cake/ClassMapBar.php",
                "<?php namespace Acme\\Cake; class ClassMapBar {}",
            ),
            ("composersrc/tests/", ""),
            ("composersrc/foo.php", "<?php class ClassMapFoo {}"),
            (
                "composersrc/excludedTests/bar.php",
                "<?php class ClassExcludeMapFoo {}",
            ),
            (
                "composersrc/ClassToExclude.php",
                "<?php class ClassClassToExclude {}",
            ),
            (
                "composersrc/long/excluded/excsubpath/foo.php",
                "<?php class ClassExcludeMapFoo2 {}",
            ),
            (
                "composersrc/long/excluded/excsubpath/bar.php",
                "<?php class ClassExcludeMapBar {}",
            ),
            (
                "forks/bar/src/exclude/FooExclClass.php",
                "<?php class FooExclClass {};",
            ),
            ("composersrc/foo/", ""),
        ],
        symlinks: &[("composersrc/foo/bar", "forks/bar/")],
        scan_psr: true,
        suffix: "_1",
        goldens: &[("autoload_classmap.php", CLASSMAP)],
        ..Case::default()
    }
}

#[allow(
    clippy::option_option,
    reason = "mirrors generator::Input::apcu_prefix"
)]
fn classmap_authoritative_and_apcu_setup(apcu: Option<Option<&'static str>>) -> Case {
    Case {
        root_requires: &["a/a", "b/b", "c/c"],
        packages: vec![
            Pkg::new("a/a", json!({"psr-4": {"": "src/"}})),
            Pkg::new("b/b", json!({"psr-4": {"": "./"}})),
            Pkg::new("c/c", json!({"psr-4": {"": "foo/"}})),
        ],
        files: &[
            (
                "composer-test-autoload/a/a/src/ClassMapFoo.php",
                "<?php class ClassMapFoo {}",
            ),
            (
                "composer-test-autoload/b/b/ClassMapBar.php",
                "<?php class ClassMapBar {}",
            ),
            (
                "composer-test-autoload/c/c/foo/ClassMapBaz.php",
                "<?php class ClassMapBaz {}",
            ),
        ],
        classmap_authoritative: true,
        apcu,
        suffix: "_7",
        goldens: &[("autoload_classmap8.php", CLASSMAP)],
        contains: &[(REAL, "$loader->setClassMapAuthoritative(true);")],
        ..Case::default()
    }
}

fn classmap_authoritative_and_apcu_case() -> Case {
    Case {
        contains: &[
            (REAL, "$loader->setClassMapAuthoritative(true);"),
            (REAL, "$loader->setApcuPrefix("),
        ],
        ..classmap_authoritative_and_apcu_setup(Some(None))
    }
}

fn classmap_authoritative_and_apcu_prefix_case() -> Case {
    Case {
        contains: &[
            (REAL, "$loader->setClassMapAuthoritative(true);"),
            (REAL, "$loader->setApcuPrefix('custom\\'Prefix');"),
        ],
        ..classmap_authoritative_and_apcu_setup(Some(Some("custom'Prefix")))
    }
}

fn include_path_file_generation_case() -> Case {
    Case {
        packages: vec![
            Pkg::new("a/a", Value::Null).include_path(&["lib/"]),
            Pkg::new("b/b", Value::Null).include_path(&["library"]),
            Pkg::new("c", Value::Null).include_path(&["library"]),
        ],
        suffix: "_10",
        goldens: &[("include_paths.php", INCLUDE_PATHS)],
        ..Case::default()
    }
}

fn include_paths_are_prepended_case() -> Case {
    Case {
        packages: vec![Pkg::new("a/a", Value::Null).include_path(&["lib/"])],
        suffix: "_11",
        contains: &[(
            REAL,
            "        $includePaths = require __DIR__ . '/include_paths.php';\n        $includePaths[] = get_include_path();\n        set_include_path(implode(PATH_SEPARATOR, $includePaths));\n",
        )],
        ..Case::default()
    }
}

fn include_paths_in_root_package_case() -> Case {
    Case {
        root_include_path: &["/lib", "/src"],
        packages: vec![Pkg::new("a/a", Value::Null).include_path(&["lib/"])],
        suffix: "_12",
        inline: &[(
            INCLUDE_PATHS,
            "<?php

// include_paths.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
    $baseDir . '/lib',
    $baseDir . '/src',
    $vendorDir . '/a/a/lib',
);
",
        )],
        contains: &[(REAL, "require __DIR__ . '/include_paths.php';")],
        ..Case::default()
    }
}

fn include_path_file_without_paths_is_skipped_case() -> Case {
    Case {
        packages: vec![Pkg::new("a/a", Value::Null)],
        suffix: "_12",
        absent: &[INCLUDE_PATHS],
        lacks: &[(REAL, "include_paths.php")],
        ..Case::default()
    }
}

fn use_global_include_path_case() -> Case {
    Case {
        root_autoload: json!({"psr-0": {"Main\\Foo": "", "Main\\Bar": ""}}),
        root_target_dir: Some("Main/Foo/"),
        use_include_path: true,
        suffix: "IncludePath",
        goldens: &[
            ("autoload_real_include_path.php", REAL),
            ("autoload_static_include_path.php", STATIC),
        ],
        ..Case::default()
    }
}

macro_rules! cases {
    ($($name:ident => $build:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                run(&$build());
            }
        )*
    };
}

cases! {
    root_package_autoloading => root_autoloading_case,
    root_package_dev_autoloading => root_dev_autoloading_case,
    root_package_dev_autoloading_disabled_by_default => root_dev_autoloading_disabled_by_default_case,
    vendor_dir_same_as_working_dir => vendor_dir_same_as_working_dir_case,
    root_package_autoloading_alternative_vendor_dir => alternative_vendor_dir_case,
    root_package_autoloading_with_target_dir => root_target_dir_case,
    duplicate_files_warning => duplicate_files_warning_case,
    vendors_autoloading => vendors_autoloading_case,
    vendors_autoloading_with_metapackages => vendors_autoloading_with_metapackages_case,
    non_dev_autoload_exclusion_with_recursion => non_dev_exclusion_with_recursion_case,
    non_dev_autoload_should_include_replaced_packages => non_dev_includes_replaced_packages_case,
    non_dev_autoload_exclusion_with_recursion_replace => non_dev_exclusion_with_recursion_replace_case,
    non_dev_autoload_replaces_nested_requirements => non_dev_replaces_nested_requirements_case,
    phar_autoload => phar_autoload_case,
    psr_to_classmap_ignores_non_existing_dir => psr_to_classmap_ignores_non_existing_dir_case,
    psr_to_classmap_ignores_non_psr_classes => psr_to_classmap_ignores_non_psr_classes_case,
    vendors_classmap_autoloading => vendors_classmap_autoloading_case,
    vendors_classmap_autoloading_with_target_dir => vendors_classmap_autoloading_with_target_dir_case,
    classmap_autoloading_empty_dir_and_exact_file => classmap_empty_dir_and_exact_file_case,
    files_autoload_generation => files_autoload_generation_case,
    files_autoload_order_by_dependencies => files_autoload_order_by_dependencies_case,
    override_vendors_autoloading => override_vendors_autoloading_case,
    vendor_dir_excluded_from_working_dir => vendor_dir_excluded_from_working_dir_case,
    up_level_relative_paths => up_level_relative_paths_case,
    empty_paths => empty_paths_case,
    vendor_substring_path => vendor_substring_path_case,
    exclude_from_classmap => exclude_from_classmap_case,
    classmap_autoloading_authoritative_and_apcu => classmap_authoritative_and_apcu_case,
    classmap_autoloading_authoritative_and_apcu_prefix => classmap_authoritative_and_apcu_prefix_case,
    include_path_file_generation => include_path_file_generation_case,
    include_paths_are_prepended_in_autoload_file => include_paths_are_prepended_case,
    include_paths_in_root_package => include_paths_in_root_package_case,
    include_path_file_without_paths_is_skipped => include_path_file_without_paths_is_skipped_case,
    use_global_include_path => use_global_include_path_case,
}

/// `testFilesAutoloadGenerationRemoveExtraEntitiesFromAutoloadFiles`: three
/// `generate()` calls against the same vendor dir, each with less autoload
/// data than the last, checking that files/include-paths a previous dump
/// wrote are cleaned up rather than left stale.
#[test]
fn files_autoload_generation_remove_extra_entities() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tmp = tmp.path().canonicalize().expect("canonical tempdir");
    let vendor_dir = tmp.join(DEFAULT_VENDOR);
    fs::create_dir_all(vendor_dir.join("composer")).unwrap();

    for (path, content) in [
        (
            "composer-test-autoload/a/a/test.php",
            "<?php function testFilesAutoloadGeneration1() {}",
        ),
        (
            "composer-test-autoload/b/b/test2.php",
            "<?php function testFilesAutoloadGeneration2() {}",
        ),
        (
            "composer-test-autoload/c/c/foo/bar/test3.php",
            "<?php function testFilesAutoloadGeneration3() {}",
        ),
        (
            "composer-test-autoload/c/c/foo/bar/test4.php",
            "<?php function testFilesAutoloadGeneration4() {}",
        ),
        (
            "root.php",
            "<?php function testFilesAutoloadGenerationRoot() {}",
        ),
    ] {
        let full = tmp.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, content).unwrap();
    }

    let install_path = |name: &str, target: Option<&str>| {
        let mut path = vendor_dir.join(name);
        if let Some(target) = target {
            path.push(target);
        }
        Some(path)
    };
    let requires = || vec!["a/a".to_string(), "b/b".to_string(), "c/c".to_string()];

    let root_with_autoload = RootPackage {
        name: "root/a".to_string(),
        autoload: json!({"files": ["root.php"]}),
        autoload_dev: Value::Null,
        target_dir: None,
        requires: requires(),
        include_path: vec!["/lib".to_string(), "/src".to_string()],
    };
    let root_without_autoload = RootPackage {
        autoload: Value::Null,
        include_path: Vec::new(),
        ..root_with_autoload.clone()
    };

    let packages_with_autoload = vec![
        Package {
            name: "a/a".to_string(),
            autoload: json!({"files": ["test.php"]}),
            requires: Vec::new(),
            replaces: Vec::new(),
            provides: Vec::new(),
            target_dir: None,
            install_path: install_path("a/a", None),
            is_dev: false,
            include_path: vec!["lib1".to_string(), "src1".to_string()],
            archive_dir: None,
        },
        Package {
            name: "b/b".to_string(),
            autoload: json!({"files": ["test2.php"]}),
            requires: Vec::new(),
            replaces: Vec::new(),
            provides: Vec::new(),
            target_dir: None,
            install_path: install_path("b/b", None),
            is_dev: false,
            include_path: vec!["lib2".to_string()],
            archive_dir: None,
        },
        Package {
            name: "c/c".to_string(),
            autoload: json!({"files": ["test3.php", "foo/bar/test4.php"]}),
            requires: Vec::new(),
            replaces: Vec::new(),
            provides: Vec::new(),
            target_dir: Some("foo/bar".to_string()),
            install_path: install_path("c/c", Some("foo/bar")),
            is_dev: false,
            include_path: vec!["lib3".to_string()],
            archive_dir: None,
        },
    ];
    let packages_without_autoload: Vec<Package> = ["a/a", "b/b", "c/c"]
        .iter()
        .map(|name| Package {
            name: (*name).to_string(),
            autoload: Value::Null,
            requires: Vec::new(),
            replaces: Vec::new(),
            provides: Vec::new(),
            target_dir: None,
            install_path: install_path(name, None),
            is_dev: false,
            include_path: Vec::new(),
            archive_dir: None,
        })
        .collect();

    let base_input = Input {
        root: root_with_autoload,
        packages: packages_with_autoload,
        dev_mode: false,
        scan_psr: false,
        suffix: "FilesAutoload".to_string(),
        vendor_dir: vendor_dir.clone(),
        base_dir: tmp.clone(),
        platform_check: false,
        prepend_autoloader: true,
        classmap_authoritative: false,
        apcu_prefix: None,
        use_include_path: false,
    };

    let assert_file = |golden: &str, actual: &str| {
        let expected = read_normalised(&goldens_dir().join(golden));
        let got = read_normalised(&vendor_dir.join(actual));
        assert_eq!(got, expected, "{actual} differs from golden {golden}");
    };

    generate(&base_input).expect("dump 1");
    assert_file("autoload_functions.php", AUTOLOAD);
    assert_file("autoload_real_functions_with_include_paths.php", REAL);
    assert_file("autoload_static_functions_with_include_paths.php", STATIC);
    assert_file("autoload_files_functions.php", FILES);
    assert_file("include_paths_functions.php", INCLUDE_PATHS);

    let input2 = Input {
        packages: packages_without_autoload.clone(),
        ..base_input.clone()
    };
    generate(&input2).expect("dump 2");
    assert_file("autoload_functions.php", AUTOLOAD);
    assert_file("autoload_real_functions_with_include_paths.php", REAL);
    assert_file("autoload_files_functions_with_removed_extra.php", FILES);
    assert_file(
        "include_paths_functions_with_removed_extra.php",
        INCLUDE_PATHS,
    );

    let input3 = Input {
        root: root_without_autoload,
        packages: packages_without_autoload,
        ..base_input
    };
    generate(&input3).expect("dump 3");
    assert_file(
        "autoload_real_functions_with_removed_include_paths_and_autolad_files.php",
        REAL,
    );
    assert_file(
        "autoload_static_functions_with_removed_include_paths_and_autolad_files.php",
        STATIC,
    );
    assert!(
        !vendor_dir.join(FILES).exists(),
        "autoload_files.php should have been removed"
    );
    assert!(
        !vendor_dir.join(INCLUDE_PATHS).exists(),
        "include_paths.php should have been removed"
    );
}

/// Issue #71: a classmap entry can declare a class name with a byte that is
/// not valid UTF-8 (`class \xA9 {}`); Composer's `var_export` writes it raw
/// inside single quotes rather than escaping or replacing it. `read_normalised`
/// can't be used here (it needs valid UTF-8), so this reads the generated
/// files as bytes and checks the exact line real Composer's `dump-autoload`
/// produces for the same layout (verified by hand against Composer 2.10).
#[test]
fn classmap_keeps_non_utf8_class_name_byte() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tmp = tmp.path().canonicalize().expect("canonical tempdir");
    let vendor_dir = tmp.join(DEFAULT_VENDOR);
    fs::create_dir_all(vendor_dir.join("composer")).unwrap();

    let src_dir = tmp.join("composersrc");
    fs::create_dir_all(&src_dir).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/composer/classmap/invalidBytes/InvalidBytes.php");
    fs::copy(&fixture, src_dir.join("InvalidBytes.php")).expect("copy fixture");

    let input = Input {
        root: RootPackage {
            name: "root/a".to_string(),
            autoload: json!({"classmap": ["composersrc/"]}),
            autoload_dev: Value::Null,
            target_dir: None,
            requires: Vec::new(),
            include_path: Vec::new(),
        },
        packages: Vec::new(),
        dev_mode: false,
        scan_psr: false,
        suffix: "InvalidBytes".to_string(),
        vendor_dir: vendor_dir.clone(),
        base_dir: tmp,
        platform_check: false,
        prepend_autoloader: true,
        classmap_authoritative: false,
        apcu_prefix: None,
        use_include_path: false,
    };
    generate(&input).expect("generate");

    let classmap = fs::read(vendor_dir.join(CLASSMAP)).expect("read autoload_classmap.php");
    let expected_classmap_line = b"    '\xA9' => $baseDir . '/composersrc/InvalidBytes.php',\n";
    assert!(
        classmap
            .windows(expected_classmap_line.len())
            .any(|w| w == expected_classmap_line.as_slice()),
        "expected {expected_classmap_line:?} in {classmap:?}"
    );

    let static_file = fs::read(vendor_dir.join(STATIC)).expect("read autoload_static.php");
    let expected_static_line =
        b"        '\xA9' => __DIR__ . '/../..' . '/composersrc/InvalidBytes.php',\n";
    assert!(
        static_file
            .windows(expected_static_line.len())
            .any(|w| w == expected_static_line.as_slice()),
        "expected {expected_static_line:?} in {static_file:?}"
    );
}
