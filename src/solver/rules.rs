//! Port of `Rule.php`, `Rule2Literals.php`, `GenericRule.php`,
//! `MultiConflictRule.php`, `RuleSet.php` and `RuleSetIterator.php`.
//!
//! Composer spreads a rule's shape across three `Rule` subclasses
//! (`GenericRule`: N literals, `Rule2Literals`: exactly 2, `MultiConflictRule`:
//! N literals with different watch-graph semantics, see `watch_graph.rs`).
//! Rust has no subclassing, and the shape is fully described by the literal
//! list plus one tag, so [`Rule`] holds both `GenericRule` and
//! `Rule2Literals` as one `Normal` [`RuleKind`] (their `equals()` already
//! cross-compares by literals alone, ignoring which PHP class either side
//! is) and keeps `MultiConflictRule` distinct, matching
//! `MultiConflictRule::equals`'s explicit type check.

use std::collections::HashMap;

/// `Rule::RULE_*` plus the reason data each one carries
/// (`@phpstan-type ReasonData` in `Rule.php`). Composer stores the reason
/// data untyped (`mixed`) and dispatches on the reason constant; storing it
/// pretty-printed up front (rather than as `Link`/`BasePackage` references)
/// is enough for this stage's blunt `Problem` port (`problem.rs`) and
/// avoids needing `Constraint`, which isn't `Clone`, inside a value that
/// outlives the pool build.
#[derive(Clone)]
pub enum Reason {
    /// `RULE_ROOT_REQUIRE`.
    RootRequire {
        package_name: String,
        pretty_constraint: String,
    },
    /// `RULE_FIXED`.
    Fixed { package_index: usize },
    /// `RULE_PACKAGE_CONFLICT`.
    PackageConflict {
        source_index: usize,
        target: String,
        pretty_constraint: String,
    },
    /// `RULE_PACKAGE_REQUIRES`.
    PackageRequires {
        source_index: usize,
        target: String,
        pretty_constraint: String,
    },
    /// `RULE_PACKAGE_SAME_NAME`: the replaced/shared name.
    PackageSameName(String),
    /// `RULE_LEARNED`: the `learnedPool`/`learnedWhy` index (`analyze`'s
    /// `$why`).
    Learned(usize),
    /// `RULE_PACKAGE_ALIAS`: the alias package's pool index.
    PackageAlias { alias_index: usize },
    /// `RULE_PACKAGE_INVERSE_ALIAS`: the aliased package's pool index.
    PackageInverseAlias { alias_of_index: usize },
}

impl Reason {
    /// `Rule::getRequiredPackage`, used by `DefaultPolicy::selectPreferredPackages`'s
    /// `$requiredPackage` (same-vendor tie-break).
    pub fn required_package(&self) -> Option<&str> {
        match self {
            Reason::RootRequire { package_name, .. } => Some(package_name),
            Reason::PackageRequires { target, .. } => Some(target),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    /// `GenericRule` or `Rule2Literals`.
    Normal,
    MultiConflictRule,
}

/// `RuleSet::TYPE_*`, highest priority first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RuleType {
    Package,
    Request,
    Learned,
}

pub struct Rule {
    /// Always sorted ascending (`sort($literals)` in every PHP constructor).
    pub literals: Vec<i32>,
    pub kind: RuleKind,
    pub reason: Reason,
    pub rule_type: RuleType,
    pub disabled: bool,
}

impl Rule {
    fn new(mut literals: Vec<i32>, kind: RuleKind, reason: Reason, rule_type: RuleType) -> Rule {
        literals.sort_unstable();
        Rule {
            literals,
            kind,
            reason,
            rule_type,
            disabled: false,
        }
    }

    /// `GenericRule::isAssertion`/`Rule2Literals::isAssertion` (always
    /// false)/`MultiConflictRule::isAssertion` (always false): true only for
    /// a single-literal `Normal` rule.
    pub fn is_assertion(&self) -> bool {
        self.kind == RuleKind::Normal && self.literals.len() == 1
    }

    pub fn is_enabled(&self) -> bool {
        !self.disabled
    }
}

/// `RuleSet.php` plus `RuleSetIterator.php`'s ordering, minus the pretty-printer
/// (`Problem.php`, stage 5) and the `getIteratorWithout` nobody here calls.
pub struct RuleSet {
    rules: Vec<Rule>,
    package: Vec<usize>,
    request: Vec<usize>,
    learned: Vec<usize>,
    /// Dedup key, matching `RuleSet::add`'s hash+`equals` check: `(kind,
    /// literals)` is an exact, collision-free replacement for Composer's
    /// 32-bit rule hash plus its `rulesByHash` collision list.
    by_shape: HashMap<(bool, Vec<i32>), usize>,
}

impl RuleSet {
    pub fn new() -> RuleSet {
        RuleSet {
            rules: Vec::new(),
            package: Vec::new(),
            request: Vec::new(),
            learned: Vec::new(),
            by_shape: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn rule(&self, id: usize) -> &Rule {
        &self.rules[id]
    }

    pub fn rule_mut(&mut self, id: usize) -> &mut Rule {
        &mut self.rules[id]
    }

    pub fn package_rule_ids(&self) -> &[usize] {
        &self.package
    }

    pub fn request_rule_ids(&self) -> &[usize] {
        &self.request
    }

    /// `RuleSet::getIterator()`'s order: `TYPE_PACKAGE` rules (in their own
    /// insertion order), then `TYPE_REQUEST`, then `TYPE_LEARNED`. Grouped
    /// by type, unlike `ruleById`'s flat global insertion order (what
    /// `rule`/`rule_mut` index by) — `Solver::solve`'s initial watch-graph
    /// population walks the ruleset this way (`foreach ($this->rules as
    /// $rule)`), so the two orders have to stay distinct here too.
    pub fn iteration_order(&self) -> Vec<usize> {
        let mut ids = Vec::with_capacity(self.rules.len());
        ids.extend_from_slice(&self.package);
        ids.extend_from_slice(&self.request);
        ids.extend_from_slice(&self.learned);
        ids
    }

    /// `RuleSet::add`. Returns the new or pre-existing rule's id.
    pub fn add(
        &mut self,
        literals: Vec<i32>,
        kind: RuleKind,
        reason: Reason,
        rule_type: RuleType,
    ) -> usize {
        let rule = Rule::new(literals, kind, reason, rule_type);
        let shape = (
            rule.kind == RuleKind::MultiConflictRule,
            rule.literals.clone(),
        );
        if let Some(&existing) = self.by_shape.get(&shape) {
            return existing;
        }

        let id = self.rules.len();
        match rule_type {
            RuleType::Package => self.package.push(id),
            RuleType::Request => self.request.push(id),
            RuleType::Learned => self.learned.push(id),
        }
        self.by_shape.insert(shape, id);
        self.rules.push(rule);
        id
    }

    /// `Rule::RULE_LEARNED` rules skip the dedup table: `analyze` always
    /// produces a fresh combination and `Solver::setPropagateLearn` needs
    /// the id back regardless (`$this->learnedWhy[spl_object_hash($newRule)]`).
    pub fn add_learned(&mut self, literals: Vec<i32>, reason: Reason) -> usize {
        let id = self.rules.len();
        self.learned.push(id);
        self.rules.push(Rule::new(
            literals,
            RuleKind::Normal,
            reason,
            RuleType::Learned,
        ));
        id
    }
}

impl Default for RuleSet {
    fn default() -> Self {
        RuleSet::new()
    }
}
