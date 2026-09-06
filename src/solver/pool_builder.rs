//! Port of `DependencyResolver/PoolBuilder.php`, `Installer.php`'s request
//! wiring (`createRequest`/`requirePackagesForUpdate`, ~line 900-1070),
//! `Package/Loader/RootPackageLoader.php`'s alias/stability-flag extraction
//! and `Repository/PlatformRepository.php`'s fixed platform packages.
//!
//! Full update only (`docs/resolver-design.md` stage 3): no partial-update
//! allow-list, path-repo unlocking, security-advisory/filter-list pool
//! filters or `PoolOptimizer` (skipped outright per the design doc: pure
//! speed, no semantic effect). `Repository::load_closure` already does the
//! breadth-first, batched metadata load `PoolBuilder::loadPackagesMarkedForLoading`
//! performs in real Composer, so this only needs to turn that closure into
//! `Package`s and filter/alias them.
//!
//! Not modelled: the root `composer.json` package itself as a *pool*
//! member (`Installer::createRequest` also does
//! `$request->fixPackage($rootPackage)`). Its own `require`/`require-dev`
//! links become `request.requires` directly, which is the exact same
//! constraint a root package's own `RULE_PACKAGE_REQUIRES` rules would add
//! on top (the root is always force-installed, so `-root` is never true,
//! collapsing that rule to the same "install one of" disjunction the
//! `RULE_ROOT_REQUIRE` rule already states). Root `conflict`/`replace`/
//! `provide` sections would need the root modelled as a real pool package;
//! add it if a fixture ever needs one.

use std::collections::{HashMap, HashSet};
use std::process::Command;

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{Map, Value};

use crate::repository::{ClosureRoot, DevAcceptance, PackageVersion, Repository, Transport};
use crate::semver;
use crate::solver::pool::{Link, Package, Pool};
use crate::solver::request::Request;

/// `BasePackage::STABILITIES` order, least to most stable... actually most
/// stable first, matching `stability_rank`'s ascending "more stable = lower
/// number" scale (`policy.rs`).
const STABILITIES: [&str; 5] = ["stable", "RC", "beta", "alpha", "dev"];

pub struct BuildResult {
    pub pool: Pool,
    pub request: Request,
}

/// `Installer::doUpdate`'s first solve: root `require` and `require-dev`
/// merged (`Installer.php:1061-1067`), against every package the
/// repository's closure discovers.
pub async fn build<T: Transport>(repo: &Repository<T>, root: &Value) -> Result<BuildResult> {
    let require = string_map(root, "require");
    let require_dev = string_map(root, "require-dev");

    let minimum_stability = root
        .get("minimum-stability")
        .and_then(Value::as_str)
        .map_or("stable", normalize_stability);

    let mut stability_flags: HashMap<String, &'static str> = HashMap::new();
    let mut root_aliases: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for (name, value) in require.iter().chain(require_dev.iter()) {
        let raw = value
            .as_str()
            .with_context(|| format!("require {name}: constraint is not a string"))?;
        extract_alias(name, raw, &mut root_aliases)?;
        extract_stability_flag(name, raw, minimum_stability, &mut stability_flags);
    }

    let acceptable: HashSet<&'static str> = STABILITIES
        .iter()
        .copied()
        .filter(|s| stability_rank(s) <= stability_rank(minimum_stability))
        .collect();

    // `ComposerRepository::loadAsyncPackages`'s `~dev` skip logic
    // (`docs/resolver-design.md`'s Metadata section), applied once for the
    // whole closure rather than per name: `Repository::load_closure` takes
    // one `DevAcceptance` for its whole breadth-first walk (stage 2's
    // API), and over-fetching a `~dev` file for a name that turns out not
    // to need it just costs a request the stability filter below discards
    // the results of, never a wrong answer.
    let dev_acceptance =
        if acceptable.contains("dev") || stability_flags.values().any(|&s| s == "dev") {
            DevAcceptance::Both
        } else {
            DevAcceptance::NonDevOnly
        };

    let roots = [ClosureRoot {
        require: &require,
        require_dev: &require_dev,
    }];
    let closure = repo.load_closure(&roots, dev_acceptance).await?;

    let mut packages = platform_packages();
    let fixed: Vec<usize> = (0..packages.len()).collect();

    for versions in closure.values() {
        for version in versions {
            push_package_version(
                &mut packages,
                version,
                &acceptable,
                &stability_flags,
                &root_aliases,
            )?;
        }
    }

    let pool = Pool::new(packages);

    let mut requires = Vec::with_capacity(require.len() + require_dev.len());
    for (name, value) in require.iter().chain(require_dev.iter()) {
        let raw = value.as_str().expect("checked as_str above");
        requires.push((
            name.to_ascii_lowercase(),
            Some(semver::parse_constraint(raw)?),
        ));
    }

    Ok(BuildResult {
        pool,
        request: Request { requires, fixed },
    })
}

fn string_map(root: &Value, key: &str) -> Map<String, Value> {
    root.get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// `VersionParser::normalizeStability`.
fn normalize_stability(raw: &str) -> &'static str {
    match raw.to_ascii_lowercase().as_str() {
        "rc" => "RC",
        "beta" => "beta",
        "alpha" => "alpha",
        "dev" => "dev",
        _ => "stable",
    }
}

fn stability_rank(stability: &str) -> u8 {
    match stability {
        "stable" => 0,
        "RC" => 5,
        "beta" => 10,
        "alpha" => 15,
        "dev" => 20,
        _ => unreachable!("unknown stability {stability:?}"),
    }
}

/// `RootPackageLoader::extractAliases`: `"X as Y"` in a require's
/// constraint text names a root alias, recorded here as `package -> [(X
/// normalized, Y pretty, Y normalized)]` (`RepositorySet::getRootAliasesPerPackage`'s
/// shape, keyed by target package rather than by package+version so
/// `push_package_version` can look up by name alone).
fn extract_alias(
    name: &str,
    raw: &str,
    root_aliases: &mut HashMap<String, Vec<(String, String, String)>>,
) -> Result<()> {
    static ALIAS: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?:^|\| *|, *)([^,\s#|]+)(?:#\S+)? +as +([^,\s|]+)(?:$| *\|| *,)").unwrap()
    });

    let Some(m) = ALIAS.captures(raw) else {
        return Ok(());
    };
    let version = semver::normalize(&m[1])?.as_str().to_string();
    let alias_normalized = semver::normalize(&m[2])?.as_str().to_string();
    root_aliases
        .entry(name.to_ascii_lowercase())
        .or_default()
        .push((version, m[2].to_string(), alias_normalized));
    Ok(())
}

/// `RootPackageLoader::extractStabilityFlags`, simplified: splits on `||`
/// for OR-groups (skipping the AND-splitting on `,`/space Composer also
/// does; a root require with an explicit `@stability` *and* a separate
/// AND-ed version constraint in the same require is not exercised by any
/// fixture here). An explicit `@stability` suffix wins outright; otherwise
/// an unstable-looking bare version (`dev-main`, `2.x-dev`, `1.0-beta1`)
/// sets the flag only if it is less stable than the minimum.
fn extract_stability_flag(
    name: &str,
    raw: &str,
    minimum_stability: &str,
    stability_flags: &mut HashMap<String, &'static str>,
) {
    static STABILITY_SUFFIX: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)^[^@]*?@(stable|rc|beta|alpha|dev)$").unwrap()
    });
    static ALIAS_SUFFIX: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"^([^,\s@]+) as .+$").unwrap());

    let name = name.to_ascii_lowercase();
    let mut matched_explicit = false;

    for constraint in raw.split("||").map(str::trim) {
        if let Some(m) = STABILITY_SUFFIX.captures(constraint) {
            let stability = normalize_stability(&m[1]);
            set_flag_if_less_stable(stability_flags, &name, stability);
            matched_explicit = true;
        }
    }
    if matched_explicit {
        return;
    }

    for constraint in raw.split("||").map(str::trim) {
        let bare = ALIAS_SUFFIX.replace(constraint, "$1");
        if bare.contains(['@', ',', ' ']) {
            continue;
        }
        let stability = semver::stability(&bare);
        if stability == "stable" {
            continue;
        }
        if stability_rank(stability) <= stability_rank(minimum_stability) {
            continue;
        }
        set_flag_if_less_stable(stability_flags, &name, stability);
    }
}

fn set_flag_if_less_stable(
    flags: &mut HashMap<String, &'static str>,
    name: &str,
    stability: &'static str,
) {
    if let Some(existing) = flags.get(name)
        && stability_rank(existing) > stability_rank(stability)
    {
        return;
    }
    flags.insert(name.to_string(), stability);
}

fn is_acceptable(
    name: &str,
    stability: &str,
    acceptable: &HashSet<&'static str>,
    flags: &HashMap<String, &'static str>,
) -> bool {
    match flags.get(name) {
        Some(&flag) => stability_rank(stability) <= stability_rank(flag),
        None => acceptable.contains(stability),
    }
}

/// `ArrayLoader::getBranchAlias`: `extra.branch-alias` names, for a `dev-*`
/// version, the normalized target branch it stands in for.
fn branch_alias_target(pv: &PackageVersion) -> Option<String> {
    if !(pv.version.starts_with("dev-") || pv.version.ends_with("-dev")) {
        return None;
    }
    let target = pv
        .branch_alias
        .as_ref()?
        .as_object()?
        .get(&pv.version)?
        .as_str()?;
    if !target.ends_with("-dev") {
        return None;
    }
    if target == "9999999-dev" {
        return Some(target.to_string());
    }
    let branch_name = &target[..target.len() - 4];
    let normalized = semver::normalize_branch(branch_name);
    if !normalized.ends_with("-dev") {
        return None;
    }
    Some(normalized)
}

/// `Preg::replace('{(\.9{7})+}', '.x', $aliasNormalized)`.
fn pretty_alias(normalized: &str) -> String {
    static NINES: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(\.9{7})+").unwrap());
    NINES.replace(normalized, ".x").into_owned()
}

/// `Link` holds a parsed `Constraint`, which is not `Clone` (`semver.rs`'s
/// facade wraps a `Box<dyn Constraint>`), so an alias's copy of the
/// aliased package's links (`AliasPackage`'s constructor copies
/// `getRequires`/`getConflicts`/`getProvides`/`getReplaces` verbatim) is
/// rebuilt by reparsing each link's own pretty constraint text rather than
/// cloned; deterministic, since the text already parsed successfully once.
fn clone_links(links: &[Link]) -> Result<Vec<Link>> {
    links
        .iter()
        .map(|link| {
            Ok(Link {
                target: link.target.clone(),
                constraint: link
                    .pretty_constraint
                    .as_deref()
                    .map(semver::parse_constraint)
                    .transpose()?,
                pretty_constraint: link.pretty_constraint.clone(),
            })
        })
        .collect()
}

fn parse_links(
    map: &Map<String, Value>,
    own_name: &str,
    own_pretty_version: &str,
) -> Result<Vec<Link>> {
    let mut links = Vec::with_capacity(map.len());
    for (target, value) in map {
        let target = target.to_ascii_lowercase();
        if target == own_name {
            continue;
        }
        let raw = value
            .as_str()
            .with_context(|| format!("{own_name}: link constraint for {target} is not a string"))?;
        // `self.version`, resolved to the package's own version like
        // `AliasPackage::replaceSelfVersionDependencies` (an exact-version
        // constraint, not a range parse); not exercised by any fixture in
        // this stage's corpus, so this leans on `parse_constraint` treating
        // a bare version string as `== version` rather than constructing
        // that constraint directly.
        let text = if raw == "self.version" {
            own_pretty_version
        } else {
            raw
        };
        links.push(Link {
            target,
            constraint: Some(semver::parse_constraint(text)?),
            pretty_constraint: Some(raw.to_string()),
        });
    }
    Ok(links)
}

/// Filters, aliases (branch and root) and pushes one provider-file entry.
/// A branch-alias version pushes both the alias and the real package it
/// aliases (`ComposerRepository.php:1350-1353`); a root-aliased version
/// additionally pushes a root-alias wrapper. Order matters here: it fixes
/// each pushed package's pool id, and pool id is `Solver`/`DefaultPolicy`'s
/// final tie-break.
fn push_package_version(
    packages: &mut Vec<Package>,
    pv: &PackageVersion,
    acceptable: &HashSet<&'static str>,
    stability_flags: &HashMap<String, &'static str>,
    root_aliases: &HashMap<String, Vec<(String, String, String)>>,
) -> Result<()> {
    let name = pv.name.to_ascii_lowercase();
    let version = semver::normalize(&pv.version)?;
    let stability = semver::stability(version.as_str());

    if !is_acceptable(&name, stability, acceptable, stability_flags) {
        return Ok(());
    }

    let requires = parse_links(&pv.require, &name, &pv.version)?;
    let conflicts = parse_links(&pv.conflict, &name, &pv.version)?;
    let provides = parse_links(&pv.provide, &name, &pv.version)?;
    let replaces = parse_links(&pv.replace, &name, &pv.version)?;
    let is_dev = stability == "dev";

    let real_index = if let Some(alias_normalized) = branch_alias_target(pv) {
        let alias_index = packages.len();
        let real_index = alias_index + 1;
        packages.push(Package {
            name: name.clone(),
            version: semver::normalize(&alias_normalized)?,
            pretty_version: pretty_alias(&alias_normalized),
            stability: semver::stability(&alias_normalized),
            is_dev: true,
            requires: clone_links(&requires)?,
            conflicts: clone_links(&conflicts)?,
            provides: clone_links(&provides)?,
            replaces: clone_links(&replaces)?,
            alias_of: Some(real_index),
            is_root_package_alias: false,
            // Not detected (see `parse_links`'s module doc): would only
            // matter for a `self.version` require inside a dev branch,
            // which no fixture here has.
            has_self_version_requires: false,
        });
        packages.push(Package {
            name: name.clone(),
            version,
            pretty_version: pv.version.clone(),
            stability,
            is_dev,
            requires,
            conflicts,
            provides,
            replaces,
            alias_of: None,
            is_root_package_alias: false,
            has_self_version_requires: false,
        });
        real_index
    } else {
        let index = packages.len();
        packages.push(Package {
            name: name.clone(),
            version,
            pretty_version: pv.version.clone(),
            stability,
            is_dev,
            requires,
            conflicts,
            provides,
            replaces,
            alias_of: None,
            is_root_package_alias: false,
            has_self_version_requires: false,
        });
        index
    };

    // Root alias: matched against the plain (non-branch-alias) version
    // only. Composer also matches a root alias against an *already*
    // branch-aliased package's own alias version
    // (`PoolBuilder::loadPackage` runs once per pool entry, and a
    // branch-aliased provider file entry produces two); no fixture in this
    // stage's corpus combines the two, so only the common case is ported.
    if let Some(aliases) = root_aliases.get(&name) {
        let real = &packages[real_index];
        let real_version = real.version.as_str().to_string();
        for (target_version, alias_pretty, alias_normalized) in aliases {
            if target_version != &real_version {
                continue;
            }
            let real = &packages[real_index];
            let requires = clone_links(&real.requires)?;
            let conflicts = clone_links(&real.conflicts)?;
            let provides = clone_links(&real.provides)?;
            let replaces = clone_links(&real.replaces)?;
            packages.push(Package {
                name: name.clone(),
                version: semver::normalize(alias_normalized)?,
                pretty_version: alias_pretty.clone(),
                stability: semver::stability(alias_normalized),
                is_dev: semver::stability(alias_normalized) == "dev",
                requires,
                conflicts,
                provides,
                replaces,
                alias_of: Some(real_index),
                is_root_package_alias: true,
                has_self_version_requires: false,
            });
        }
    }

    Ok(())
}

/// `Installer::createRequest`'s platform-fixing loop, backed by a
/// deliberately small stand-in for `Repository/PlatformRepository.php`:
/// the running PHP's own version plus its loaded extensions, each given
/// the interpreter's version rather than the real per-library version
/// `PlatformRepository::initialize` painstakingly parses from
/// `phpinfo()`-style extension info.
///
/// ponytail: exact library versions (`lib-openssl`, `lib-icu`, ...) are not
/// reproduced; a `composer.json` pinning one of those (`"lib-openssl":
/// "^1.1"`, rather than the common `"ext-openssl": "*"`) will not resolve.
/// Widen `extension_package` if a fixture needs it.
fn platform_packages() -> Vec<Package> {
    let php_pretty = detect_php_version().unwrap_or_else(|| "8.3.0".to_string());
    let php_version =
        semver::normalize(&php_pretty).unwrap_or_else(|_| semver::normalize("8.3.0").unwrap());

    let mut packages = vec![
        platform_package("php", &php_pretty, php_version.clone()),
        platform_package(
            "composer-plugin-api",
            "2.9.0",
            semver::normalize("2.9.0").unwrap(),
        ),
        platform_package(
            "composer-runtime-api",
            "2.2.2",
            semver::normalize("2.2.2").unwrap(),
        ),
    ];

    for extension in detect_extensions() {
        let lower = extension.to_ascii_lowercase();
        if lower == "core" || lower == "standard" {
            continue;
        }
        let package_name = format!("ext-{}", lower.replace(' ', "-"));
        packages.push(platform_package(
            &package_name,
            &php_pretty,
            php_version.clone(),
        ));
    }

    packages
}

fn platform_package(
    name: &str,
    pretty_version: &str,
    version: semver::NormalizedVersion,
) -> Package {
    Package {
        name: name.to_string(),
        stability: semver::stability(version.as_str()),
        is_dev: false,
        version,
        pretty_version: pretty_version.to_string(),
        requires: Vec::new(),
        conflicts: Vec::new(),
        provides: Vec::new(),
        replaces: Vec::new(),
        alias_of: None,
        is_root_package_alias: false,
        has_self_version_requires: false,
    }
}

fn detect_php_version() -> Option<String> {
    let output = Command::new("php")
        .args(["-r", "echo PHP_VERSION;"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    (!version.is_empty()).then_some(version)
}

fn detect_extensions() -> Vec<String> {
    let Some(output) = Command::new("php")
        .args(["-r", "echo implode(',', get_loaded_extensions());"])
        .output()
        .ok()
        .filter(|o| o.status.success())
    else {
        return Vec::new();
    };
    let Ok(text) = String::from_utf8(output.stdout) else {
        return Vec::new();
    };
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
