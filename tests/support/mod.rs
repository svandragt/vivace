//! Runs one Composer installer `.test` fixture (upstream's own
//! `--SECTION--` format, `Composer\Test\InstallerTest::readTestFile`)
//! against `vivace::plan::plan` and compares the resulting operations with
//! `--EXPECT--`.
//!
//! Scope: this only exercises the *planner*. Fetching, linking and writing
//! `installed.json` are someone else's job; `--EXPECT-LOCK--` and
//! `--EXPECT-INSTALLED--` aren't asserted here. `--EXPECT-OUTPUT--`'s exact
//! CLI wording (colours, verbosity, the platform-check preamble) is
//! `install`'s job too, not the planner's, so it isn't asserted either —
//! only the key phrase for the rejected-lock fixtures below.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use vivace::lock::{Lock, Root, missing_requirements, read_lock, read_root, validate_against_root};
use vivace::plan::{self, Plan};
use vivace::version::{normalize, normalize_branch};

struct Fixture {
    composer: Value,
    lock_json: String,
    installed: Vec<Value>,
    dev: bool,
    expect: Vec<String>,
    expect_exit_code: Option<i32>,
    expect_output: Option<String>,
}

/// Split a `.test` file on `--SECTION--` marker lines.
fn sections(src: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut name: Option<String> = None;
    let mut body = String::new();
    for line in src.lines() {
        if let Some(header) = section_header(line) {
            if let Some(prev) = name.replace(header) {
                out.insert(prev, std::mem::take(&mut body));
            } else {
                body.clear();
            }
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    if let Some(prev) = name {
        out.insert(prev, body);
    }
    out
}

/// `--FOO-BAR--` on its own line names a section; anything else is content.
fn section_header(line: &str) -> Option<String> {
    let inner = line.strip_prefix("--")?.strip_suffix("--")?;
    (!inner.is_empty() && inner.chars().all(|c| c.is_ascii_uppercase() || c == '-'))
        .then(|| inner.to_string())
}

fn parse(src: &str) -> Fixture {
    let data = sections(src);
    let composer: Value = serde_json::from_str(
        data.get("COMPOSER")
            .expect("installer fixtures always have a COMPOSER section"),
    )
    .expect("COMPOSER section is valid JSON");
    let lock_json = data
        .get("LOCK")
        .expect(
            "this corpus only keeps install runs with a LOCK section \
             (tests/fixtures/composer/README.md)",
        )
        .clone();
    let installed = data
        .get("INSTALLED")
        .map(|s| serde_json::from_str(s).expect("INSTALLED section is a bare package array"))
        .unwrap_or_default();
    let run = data
        .get("RUN")
        .expect("installer fixtures always have a RUN section");
    let dev = !run.split_whitespace().any(|token| token == "--no-dev");
    let expect = data
        .get("EXPECT")
        .map(|s| {
            s.lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let expect_exit_code = data
        .get("EXPECT-EXIT-CODE")
        .map(|s| s.trim().parse().expect("EXPECT-EXIT-CODE is an integer"));
    let expect_output = data.get("EXPECT-OUTPUT").cloned();

    Fixture {
        composer,
        lock_json,
        installed,
        dev,
        expect,
        expect_exit_code,
        expect_output,
    }
}

/// The upstream `--INSTALLED--` section is the pre-Composer-2 bare package
/// array: no `install-path` (a package lived at `vendor/<name>` by
/// convention). Synthesise it in the modern `installed.json` shape and
/// create the directory, so a `remove` is meaningful.
fn write_installed(vendor: &Path, packages: &[Value]) {
    let packages: Vec<Value> = packages
        .iter()
        .cloned()
        .map(|mut package| {
            let name = package
                .get("name")
                .and_then(Value::as_str)
                .expect("INSTALLED entry has a name")
                .to_string();
            if let Some(object) = package.as_object_mut() {
                object
                    .entry("install-path")
                    .or_insert_with(|| Value::String(format!("../{name}")));
            }
            fs_err::create_dir_all(vendor.join(&name)).expect("creating install-path dir");
            package
        })
        .collect();

    fs_err::write(
        vendor.join("composer/installed.json"),
        serde_json::to_vec(&serde_json::json!({
            "packages": packages,
            "dev": true,
            "dev-package-names": [],
        }))
        .expect("serializing installed.json"),
    )
    .expect("writing installed.json");
}

pub(crate) fn run(src: &str) {
    let fixture = parse(src);

    let project = tempfile::tempdir().expect("temp project dir");
    let vendor = project.path().join("vendor");
    fs_err::create_dir_all(vendor.join("composer")).expect("creating vendor/composer");

    let composer_bytes =
        serde_json::to_vec(&fixture.composer).expect("re-encoding the COMPOSER section");
    fs_err::write(project.path().join("composer.json"), &composer_bytes)
        .expect("writing composer.json");
    fs_err::write(project.path().join("composer.lock"), &fixture.lock_json)
        .expect("writing composer.lock");
    write_installed(&vendor, &fixture.installed);

    let root = read_root(&project.path().join("composer.json")).expect("parsing composer.json");
    let lock = read_lock(&project.path().join("composer.lock")).expect("parsing composer.lock");

    match fixture.expect_exit_code {
        Some(code) if code != 0 => assert_rejected(&fixture, &lock, &root, &composer_bytes),
        _ => {
            let result = plan::plan(&lock, fixture.dev, &vendor, project.path()).expect("planning");
            assert_eq!(operations(&fixture, &result), fixture.expect);
        }
    }
}

/// Composer aborts these fixtures before computing any operations (their
/// `EXPECT` is empty). vivace's planner does no dependency solving, so it
/// can't reproduce a solver "Problem N" or policy-block message; the two
/// rejections it *can* reproduce are asserted directly instead:
///
/// - a stale `content-hash` (an "outdated lock" fixture) — checked by
///   phrase only, since content-hash staleness is by itself just a warning
///   in real Composer, and the fixture's actual abort reason may be a
///   deeper solver problem this planner can't see either.
/// - a root requirement whose package name is entirely absent from the
///   lock (a "lock missing packages" fixture) — checked against the exact
///   set of "- Required package ..." lines in EXPECT-OUTPUT, so a fixture
///   that *also* needs constraint checking (version doesn't satisfy, not
///   just "missing") correctly fails here rather than passing by accident.
fn assert_rejected(fixture: &Fixture, lock: &Lock, root: &Root, composer_bytes: &[u8]) {
    assert!(
        fixture.expect.is_empty(),
        "a rejected install has no operations to compare"
    );

    if lock.content_hash.is_some() {
        let err = validate_against_root(lock, composer_bytes)
            .expect_err("a stale content-hash should be rejected");
        assert!(
            err.to_string().contains("is not up to date"),
            "unexpected message: {err}"
        );
        return;
    }

    let expected: std::collections::BTreeSet<&str> = fixture
        .expect_output
        .as_deref()
        .into_iter()
        .flat_map(str::lines)
        .filter(|line| line.starts_with("- Required package"))
        .collect();
    assert!(
        !expected.is_empty()
            && expected
                .iter()
                .all(|l| l.contains("is not present in the lock file")),
        "this fixture's rejection needs constraint checking (\"does not satisfy\"), \
         which vivace's planner doesn't implement (no dependency solver); \
         EXPECT-OUTPUT's \"Required package\" lines: {expected:?}"
    );

    let missing: std::collections::BTreeSet<String> = missing_requirements(lock, root, fixture.dev)
        .into_iter()
        .collect();
    let missing: std::collections::BTreeSet<&str> = missing.iter().map(String::as_str).collect();
    assert_eq!(missing, expected);
}

/// A pre-existing `installed.json` package, enough to derive
/// `Installing`/`Upgrading`/`Downgrading` text and alias marking.
struct Prior {
    version: String,
    reference: Option<String>,
    alias: Option<(String, String)>,
}

/// Derive Composer's `Installing`/`Upgrading`/`Downgrading`/`Removing`/
/// `Marking ... alias of ...` lines from a [`Plan`], in Composer's own
/// order: removals (with their alias markings) first, then installs and
/// upgrades (with theirs) in lock order.
fn operations(fixture: &Fixture, result: &Plan) -> Vec<String> {
    let prior: HashMap<String, Prior> = fixture
        .installed
        .iter()
        .map(|raw| {
            let name = raw
                .get("name")
                .and_then(Value::as_str)
                .expect("INSTALLED entry has a name")
                .to_lowercase();
            let prior = Prior {
                version: raw
                    .get("version")
                    .and_then(Value::as_str)
                    .expect("INSTALLED entry has a version")
                    .to_string(),
                reference: display_reference(raw),
                alias: branch_alias_of(raw),
            };
            (name, prior)
        })
        .collect();

    let mut removes = Vec::new();
    for entry in &result.remove {
        let old = prior.get(&entry.name);
        let reference = old.and_then(|p| p.reference.clone());
        removes.push(format!(
            "Removing {} ({})",
            entry.name,
            pretty(&entry.version, reference.as_deref())
        ));
        if let Some((alias_normalized, alias_pretty)) = old.and_then(|p| p.alias.clone()) {
            removes.push(format!(
                "Marking {} ({}) as uninstalled, alias of {} ({})",
                entry.name,
                pretty(&alias_pretty, reference.as_deref()),
                entry.name,
                pretty(&entry.version, reference.as_deref())
            ));
            let _ = alias_normalized; // only the pretty form is ever shown
        }
    }

    let mut installs = Vec::new();
    for package in &result.install {
        let new_reference = display_reference(&package.raw);
        match prior.get(&package.name) {
            None => {
                installs.push(format!(
                    "Installing {} ({})",
                    package.name,
                    pretty(&package.version, new_reference.as_deref())
                ));
                if let Some((_, alias_pretty)) = branch_alias_of(&package.raw) {
                    installs.push(format!(
                        "Marking {} ({}) as installed, alias of {} ({})",
                        package.name,
                        pretty(&alias_pretty, new_reference.as_deref()),
                        package.name,
                        pretty(&package.version, new_reference.as_deref())
                    ));
                }
            }
            Some(old) => {
                let verb = match version_order(&old.version, &package.version) {
                    std::cmp::Ordering::Less => "Downgrading",
                    _ => "Upgrading", // Greater, or a tie shown as an upgrade too
                };
                installs.push(format!(
                    "{verb} {} ({} => {})",
                    package.name,
                    pretty(&old.version, old.reference.as_deref()),
                    pretty(&package.version, new_reference.as_deref())
                ));
            }
        }
    }

    removes.into_iter().chain(installs).collect()
}

/// Composer's `BasePackage::getFullPrettyVersion`: the VCS reference is only
/// ever appended for a `dev-*`/`*-dev` version — a tagged release's dist
/// reference never shows up in the operation text, only its version.
fn pretty(version: &str, reference: Option<&str>) -> String {
    if is_dev_version(version)
        && let Some(r) = reference.filter(|r| !r.is_empty())
    {
        return format!("{version} {r}");
    }
    version.to_string()
}

fn is_dev_version(version: &str) -> bool {
    let lower = version.to_ascii_lowercase();
    lower.starts_with("dev-") || lower.ends_with("-dev")
}

/// vivace only reads `dist.reference` (v0.1 is zip-dist only, see
/// `Package::validate_dist`); the fixture corpus also has git-sourced
/// packages recorded under `source.reference`, which the operation text
/// still shows, so fall back to it for display purposes only.
fn display_reference(raw: &Value) -> Option<String> {
    raw.get("dist")
        .and_then(|d| d.get("reference"))
        .or_else(|| raw.get("source").and_then(|s| s.get("reference")))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Composer picks `Upgrading` vs `Downgrading` by comparing normalized
/// versions; a plain numeric compare of `version::normalize`'s components
/// covers this corpus (no need for a full semver comparator). Two `dev-*`
/// versions normalize to the same non-numeric string and read as equal,
/// which — like a genuine version tie — is shown as `Upgrading`, matching
/// Composer's wording for a same-version reinstall.
fn version_order(old: &str, new: &str) -> std::cmp::Ordering {
    fn key(version: &str) -> Vec<u64> {
        let normalized = normalize(version).unwrap_or_else(|_| version.to_string());
        normalized
            .split(['.', '-'])
            .map_while(|part| part.parse::<u64>().ok())
            .collect()
    }
    key(new).cmp(&key(old))
}

fn branch_alias_of(raw: &Value) -> Option<(String, String)> {
    let version = raw.get("version").and_then(Value::as_str)?;
    let default_branch = raw
        .get("default-branch")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    branch_alias(version, raw.get("extra"), default_branch)
}

/// Port of Composer's `ArrayLoader::getBranchAlias`: a `dev-*`/`*-dev`
/// package can implicitly alias itself to a numbered branch, either
/// declared via `extra.branch-alias` (a package aliasing its own dev
/// branch) or, lacking that, via `default-branch: true` (the repository's
/// default branch gets `9999999-dev`, Composer's wildcard-match version).
/// Returns `(alias_normalized, pretty_alias)`.
fn branch_alias(
    version: &str,
    extra: Option<&Value>,
    default_branch: bool,
) -> Option<(String, String)> {
    if !is_dev_version(version) {
        return None;
    }

    if let Some(aliases) = extra
        .and_then(|e| e.get("branch-alias"))
        .and_then(Value::as_object)
    {
        for (source_branch, target_branch) in aliases {
            let Some(target_branch) = target_branch.as_str() else {
                continue;
            };
            if !target_branch.to_ascii_lowercase().ends_with("-dev") {
                continue;
            }
            if !source_branch.eq_ignore_ascii_case(version) {
                continue;
            }
            let normalized = if target_branch.eq_ignore_ascii_case("9999999-dev") {
                "9999999-dev".to_string()
            } else {
                normalize_branch(&target_branch[..target_branch.len() - 4])
            };
            if !normalized.ends_with("-dev") {
                continue;
            }
            return Some((
                normalized.clone(),
                collapse_default_branch_alias(&normalized),
            ));
        }
    }

    if default_branch && !looks_numeric_branch(version.strip_prefix('v').unwrap_or(version)) {
        return Some(("9999999-dev".to_string(), "9999999-dev".to_string()));
    }

    None
}

/// `VersionParser::parseNumericAliasPrefix`: is `branch` already a numbered
/// branch (`2.1.x-dev`)? Rules out a `default-branch: true` alias for one.
fn looks_numeric_branch(branch: &str) -> bool {
    static NUMERIC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^(?:\d+\.)*\d+(?:\.x)?-dev$").unwrap());
    NUMERIC.is_match(branch)
}

/// `Preg::replace('{(\.9{7})+}', '.x', $aliasNormalized)`: collapse the
/// `.9999999` runs `normalize_branch` produces into Composer's wildcard
/// display form (`2.1.9999999.9999999-dev` -> `2.1.x-dev`).
fn collapse_default_branch_alias(normalized: &str) -> String {
    static NINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\.9{7})+").unwrap());
    NINES.replace_all(normalized, ".x").into_owned()
}
