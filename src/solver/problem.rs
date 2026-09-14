//! A port of `Problem.php`/`SolverProblemsException.php` good enough to
//! name the packages and constraints an unsatisfiable request involved in
//! Composer's own wording, for the four shapes `docs/resolver-design.md`
//! stage 5 (composer/composer#42) calls out: an unsatisfiable root
//! requirement, a conflict between two requirements, a package that cannot
//! be found at all, and a platform requirement. Not ported:
//! security-advisory/abandoned/filter-list wording, and
//! `computeCheckForLowerPrioRepo`'s repository-priority diagnostics —
//! vivace has one repository, so that class of message never applies.
//!
//! `Problem` collects `Reason`s exactly as before; the pretty-printing that
//! used to live in a `Display` impl now needs `&Pool` (to look up a
//! package's pretty name/version and to ask whether a name exists at all),
//! so it moved to [`Problem::pretty_string`], called once at
//! `solver::solve`'s only `Err(SolverError { .. })` site while `pool` is
//! still in scope (`SolverError` itself now carries pre-rendered strings,
//! not `Problem`s, since a `Problem` has no lifetime of its own to borrow
//! `Pool` across the `?`/`Result` boundary).
//!
//! `Problem.php`'s reason sort (`getRulePriority`/`getSortableString`) and
//! `formatDeduplicatedRules`'s version-range collapsing (composer/composer#42,
//! this crate's #62) are ported below: `rule_priority`/`sortable_string`
//! feed the sort in [`Problem::pretty_string`], and `format_deduplicated_rules`
//! replaces the old flat `lines.dedup()`. Ported against `Reason` directly
//! rather than PHP's regex-over-rendered-message trick (`Preg::isMatchStrictGroups`
//! recovering the source package/version PHP had already thrown away by
//! rendering to a plain string): this port still has the `Reason`, so it
//! already knows which lines are `PackageRequires`/`PackageConflict` and
//! what their source package is, no parse-back needed.

use std::collections::HashSet;
use std::fmt;

use crate::semver::{self, NormalizedVersion};
use crate::solver::pool::{self, Pool};
use crate::solver::request::Request;
use crate::solver::rules::Reason;

/// One `Problem`: every [`Reason`] the solver could trace back from an
/// unsatisfiable rule (`Rule::RULE_PACKAGE` rules never reach here, matching
/// `analyze_unsolvable_rule`'s "package rules cannot be part of a problem").
#[derive(Default)]
pub struct Problem {
    reasons: Vec<Reason>,
}

impl Problem {
    pub fn new() -> Problem {
        Problem::default()
    }

    pub fn add_reason(&mut self, reason: Reason) {
        self.reasons.push(reason);
    }

    /// `Problem::getPrettyString`. Composer builds this bottom-up from
    /// `array_merge(...array_reverse($this->reasons))` (sections in
    /// reverse, each section's own order preserved); `add_reason` never
    /// sections here (no `nextSection` caller in this port), so replaying
    /// `self.reasons` in insertion order is the same list.
    pub fn pretty_string(&self, pool: &Pool, request: &Request) -> String {
        // `getPrettyString`'s single-reason fast path: a lone
        // `RULE_ROOT_REQUIRE` with *zero* pool matches for the name (any
        // version, ignoring the constraint that made it unsatisfiable)
        // short-circuits straight to `getMissingPackageReason`. A name that
        // does exist falls through to the general per-reason list below,
        // same as every other reason count.
        if let [
            Reason::RootRequire {
                package_name,
                pretty_constraint,
            },
        ] = self.reasons.as_slice()
            && pool.what_provides(package_name, None).is_empty()
        {
            return format!(
                "\n    {}",
                missing_package_reason(pool, request, package_name, pretty_constraint)
            );
        }

        // `getPrettyString`'s `usort`: highest `getRulePriority` first, tied
        // reasons ordered by `getSortableString`.
        let mut reasons: Vec<&Reason> = self.reasons.iter().collect();
        reasons.sort_by(|a, b| {
            rule_priority(b)
                .cmp(&rule_priority(a))
                .then_with(|| sortable_string(pool, a).cmp(&sortable_string(pool, b)))
        });

        let lines = format_deduplicated_rules(pool, request, &reasons);
        format!("\n    - {}", lines.join("\n    - "))
    }
}

/// `Problem::getRulePriority`.
fn rule_priority(reason: &Reason) -> u8 {
    match reason {
        Reason::Fixed { .. } => 3,
        Reason::RootRequire { .. } => 2,
        Reason::PackageConflict { .. } | Reason::PackageRequires { .. } => 1,
        Reason::PackageSameName(_)
        | Reason::Learned(_)
        | Reason::PackageAlias { .. }
        | Reason::PackageInverseAlias { .. } => 0,
    }
}

/// `Problem::getSortableString`. Only a tie-break key (never printed), so
/// `Reason::Learned`'s `implode('-', $rule->getLiterals())` — literals this
/// port's `Reason::Learned` doesn't carry — is approximated with its
/// `learnedPool` index instead: `reason_line` already renders `Learned`
/// (and both alias reasons) as `""`, and `format_deduplicated_rules` drops
/// empty lines outright, so none of the three ever reach the output
/// regardless of where this sorts them.
fn sortable_string(pool: &Pool, reason: &Reason) -> String {
    match reason {
        Reason::RootRequire { package_name, .. } => package_name.clone(),
        Reason::Fixed { package_index } => pool
            .package_by_id(pool::id_of(*package_index))
            .pretty_string(),
        Reason::PackageConflict {
            source_index,
            target,
            pretty_constraint,
            ..
        }
        | Reason::PackageRequires {
            source_index,
            target,
            pretty_constraint,
        } => {
            let source = pool.package_by_id(pool::id_of(*source_index));
            format!("{}//{target} {pretty_constraint}", source.pretty_string())
        }
        Reason::PackageSameName(name) => name.clone(),
        Reason::Learned(index) => index.to_string(),
        Reason::PackageAlias { alias_index } => pool
            .package_by_id(pool::id_of(*alias_index))
            .pretty_string(),
        Reason::PackageInverseAlias { alias_of_index } => pool
            .package_by_id(pool::id_of(*alias_of_index))
            .pretty_string(),
    }
}

/// `Rule::getPrettyString`, the lines `formatDeduplicatedRules` joins with
/// `"\n    - "` (the version-range dedup itself is not ported, see the
/// module doc).
fn reason_line(pool: &Pool, request: &Request, reason: &Reason) -> String {
    match reason {
        Reason::RootRequire {
            package_name,
            pretty_constraint,
        } => {
            let packages = pool.what_provides(package_name, None);
            if packages.is_empty() {
                return format!(
                    "No package found to satisfy root composer.json require {package_name} \
                     {pretty_constraint}"
                );
            }
            format!(
                "Root composer.json requires {package_name} {pretty_constraint} -> {}",
                satisfiable_or_found_suffix(pool, package_name, pretty_constraint)
            )
        }
        Reason::Fixed { package_index } => {
            let package = pool.package_by_id(pool::id_of(*package_index));
            format!(
                "{} is present at version {} and cannot be modified by Composer",
                package.name, package.pretty_version
            )
        }
        Reason::PackageConflict {
            source_index,
            target_index,
            ..
        } => {
            // `Rule::getPrettyString`'s default (no-swap) case: the
            // declaring package leads, and the conflicted-against package
            // names itself with its own resolved version (`$package1->
            // getPrettyString()`), not the raw `target`/`pretty_constraint`
            // this reason also carries (those describe the *link*, e.g.
            // `acme/plugin <2.0`, and only PHP's rarer "conflict points at
            // something the package provides/replaces, not itself" swap
            // branch — not reached by this port's fixtures — ever prints
            // that text instead of the resolved instance).
            let source = pool.package_by_id(pool::id_of(*source_index));
            let conflicter = pool.package_by_id(pool::id_of(*target_index));
            format!(
                "{} conflicts with {}.",
                source.pretty_string(),
                conflicter.pretty_string()
            )
        }
        Reason::PackageRequires {
            source_index,
            target,
            pretty_constraint,
        } => {
            let source = pool.package_by_id(pool::id_of(*source_index));
            // #238/#152: constraint-aware, matching `RULE_PACKAGE_REQUIRES`'s
            // own `count($requires) === 0` guard (`$requires` is the rule's
            // own literals, built from `whatProvides($target, $constraint)`)
            // — the plain `what_provides(target, None)` this used to check
            // only ruled out "no version of `target` exists anywhere",
            // missing every case where a *different*, unrelated version
            // exists but every version actually matching this constraint
            // was removed (advisory-filtered, or simply never published).
            let constraint = pretty_constraint_as_constraint(pretty_constraint);
            let matching = pool.what_provides(target, constraint.as_ref());
            if matching.is_empty() {
                format!(
                    "{} requires {target} {pretty_constraint} -> {}",
                    source.pretty_string(),
                    missing_package_suffix(pool, request, target, pretty_constraint)
                )
            } else {
                format!(
                    "{} requires {target} {pretty_constraint} -> satisfiable by {}.",
                    source.pretty_string(),
                    package_list(pool, &matching)
                )
            }
        }
        Reason::PackageSameName(name) => {
            format!("Only one package named {name} can be installed at a time")
        }
        Reason::PackageAlias { .. } | Reason::PackageInverseAlias { .. } | Reason::Learned(_) => {
            // Not a leaf reason this port's four message shapes need to
            // spell out: `analyze_unsolvable_rule` already recurses through
            // `Learned` down to the request-level reasons that actually
            // caused the conflict, and the two alias reasons only ever
            // co-occur with one of those in practice.
            String::new()
        }
    }
}

/// `Problem::getMissingPackageReason`'s `[prefix, suffix]` tuple, joined:
/// the platform-package, root-conflict (#152), security-advisory (#238) and
/// found-but-mismatched-constraint branches, plus the final
/// could-not-be-found-at-all fallback. Abandoned/filter-list/
/// lower-priority-repository still aren't reached: no abandoned-package
/// bookkeeping on a [`pool::RemovedPackage`] yet (nothing filters for it),
/// no filter-list feature, and one repository only. Minimum-stability is
/// also not reached here — see [`pool::RemovalReason`]'s own doc for why
/// that one is filtered a stage earlier than this module can see. Each
/// branch has its own prefix shape (a not-found-at-all package gets no
/// constraint text at all in the message, unlike the others), so this
/// isn't a single shared template; `missing_package_reason` picks the
/// shape from what `missing_package_suffix` actually returns, rather than
/// re-deriving the same branch condition a second time.
/// `Rule::getPrettyString`'s `-> satisfiable by ...` tail for
/// `Reason::RootRequire` (`Reason::PackageRequires`'s own arm above builds
/// its constraint-aware `matching` list directly now, #238/#152 — its
/// "nothing matches" case needs `missing_package_suffix`'s security-advisory
/// and root-conflict branches too, which this function doesn't have the
/// `Request` to check). #169: naively calling `what_provides(target, None)`
/// here — as this used to — ignores the constraint entirely, so a platform
/// package present at a non-matching version (`php ^7.3` against an assumed
/// `php[8.3.0]`) reported "satisfiable by" even though nothing satisfies the
/// requirement. A real Composer rule's literals are already
/// constraint-filtered by the time `getPrettyString` runs (`Rule2Literals`/
/// `GenericRule` are built from matching providers only); this re-filters
/// to match, and falls back to `Problem::getMissingPackageReason`'s
/// platform-specific "found X but it does not match the constraint."
/// wording when a platform package exists but nothing matches.
fn satisfiable_or_found_suffix(pool: &Pool, target: &str, pretty_constraint: &str) -> String {
    let providers = pool.what_provides(target, None);
    let matching = pretty_constraint_as_constraint(pretty_constraint)
        .map(|constraint| pool.what_provides(target, Some(&constraint)));
    match matching {
        Some(matching) if !matching.is_empty() => {
            format!("satisfiable by {}.", package_list(pool, &matching))
        }
        Some(_) if crate::repository::is_platform_package(target) && !providers.is_empty() => {
            format!(
                "found {} but it does not match the constraint.",
                package_list(pool, &providers)
            )
        }
        _ => format!("satisfiable by {}.", package_list(pool, &providers)),
    }
}

fn missing_package_reason(
    pool: &Pool,
    request: &Request,
    package_name: &str,
    pretty_constraint: &str,
) -> String {
    if crate::repository::is_platform_package(package_name) {
        return format!(
            "- Root composer.json requires {package_name} {pretty_constraint} but {}",
            missing_package_suffix(pool, request, package_name, pretty_constraint)
        );
    }
    // Every branch but the final could-not-be-found-at-all fallback keeps
    // the constraint in the prefix (`getMissingPackageReason`'s own
    // per-branch `[prefix, suffix]` pairs); reading it off the rendered
    // suffix, rather than re-running the same "is there a reason" checks a
    // second time here, is the only way this stays a single decision.
    let suffix = missing_package_suffix(pool, request, package_name, pretty_constraint);
    if suffix.starts_with("could not be found in any version") {
        return format!("- Root composer.json requires {package_name}, it {suffix}");
    }
    format!("- Root composer.json requires {package_name} {pretty_constraint}, {suffix}")
}

/// The suffix half of `getMissingPackageReason`'s `[prefix, suffix]` tuple:
/// also reused standalone by `Reason::PackageRequires`'s `-> ...` tail
/// (`Rule::getPrettyString`'s own `$text . ' -> ' . $reason[1]`, suffix
/// only, no prefix). Both callers only reach this once their own
/// "nothing at all provides this" check already failed, matching
/// Composer's own `count($packages) === 0`/`count($requires) === 0` guard
/// before either calls into `getMissingPackageReason`.
fn missing_package_suffix(
    pool: &Pool,
    request: &Request,
    package_name: &str,
    pretty_constraint: &str,
) -> String {
    if crate::repository::is_platform_package(package_name) {
        let installed = pool.what_provides(package_name, None);
        if let Some(&id) = installed.first() {
            let package = pool.package_by_id(id);
            return format!(
                "your {package_name} version ({}) does not satisfy that requirement.",
                package.pretty_version
            );
        }
        return format!("{package_name} is missing from your platform.");
    }

    // #238/#152: a name whose only matching candidates were filtered out
    // before the pool was built (a security advisory, or — root-conflict's
    // own extra check below — filtered *and* disjoint from a separate root
    // require for the same name) still has something to say beyond "never
    // existed". `removed_matching` only ever holds advisory removals today
    // (`pool::RemovalReason`'s own doc explains why minimum-stability isn't
    // here yet), so this is as far as either branch reaches.
    if let Some(constraint) = pretty_constraint_as_constraint(pretty_constraint) {
        let removed = pool.removed_matching(package_name, Some(&constraint));
        if !removed.is_empty() {
            if let Some(conflict) = root_conflict_suffix(request, package_name, &removed) {
                return conflict;
            }
            return advisory_suffix(&removed);
        }
    }

    let any_version = pool.what_provides(package_name, None);
    if any_version.is_empty() {
        return "could not be found in any version, there may be a typo in the package name."
            .to_string();
    }
    format!(
        "found {} but it does not match the constraint.",
        package_list(pool, &any_version)
    )
}

/// `getMissingPackageReason`'s root-conflict branch (#152): `removed`
/// candidates satisfy *this* requirement's own constraint (that's what
/// `removed_matching` above already filtered by) but were dropped from the
/// pool before solving; if the root also separately requires this same
/// name and none of `removed` would have satisfied *that* constraint
/// either, the real story isn't the removal reason at all, it's that the
/// two requirements can never share a version. `None` when the root
/// doesn't require this name, or its own constraint would have accepted
/// one of `removed` anyway (including the case where this call *is* the
/// root's own direct requirement: `removed` was filtered by the identical
/// constraint, so it trivially satisfies it, and this falls through to
/// [`advisory_suffix`] instead).
fn root_conflict_suffix(
    request: &Request,
    package_name: &str,
    removed: &[&pool::RemovedPackage],
) -> Option<String> {
    let root_require = request.requires.iter().find(|r| r.name == package_name)?;
    // No constraint (`"*"`) never conflicts with anything.
    let root_constraint = root_require.constraint.as_ref()?;
    if removed.iter().any(|r| root_constraint.matches(&r.version)) {
        return None;
    }
    Some(format!(
        "found {} but it conflicts with your root composer.json require ({}).",
        removed_package_list(removed),
        root_require.pretty_constraint
    ))
}

/// `getMissingPackageReason`'s security-advisory branch (#238), Composer's
/// own wording up to the ignore-config sentence: viv's equivalent config
/// key is `audit.ignore`, not Composer's `policy.advisories.*` (this crate
/// has no `policy.*` surface at all — see `AuditConfig`'s own doc), and the
/// bug this exists to fix is precisely that viv's original message named
/// neither remedy at all, so that closing sentence is viv's own rather
/// than a byte-for-byte port.
fn advisory_suffix(removed: &[&pool::RemovedPackage]) -> String {
    let mut ids: Vec<&str> = Vec::new();
    for package in removed {
        let pool::RemovalReason::Advisory(advisory_ids) = &package.reason;
        for id in advisory_ids {
            if !ids.contains(&id.as_str()) {
                ids.push(id);
            }
        }
    }
    format!(
        "found {} but these were not loaded, because they are affected by security advisories \
         (\"{}\"). Go to https://packagist.org/security-advisories/ to find advisory details. \
         Require a patched version to clear this, or override with --no-blocking (or add the \
         advisory to \"audit.ignore\") to install it anyway.",
        removed_package_list(removed),
        ids.join("\", \"")
    )
}

/// [`package_list`]'s counterpart for a removed candidate: the same
/// `name[v1, v2, ...]` shape, but built straight from [`pool::RemovedPackage`]
/// (a removed version was never in `pool`'s own package list to look up by
/// id).
fn removed_package_list(removed: &[&pool::RemovedPackage]) -> String {
    let name = &removed[0].name;
    let mut versions: Vec<(NormalizedVersion, String)> = Vec::new();
    for package in removed {
        match versions.iter_mut().find(|(v, _)| *v == package.version) {
            Some((_, pretty)) => pretty.clone_from(&package.pretty_version),
            None => versions.push((package.version.clone(), package.pretty_version.clone())),
        }
    }
    versions.sort_by(|a, b| semver::compare(&a.0, &b.0));
    format!(
        "{name}[{}]",
        condense_version_list(&versions, 4, 16).join(", ")
    )
}

/// Parses a rule's stored `pretty_constraint` text back into a
/// [`semver::Constraint`] so [`Pool::what_provides`] can filter by it: reasons
/// only keep the pretty string (see `Reason`'s own doc comment), not the
/// `Constraint` object the solver built the rule from. `None` on a parse
/// failure, which the caller treats as "can't tell, fall back to the
/// unfiltered list" rather than a hard error — this only ever renders a
/// diagnostic message.
fn pretty_constraint_as_constraint(pretty_constraint: &str) -> Option<semver::Constraint> {
    semver::parse_constraint(pretty_constraint).ok()
}

/// `Problem::getPackageList`, without the verbose/removed-version-group
/// bracketing: `name[v1, v2, ...]`, one entry per distinct normalised
/// version (`uksort($versions, 'version_compare')`), condensed the same
/// way a real `--no-verbose` run always is (`self::condenseVersionList($package['versions'], 4)`,
/// vivace has no `-v`/verbose distinction to gate this on).
fn package_list(pool: &Pool, ids: &[i32]) -> String {
    let mut by_name: Vec<(String, Vec<(NormalizedVersion, String)>)> = Vec::new();
    for &id in ids {
        let package = pool.package_by_id(id);
        let (_, versions) =
            if let Some(entry) = by_name.iter_mut().find(|(name, _)| *name == package.name) {
                entry
            } else {
                by_name.push((package.name.clone(), Vec::new()));
                by_name.last_mut().expect("just pushed")
            };
        match versions.iter_mut().find(|(v, _)| *v == package.version) {
            Some((_, pretty)) => pretty.clone_from(&package.pretty_version),
            None => versions.push((package.version.clone(), package.pretty_version.clone())),
        }
    }
    by_name
        .into_iter()
        .map(|(name, mut versions)| {
            versions.sort_by(|a, b| semver::compare(&a.0, &b.0));
            format!(
                "{name}[{}]",
                condense_version_list(&versions, 4, 16).join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `Problem::condenseVersionList`: below `max` total versions, every
/// pretty version as-is; at or above it, bucketed by major version (every
/// `dev-*` version sharing one `dev` bucket instead, capped at `max_dev`
/// rather than `max`) and each oversized bucket collapsed to its first and
/// last pretty version either side of a literal `"..."`, in the buckets'
/// order of first appearance (ascending, since `versions` is already
/// `version_compare`-sorted; a `dev` bucket sorts last, matching
/// `version_compare`'s own dev-is-newest ordering).
fn condense_version_list(
    versions: &[(NormalizedVersion, String)],
    max: usize,
    max_dev: usize,
) -> Vec<String> {
    if versions.len() <= max {
        return versions.iter().map(|(_, pretty)| pretty.clone()).collect();
    }

    let mut by_major: Vec<(String, Vec<String>)> = Vec::new();
    for (version, pretty) in versions {
        let major = major_bucket(version.as_str());
        match by_major.iter_mut().find(|(bucket, _)| *bucket == major) {
            Some((_, group)) => group.push(pretty.clone()),
            None => by_major.push((major, vec![pretty.clone()])),
        }
    }

    let mut filtered = Vec::new();
    for (major, group) in by_major {
        let threshold = if major == "dev" { max_dev } else { max };
        if group.len() > threshold {
            filtered.push(group[0].clone());
            filtered.push("...".to_string());
            filtered.push(group[group.len() - 1].clone());
        } else {
            filtered.extend(group);
        }
    }
    filtered
}

/// `condenseVersionList`'s own bucket key: `dev` for any `dev-*` normalised
/// version (`0 === stripos($version, 'dev-')`), else the leading run of
/// digits before the first `.` (`Preg::replace('{^(\d+)\..*}', '$1', ...)`,
/// which leaves a version with no such prefix as its own one-version
/// bucket — Composer normalises every version it can parse to start with a
/// numeric segment, so that fallback is dead in practice, not fixture'd).
fn major_bucket(normalized: &str) -> String {
    if normalized.len() >= 4 && normalized[..4].eq_ignore_ascii_case("dev-") {
        return "dev".to_string();
    }
    match normalized.split_once('.') {
        Some((major, _)) if !major.is_empty() && major.bytes().all(|b| b.is_ascii_digit()) => {
            major.to_string()
        }
        _ => normalized.to_string(),
    }
}

/// `Problem::formatDeduplicatedRules`: renders every sorted reason, then
/// merges the `PackageRequires`/`PackageConflict` lines that share every
/// word but their source package's name and version into one
/// `name[v1, v2] require target constraint -> ...` line (singularising
/// "requires"/"conflicts" to match), preserving each line's first-seen
/// position (`array_unique($messages)`'s own order) and, within one merged
/// line, each source package name's first-seen order too.
fn format_deduplicated_rules(pool: &Pool, request: &Request, reasons: &[&Reason]) -> Vec<String> {
    struct Group {
        /// Everything after `"{source pretty_string} "` in the rendered
        /// line: identical across every `Reason` this group merges.
        tail: String,
        by_source: Vec<(String, Vec<(NormalizedVersion, String)>)>,
    }

    let mut groups: Vec<Group> = Vec::new();
    let mut order: Vec<String> = Vec::new();

    for reason in reasons {
        let message = reason_line(pool, request, reason);
        if message.is_empty() {
            continue;
        }

        let dedup_target = match reason {
            Reason::PackageRequires { source_index, .. }
            | Reason::PackageConflict { source_index, .. } => {
                let source = pool.package_by_id(pool::id_of(*source_index));
                let prefix = format!("{} ", source.pretty_string());
                message
                    .strip_prefix(prefix.as_str())
                    .map(|tail| (source, tail.to_string()))
            }
            _ => None,
        };

        match dedup_target {
            Some((source, tail)) => {
                order.push(tail.clone());
                let group = if let Some(g) = groups.iter_mut().find(|g| g.tail == tail) {
                    g
                } else {
                    groups.push(Group {
                        tail,
                        by_source: Vec::new(),
                    });
                    groups.last_mut().expect("just pushed")
                };
                let (_, versions) = if let Some(entry) = group
                    .by_source
                    .iter_mut()
                    .find(|(name, _)| *name == source.name)
                {
                    entry
                } else {
                    group.by_source.push((source.name.clone(), Vec::new()));
                    group.by_source.last_mut().expect("just pushed")
                };
                match versions.iter_mut().find(|(v, _)| *v == source.version) {
                    Some((_, pretty)) => pretty.clone_from(&source.pretty_version),
                    None => versions.push((source.version.clone(), source.pretty_version.clone())),
                }
            }
            None => order.push(message),
        }
    }

    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for key in order {
        if !seen.insert(key.clone()) {
            continue;
        }
        match groups.iter().find(|g| g.tail == key) {
            Some(group) => {
                for (source_name, mut versions) in group.by_source.clone() {
                    versions.sort_by(|a, b| semver::compare(&a.0, &b.0));
                    let condensed = condense_version_list(&versions, 1, 16);
                    if condensed.len() > 1 {
                        result.push(format!(
                            "{source_name}[{}] {}",
                            condensed.join(", "),
                            depluralize(&group.tail)
                        ));
                    } else {
                        result.push(format!("{source_name} {} {}", condensed[0], group.tail));
                    }
                }
            }
            None => result.push(key),
        }
    }
    result
}

/// The grammar fix `formatDeduplicatedRules` applies once a line's source
/// package gets a `[v1, v2]` version list instead of one version: `requires`
/// -> `require`, `conflicts` -> `conflict` (only when either is the very
/// first word — `reason_line`'s two deduplicatable shapes always start a
/// tail with one of them).
fn depluralize(tail: &str) -> String {
    if let Some(rest) = tail.strip_prefix("requires ") {
        format!("require {rest}")
    } else if let Some(rest) = tail.strip_prefix("conflicts ") {
        format!("conflict {rest}")
    } else {
        tail.to_string()
    }
}

/// The solver's public error: one or more [`Problem`]s, pre-rendered
/// against the pool that produced them (`SolverError` used to hold
/// `Problem`s directly and implement `Display` itself; see the module
/// doc for why that moved to `Problem::pretty_string`).
#[derive(Default)]
pub struct SolverError {
    pub problems: Vec<String>,
}

impl SolverError {
    pub fn from_problems(problems: &[Problem], pool: &Pool, request: &Request) -> SolverError {
        SolverError {
            problems: problems
                .iter()
                .map(|p| p.pretty_string(pool, request))
                .collect(),
        }
    }
}

/// `SolverProblemsException::getPrettyString`'s "Potential causes" hint,
/// appended verbatim whenever any problem's text contains either phrase
/// (`str_contains($text, 'could not be found') ||
/// str_contains($text, 'no matching package found')`). The other hints
/// there (missing PHP extensions, `--with-all-dependencies`, two
/// ocramius/package-versions special cases) all depend on data this port
/// doesn't track (`isCausedByLock`, extension detection) and are not
/// reached by this stage's fixtures.
const TYPO_HINT: &str = "Potential causes:\n - A typo in the package name\n - The package is not \
     available in a stable-enough version according to your minimum-stability setting\n   see \
     <https://getcomposer.org/doc/04-schema.md#minimum-stability> for more details.\n - It's a \
     private package and you forgot to add a custom repository to find it\n\nRead \
     <https://getcomposer.org/doc/articles/troubleshooting.md> for further common problems.";

impl fmt::Display for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The blank line here is Composer's own: the fixed intro sentence a
        // command prints, then `SolverProblemsException::getPrettyString`'s
        // text, which itself starts with `"\n"` before the first `  Problem`.
        writeln!(
            f,
            "Your requirements could not be resolved to an installable set of packages.\n"
        )?;
        let mut any_not_found = false;
        for (i, problem) in self.problems.iter().enumerate() {
            // `problem` already starts with its own leading `\n` (`Problem::
            // getPrettyString`'s `"\n    ".implode(...)`/`"\n    - "` lead-in),
            // so `"  Problem N"` takes no newline of its own here — one
            // `writeln!` would double it into a blank line Composer's real
            // output never has.
            writeln!(f, "  Problem {}{problem}", i + 1)?;
            any_not_found |= problem.contains("could not be found")
                || problem.contains("no matching package found");
        }
        if any_not_found {
            writeln!(f, "\n{TYPO_HINT}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for SolverError {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{condense_version_list, major_bucket, missing_package_suffix};
    use crate::semver;
    use crate::solver::pool::{Package, Pool, RemovalReason, RemovedPackage};
    use crate::solver::request::{Request, RootRequire};

    /// #152/#238's own goldens (`tests/problem_messages.rs`) only reach the
    /// security-advisory branch: root-conflict needs a package name the
    /// root itself requires *and* a sibling that needs a wider constraint
    /// on the very same name, and viv's closure walk resolves a root-level
    /// name's candidates from whatever constraints are known at that name's
    /// own first fetch — a root require's own fetch always starts before a
    /// transitive one can contribute a wider constraint for the same name,
    /// so `acme/a`'s pool never actually gains the version root-conflict
    /// needs, the same "solver explores fewer candidates than libsolv" gap
    /// #62 already names for dedup. That's `pool_builder`/`Repository`'s
    /// closure breadth, not this function's own logic, so it's tested
    /// directly here instead — the same call `condense_version_list`'s own
    /// tests above already make for a live-solve-rarely-reaches-it reason.
    fn package(name: &str, pretty_version: &str) -> Package {
        let version = semver::normalize(pretty_version).unwrap();
        Package {
            stability: semver::stability(version.as_str()),
            is_dev: false,
            name: name.to_string(),
            version,
            pretty_version: pretty_version.to_string(),
            requires: Vec::new(),
            conflicts: Vec::new(),
            provides: Vec::new(),
            replaces: Vec::new(),
            alias_of: None,
            is_root_package_alias: false,
            has_self_version_requires: false,
            raw: Arc::new(serde_json::json!({"name": name, "version": pretty_version})),
        }
    }

    fn removed(name: &str, pretty_version: &str, advisory_ids: &[&str]) -> RemovedPackage {
        RemovedPackage {
            name: name.to_string(),
            version: semver::normalize(pretty_version).unwrap(),
            pretty_version: pretty_version.to_string(),
            reason: RemovalReason::Advisory(advisory_ids.iter().map(ToString::to_string).collect()),
        }
    }

    fn root_require(name: &str, pretty_constraint: &str) -> RootRequire {
        RootRequire {
            name: name.to_string(),
            constraint: semver::parse_constraint(pretty_constraint).ok(),
            pretty_constraint: pretty_constraint.to_string(),
        }
    }

    /// `getMissingPackageReason`'s root-conflict branch: a name's only
    /// pool-removed candidate satisfies the constraint being rendered, but
    /// not the root's own separate requirement for the same name.
    #[test]
    fn missing_package_suffix_prefers_root_conflict_over_the_advisory_message() {
        let pool = Pool::new(vec![package("acme/a", "1.5.0")]).with_removed(vec![removed(
            "acme/a",
            "2.0.0",
            &["PKSA-test-0004"],
        )]);
        let request = Request {
            requires: vec![root_require("acme/a", "^1.0")],
            fixed: Vec::new(),
        };

        assert_eq!(
            missing_package_suffix(&pool, &request, "acme/a", "^2.0"),
            "found acme/a[2.0.0] but it conflicts with your root composer.json require (^1.0)."
        );
    }

    /// Same removed candidate, but the root doesn't separately require this
    /// name (`root_conflict_suffix` has nothing to compare against) — falls
    /// through to the security-advisory branch instead.
    #[test]
    fn missing_package_suffix_reports_the_advisory_when_root_does_not_conflict() {
        let pool = Pool::new(vec![package("acme/a", "1.5.0")]).with_removed(vec![removed(
            "acme/a",
            "2.0.0",
            &["PKSA-test-0004"],
        )]);
        let request = Request {
            requires: Vec::new(),
            fixed: Vec::new(),
        };

        assert_eq!(
            missing_package_suffix(&pool, &request, "acme/a", "^2.0"),
            "found acme/a[2.0.0] but these were not loaded, because they are affected by \
             security advisories (\"PKSA-test-0004\"). Go to \
             https://packagist.org/security-advisories/ to find advisory details. Require a \
             patched version to clear this, or override with --no-blocking (or add the advisory \
             to \"audit.ignore\") to install it anyway."
        );
    }

    fn versions(pretty: &[&str]) -> Vec<(semver::NormalizedVersion, String)> {
        pretty
            .iter()
            .map(|v| (semver::normalize(v).unwrap(), (*v).to_string()))
            .collect()
    }

    /// Below `max`, `condenseVersionList` returns every pretty version
    /// untouched — no bucketing, no `"..."`.
    #[test]
    fn condense_version_list_keeps_a_short_list_as_is() {
        let versions = versions(&["1.0.0", "1.1.0"]);
        assert_eq!(
            condense_version_list(&versions, 4, 16),
            vec!["1.0.0".to_string(), "1.1.0".to_string()]
        );
    }

    /// At/above `max`, each major-version bucket over `max` collapses to
    /// its first and last pretty version either side of `"..."`; a bucket
    /// at or under `max` is kept whole (`condenseVersionList`, `Problem.php:629`).
    #[test]
    fn condense_version_list_collapses_an_oversized_major_bucket() {
        let versions = versions(&["1.0.0", "1.1.0", "1.2.0", "1.3.0", "1.4.0", "2.0.0"]);
        assert_eq!(
            condense_version_list(&versions, 4, 16),
            vec![
                "1.0.0".to_string(),
                "...".to_string(),
                "1.4.0".to_string(),
                "2.0.0".to_string(),
            ]
        );
    }

    /// `dev-*` versions share one `dev` bucket regardless of major, capped
    /// at `max_dev` rather than `max`.
    #[test]
    fn condense_version_list_buckets_dev_versions_together() {
        let versions = versions(&["1.0.0", "1.1.0", "dev-a", "dev-b", "dev-c"]);
        assert_eq!(
            condense_version_list(&versions, 4, 2),
            vec![
                "1.0.0".to_string(),
                "1.1.0".to_string(),
                "dev-a".to_string(),
                "...".to_string(),
                "dev-c".to_string(),
            ]
        );
    }

    #[test]
    fn major_bucket_groups_by_leading_digits() {
        assert_eq!(major_bucket("2.3.4"), "2");
        assert_eq!(major_bucket("dev-master"), "dev");
        assert_eq!(major_bucket("DEV-MASTER"), "dev");
    }
}
