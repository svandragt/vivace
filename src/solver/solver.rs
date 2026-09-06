//! Port of `DependencyResolver/Solver.php`: `setup`, `propagate`,
//! `analyze`/backjump, `selectAndInstall`, `runSat`, learned-rule handling.
//!
//! No `PlatformRequirementFilter` (`--ignore-platform-req(s)`, out of scope:
//! no CLI wiring this stage) and no `checkForFilterListRemovedLockedPackages`
//! (no locked repository exists in a full update, see `request.rs`).

use std::collections::{HashMap, HashSet};

use crate::solver::decisions::Decisions;
use crate::solver::policy::DefaultPolicy;
use crate::solver::pool::{self, Pool};
use crate::solver::problem::{Problem, SolverError};
use crate::solver::request::Request;
use crate::solver::rule_set_generator;
use crate::solver::rules::{Reason, RuleKind, RuleSet, RuleType};
use crate::solver::watch_graph::RuleWatchGraph;

struct Solver<'a> {
    policy: &'a DefaultPolicy,
    pool: &'a Pool,
    rules: RuleSet,
    watch_graph: RuleWatchGraph,
    decisions: Decisions,
    /// Pool ids `Request::fixPackage` marked (platform packages): once any
    /// of them is a candidate, `runSat`'s root-require/fixed handling
    /// prunes the decision queue down to just those (`Solver.php:641-652`).
    fixed_map: HashSet<i32>,
    propagate_index: usize,
    /// `Solver::BRANCH_LITERALS`/`BRANCH_LEVEL`: remaining alternatives and
    /// the level they were branched at.
    branches: Vec<(Vec<i32>, i32)>,
    problems: Vec<Problem>,
    /// `learnedPool`: each learned rule's contributing rule ids, indexed by
    /// `analyze`'s `$why`.
    learned_pool: Vec<Vec<usize>>,
    /// `learnedWhy`: learned rule id -> its `learnedPool` index (identical
    /// information to the index itself right now, since rules are only
    /// ever learned once, but kept as its own map to mirror the source and
    /// leave room for that to stop being true).
    learned_why: HashMap<usize, usize>,
}

/// `Solver::solve`. Returns the pool ids the solver decided to install.
pub fn solve(
    policy: &DefaultPolicy,
    pool: &Pool,
    request: &Request,
) -> Result<Vec<i32>, SolverError> {
    let mut solver = Solver {
        policy,
        pool,
        rules: RuleSet::new(),
        watch_graph: RuleWatchGraph::new(),
        decisions: Decisions::new(pool.len()),
        fixed_map: request
            .fixed
            .iter()
            .map(|&index| pool::id_of(index))
            .collect(),
        propagate_index: 0,
        branches: Vec::new(),
        problems: Vec::new(),
        learned_pool: Vec::new(),
        learned_why: HashMap::new(),
    };

    solver.rules = rule_set_generator::rules_for(pool, request);
    solver.check_for_root_require_problems(request);

    for rule_id in solver.rules.iteration_order() {
        solver.watch_graph.insert(rule_id, &solver.rules);
    }

    solver.make_assertion_rule_decisions();
    solver.run_sat();

    if !solver.problems.is_empty() {
        return Err(SolverError::from_problems(&solver.problems, pool));
    }

    let mut installed: Vec<i32> = solver
        .decisions
        .iter()
        .filter(|&(literal, _)| literal > 0)
        .map(|(literal, _)| literal)
        .collect();
    installed.sort_unstable();
    installed.dedup();
    Ok(installed)
}

impl Solver<'_> {
    /// `Solver::checkForRootRequireProblems`.
    fn check_for_root_require_problems(&mut self, request: &Request) {
        for require in &request.requires {
            if self
                .pool
                .what_provides(&require.name, require.constraint.as_ref())
                .is_empty()
            {
                let mut problem = Problem::new();
                problem.add_reason(Reason::RootRequire {
                    package_name: require.name.clone(),
                    pretty_constraint: require.pretty_constraint.clone(),
                });
                self.problems.push(problem);
            }
        }
    }

    /// `Solver::makeAssertionRuleDecisions` (PHP's own alias for this:
    /// `solver_makeruledecisions`).
    fn make_assertion_rule_decisions(&mut self) {
        let decision_start = i32::try_from(self.decisions.len()).unwrap_or(i32::MAX) - 1;
        let rules_count = self.rules.len();
        let mut rule_index = 0usize;

        while rule_index < rules_count {
            let rule_id = rule_index;
            let mut restart = false;

            let rule = self.rules.rule(rule_id);
            if rule.is_assertion() && !rule.disabled {
                let literal = rule.literals[0];

                if !self.decisions.decided(literal) {
                    self.decisions.decide(literal, 1, rule_id);
                } else if !self.decisions.satisfy(literal) {
                    if self.rules.rule(rule_id).rule_type == RuleType::Learned {
                        self.rules.rule_mut(rule_id).disabled = true;
                    } else {
                        let conflict_id = self.decisions.decision_rule(literal);

                        if self.rules.rule(conflict_id).rule_type == RuleType::Package {
                            let mut problem = Problem::new();
                            problem.add_reason(self.rules.rule(rule_id).reason.clone());
                            problem.add_reason(self.rules.rule(conflict_id).reason.clone());
                            self.rules.rule_mut(rule_id).disabled = true;
                            self.problems.push(problem);
                        } else {
                            let mut problem = Problem::new();
                            problem.add_reason(self.rules.rule(rule_id).reason.clone());
                            problem.add_reason(self.rules.rule(conflict_id).reason.clone());

                            for assert_id in self.rules.request_rule_ids().to_vec() {
                                let assert_rule = self.rules.rule(assert_id);
                                if assert_rule.disabled || !assert_rule.is_assertion() {
                                    continue;
                                }
                                if literal.abs() != assert_rule.literals[0].abs() {
                                    continue;
                                }
                                problem.add_reason(assert_rule.reason.clone());
                                self.rules.rule_mut(assert_id).disabled = true;
                            }
                            self.problems.push(problem);

                            self.decisions.reset_to_offset(decision_start);
                            restart = true;
                        }
                    }
                }
            }

            rule_index = if restart { 0 } else { rule_index + 1 };
        }
    }

    /// `Solver::propagate`.
    fn propagate(&mut self, level: i32) -> Option<usize> {
        while self.decisions.valid_offset(self.propagate_index) {
            let (literal, _) = self.decisions.at_offset(self.propagate_index);
            let conflict = self.watch_graph.propagate_literal(
                literal,
                level,
                &mut self.decisions,
                &self.rules,
            );
            self.propagate_index += 1;
            if conflict.is_some() {
                return conflict;
            }
        }
        None
    }

    /// `Solver::revert`.
    fn revert(&mut self, level: i32) {
        while !self.decisions.is_empty() {
            let literal = self.decisions.last_literal();
            if self.decisions.undecided(literal) {
                break;
            }
            if self.decisions.decision_level(literal) <= level {
                break;
            }
            self.decisions.revert_last();
            self.propagate_index = self.decisions.len();
        }

        while self.branches.last().is_some_and(|(_, l)| *l >= level) {
            self.branches.pop();
        }
    }

    /// `Solver::setPropagateLearn`.
    fn set_propagate_learn(&mut self, level: i32, literal: i32, reason_rule_id: usize) -> i32 {
        let mut level = level + 1;
        self.decisions.decide(literal, level, reason_rule_id);

        while let Some(conflict_rule_id) = self.propagate(level) {
            if level == 1 {
                self.analyze_unsolvable(conflict_rule_id);
                return 0;
            }

            let (learn_literal, new_level, new_literals, why) =
                self.analyze(level, conflict_rule_id);
            assert!(
                new_level > 0 && new_level < level,
                "solver bug: invalid backjump from level {level} to {new_level}"
            );
            level = new_level;
            self.revert(level);

            let new_rule_id = self.rules.add_learned(new_literals, Reason::Learned(why));
            self.learned_why.insert(new_rule_id, why);
            self.watch_graph
                .insert_learned(new_rule_id, &self.rules, &self.decisions);

            self.decisions.decide(learn_literal, level, new_rule_id);
        }

        level
    }

    /// `Solver::selectAndInstall`.
    fn select_and_install(&mut self, level: i32, decision_queue: &[i32], rule_id: usize) -> i32 {
        let required_package = self
            .rules
            .rule(rule_id)
            .reason
            .required_package()
            .map(str::to_string);
        let mut literals = self.policy.select_preferred_packages(
            self.pool,
            decision_queue,
            required_package.as_deref(),
        );

        let selected = literals.remove(0);
        if !literals.is_empty() {
            self.branches.push((literals, level));
        }

        self.set_propagate_learn(level, selected, rule_id)
    }

    /// `Solver::analyze`. Returns `(learnLiteral, ruleLevel, newRule
    /// literals, why)`.
    fn analyze(&mut self, level: i32, rule_id: usize) -> (i32, i32, Vec<i32>, usize) {
        let mut rule_id = rule_id;
        let mut rule_level = 1;
        let mut num = 0i32;
        let mut l1num = 0i32;
        let mut seen: HashSet<i32> = HashSet::new();
        let mut learned_literal: Option<i32> = None;
        let mut other_learned_literals: Vec<i32> = Vec::new();
        let mut decision_id = self.decisions.len();

        self.learned_pool.push(Vec::new());
        let why = self.learned_pool.len() - 1;

        'outer: loop {
            self.learned_pool[why].push(rule_id);

            let rule = self.rules.rule(rule_id);
            let is_multi = rule.kind == RuleKind::MultiConflictRule;
            for literal in rule.literals.clone() {
                if is_multi && !self.decisions.decided(literal) {
                    continue;
                }
                if self.decisions.satisfy(literal) {
                    continue;
                }
                if !seen.insert(literal.abs()) {
                    continue;
                }
                let l = self.decisions.decision_level(literal);
                if l == 1 {
                    l1num += 1;
                } else if level == l {
                    num += 1;
                } else {
                    other_learned_literals.push(literal);
                    if l > rule_level {
                        rule_level = l;
                    }
                }
            }

            let mut l1retry = true;
            while l1retry {
                l1retry = false;

                if num == 0 {
                    l1num -= 1;
                    if l1num == 0 {
                        break 'outer;
                    }
                }

                let literal;
                loop {
                    assert!(
                        decision_id > 0,
                        "solver bug: ran out of decisions analyzing rule {rule_id}"
                    );
                    decision_id -= 1;
                    let (l, _) = self.decisions.at_offset(decision_id);
                    if seen.contains(&l.abs()) {
                        literal = l;
                        break;
                    }
                }
                seen.remove(&literal.abs());

                let mut took_learned_branch = false;
                if num != 0 {
                    num -= 1;
                    if num == 0 {
                        took_learned_branch = true;
                        learned_literal = Some(-literal);
                        if l1num == 0 {
                            break 'outer;
                        }
                        for other in &other_learned_literals {
                            seen.remove(&other.abs());
                        }
                        l1num += 1;
                        l1retry = true;
                    }
                }

                if !took_learned_branch {
                    let (_, reason_rule_id) = self.decisions.at_offset(decision_id);
                    if self.rules.rule(reason_rule_id).kind == RuleKind::MultiConflictRule {
                        for rule_literal in self.rules.rule(reason_rule_id).literals.clone() {
                            if !seen.contains(&rule_literal.abs())
                                && self.decisions.satisfy(-rule_literal)
                            {
                                self.learned_pool[why].push(reason_rule_id);
                                let l = self.decisions.decision_level(rule_literal);
                                if l == 1 {
                                    l1num += 1;
                                } else if level == l {
                                    num += 1;
                                } else {
                                    other_learned_literals.push(rule_literal);
                                    if l > rule_level {
                                        rule_level = l;
                                    }
                                }
                                seen.insert(rule_literal.abs());
                                break;
                            }
                        }
                        l1retry = true;
                    }
                }
            }

            let (_, reason_rule_id) = self.decisions.at_offset(decision_id);
            rule_id = reason_rule_id;
        }

        let learned_literal =
            learned_literal.expect("solver bug: no learnable literal in analyzed rule");
        let mut literals = vec![learned_literal];
        literals.extend(other_learned_literals);

        (learned_literal, rule_level, literals, why)
    }

    /// `Solver::analyzeUnsolvableRule`.
    fn analyze_unsolvable_rule(
        &self,
        problem: &mut Problem,
        rule_id: usize,
        rule_seen: &mut HashSet<usize>,
    ) {
        if !rule_seen.insert(rule_id) {
            return;
        }

        let rule = self.rules.rule(rule_id);
        if rule.rule_type == RuleType::Learned {
            for &contributing in &self.learned_pool[self.learned_why[&rule_id]].clone() {
                if !rule_seen.contains(&contributing) {
                    self.analyze_unsolvable_rule(problem, contributing, rule_seen);
                }
            }
            return;
        }

        if rule.rule_type == RuleType::Package {
            // Package rules cannot be part of a problem (Rule.php's own
            // comment): they are internal implication rules, never
            // something a root require or fixed package chose.
            return;
        }

        problem.add_reason(rule.reason.clone());
    }

    /// `Solver::analyzeUnsolvable`.
    fn analyze_unsolvable(&mut self, conflict_rule_id: usize) {
        let mut problem = Problem::new();
        problem.add_reason(self.rules.rule(conflict_rule_id).reason.clone());

        let mut rule_seen = HashSet::new();
        self.analyze_unsolvable_rule(&mut problem, conflict_rule_id, &mut rule_seen);

        let mut seen: HashSet<i32> = HashSet::new();
        for literal in self.rules.rule(conflict_rule_id).literals.clone() {
            if self.decisions.satisfy(literal) {
                continue;
            }
            seen.insert(literal.abs());
        }

        // Newest-decision-first, matching `Decisions`'s own `Iterator`
        // (`rewind()` calls `end()`): later decisions can add package ids
        // to `seen` that an earlier (in real time) decision needs to
        // already be present, so the direction here is not cosmetic.
        for offset in (0..self.decisions.len()).rev() {
            let (decision_literal, why) = self.decisions.at_offset(offset);
            if !seen.contains(&decision_literal.abs()) {
                continue;
            }
            problem.add_reason(self.rules.rule(why).reason.clone());
            self.analyze_unsolvable_rule(&mut problem, why, &mut rule_seen);

            for literal in self.rules.rule(why).literals.clone() {
                if self.decisions.satisfy(literal) {
                    continue;
                }
                seen.insert(literal.abs());
            }
        }

        self.problems.push(problem);
    }

    /// `Solver::runSat`.
    fn run_sat(&mut self) {
        self.propagate_index = 0;

        let mut level = 1;
        let mut system_level = level + 1;

        loop {
            if level == 1
                && let Some(conflict_rule_id) = self.propagate(level)
            {
                self.analyze_unsolvable(conflict_rule_id);
                return;
            }

            if level < system_level {
                let request_ids = self.rules.request_rule_ids().to_vec();
                let mut broke_at: Option<usize> = None;

                for (pos, &rule_id) in request_ids.iter().enumerate() {
                    if !self.rules.rule(rule_id).is_enabled() {
                        continue;
                    }

                    let mut decision_queue = Vec::new();
                    let mut none_satisfied = true;
                    for literal in self.rules.rule(rule_id).literals.clone() {
                        if self.decisions.satisfy(literal) {
                            none_satisfied = false;
                            break;
                        }
                        if literal > 0 && self.decisions.undecided(literal) {
                            decision_queue.push(literal);
                        }
                    }

                    if none_satisfied && !decision_queue.is_empty() {
                        let pruned: Vec<i32> = decision_queue
                            .iter()
                            .copied()
                            .filter(|literal| self.fixed_map.contains(&literal.abs()))
                            .collect();
                        if !pruned.is_empty() {
                            decision_queue = pruned;
                        }
                    }

                    if none_satisfied && !decision_queue.is_empty() {
                        let o_level = level;
                        level = self.select_and_install(level, &decision_queue, rule_id);
                        if level == 0 {
                            return;
                        }
                        if level <= o_level {
                            broke_at = Some(pos);
                            break;
                        }
                    }
                }

                system_level = level + 1;

                if let Some(pos) = broke_at
                    && pos + 1 < request_ids.len()
                {
                    continue;
                }
            }

            if level < system_level {
                system_level = level;
            }

            let mut rules_count = self.rules.len();
            let mut i = 0usize;
            let mut n = 0usize;

            while n < rules_count {
                if i == rules_count {
                    i = 0;
                }
                let rule_id = i;
                let mut just_installed = false;

                if self.rules.rule(rule_id).is_enabled() {
                    let literals = self.rules.rule(rule_id).literals.clone();
                    let mut decision_queue = Vec::new();
                    let mut skip = false;

                    for literal in literals {
                        if literal <= 0 {
                            if !self.decisions.decided_install(literal) {
                                skip = true;
                                break;
                            }
                        } else {
                            if self.decisions.decided_install(literal) {
                                skip = true;
                                break;
                            }
                            if self.decisions.undecided(literal) {
                                decision_queue.push(literal);
                            }
                        }
                    }

                    if !skip && decision_queue.len() >= 2 {
                        level = self.select_and_install(level, &decision_queue, rule_id);
                        if level == 0 {
                            return;
                        }
                        rules_count = self.rules.len();
                        just_installed = true;
                    }
                }

                i += 1;
                n = if just_installed { 0 } else { n + 1 };
            }

            if level < system_level {
                continue;
            }

            if !self.branches.is_empty() {
                let mut chosen: Option<(i32, i32, usize, usize)> = None;
                for branch_index in (0..self.branches.len()).rev() {
                    let (literals, branch_level) = &self.branches[branch_index];
                    for (offset, &literal) in literals.iter().enumerate() {
                        if literal > 0 && self.decisions.decision_level(literal) > branch_level + 1
                        {
                            chosen = Some((literal, *branch_level, branch_index, offset));
                        }
                    }
                }

                if let Some((last_literal, last_level, branch_index, offset)) = chosen {
                    self.branches[branch_index].0.remove(offset);

                    level = last_level;
                    self.revert(level);

                    let why = self.decisions.last_reason();
                    level = self.set_propagate_learn(level, last_literal, why);
                    if level == 0 {
                        return;
                    }
                    continue;
                }
            }

            break;
        }
    }
}
