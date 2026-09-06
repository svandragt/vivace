//! Port of `RuleWatchGraph.php`, `RuleWatchChain.php` and `RuleWatchNode.php`.
//!
//! Composer represents a chain as an `SplDoublyLinkedList` of shared
//! `RuleWatchNode` objects (the same node sits in both of its two watched
//! literals' chains, so moving a watch is visible from either chain).
//! Here every node lives once in a `nodes` arena and chains hold node
//! indices into it, which gives the same sharing without a `Rc<RefCell<_>>`
//! per node.

use crate::solver::decisions::Decisions;
use crate::solver::rules::{RuleKind, RuleSet};

struct WatchNode {
    rule_id: usize,
    watch1: i32,
    watch2: i32,
}

impl WatchNode {
    fn other_watch(&self, literal: i32) -> i32 {
        if self.watch1 == literal {
            self.watch2
        } else {
            self.watch1
        }
    }

    fn move_watch(&mut self, from: i32, to: i32) {
        if self.watch1 == from {
            self.watch1 = to;
        } else {
            self.watch2 = to;
        }
    }
}

pub struct RuleWatchGraph {
    nodes: Vec<WatchNode>,
    /// literal -> node indices, most-recently-inserted first
    /// (`RuleWatchChain::unshift`).
    chains: std::collections::HashMap<i32, Vec<usize>>,
}

impl RuleWatchGraph {
    pub fn new() -> RuleWatchGraph {
        RuleWatchGraph {
            nodes: Vec::new(),
            chains: std::collections::HashMap::new(),
        }
    }

    /// `RuleWatchGraph::insert(new RuleWatchNode($rule))` for the initial
    /// rule set, watching the rule's first two literals.
    pub fn insert(&mut self, rule_id: usize, rules: &RuleSet) {
        self.insert_with(rule_id, rules, None);
    }

    /// `RuleWatchNode::watch2OnHighest` followed by
    /// `RuleWatchGraph::insert`, for a freshly learned rule
    /// (`Solver::setPropagateLearn`): the second watch prefers the literal
    /// decided at the highest level, so it is the first to go live again on
    /// backjump. That pick has to land before the chains are built from it,
    /// so (unlike the PHP source, which mutates the node in place between
    /// two separate calls) this computes both watches up front.
    pub fn insert_learned(&mut self, rule_id: usize, rules: &RuleSet, decisions: &Decisions) {
        self.insert_with(rule_id, rules, Some(decisions));
    }

    fn insert_with(
        &mut self,
        rule_id: usize,
        rules: &RuleSet,
        watch2_decisions: Option<&Decisions>,
    ) {
        let rule = rules.rule(rule_id);
        if rule.is_assertion() {
            return;
        }

        let watch1 = rule.literals[0];
        let mut watch2 = rule.literals.get(1).copied().unwrap_or(0);

        if let Some(decisions) = watch2_decisions
            && rule.literals.len() >= 3
            && rule.kind != RuleKind::MultiConflictRule
        {
            let mut watch_level = 0;
            for &literal in &rule.literals {
                let level = decisions.decision_level(literal);
                if level > watch_level {
                    watch2 = literal;
                    watch_level = level;
                }
            }
        }

        let node_idx = self.nodes.len();
        self.nodes.push(WatchNode {
            rule_id,
            watch1,
            watch2,
        });

        if rule.kind == RuleKind::MultiConflictRule {
            for &literal in &rule.literals {
                self.chains.entry(literal).or_default().insert(0, node_idx);
            }
        } else {
            for literal in [watch1, watch2] {
                self.chains.entry(literal).or_default().insert(0, node_idx);
            }
        }
    }

    /// `RuleWatchGraph::propagateLiteral`. Returns the conflicting rule id,
    /// if any.
    pub fn propagate_literal(
        &mut self,
        decided_literal: i32,
        level: i32,
        decisions: &mut Decisions,
        rules: &RuleSet,
    ) -> Option<usize> {
        let literal = -decided_literal;
        let mut pos = 0usize;
        loop {
            // Re-fetched every iteration (rather than snapshotting the
            // chain up front) so a `move_watch` earlier in this same call
            // is visible immediately, matching the PHP source's shared,
            // live linked list.
            let &node_idx = self.chains.get(&literal).and_then(|c| c.get(pos))?;
            let rule_id = self.nodes[node_idx].rule_id;
            let rule = rules.rule(rule_id);

            if rule.kind == RuleKind::MultiConflictRule {
                for other_literal in rule.literals.clone() {
                    if literal != other_literal && !decisions.satisfy(other_literal) {
                        if decisions.conflict(other_literal) {
                            return Some(rule_id);
                        }
                        decisions.decide(other_literal, level, rule_id);
                    }
                }
            } else {
                let other_watch = self.nodes[node_idx].other_watch(literal);

                if !rule.is_enabled() || decisions.satisfy(other_watch) {
                    pos += 1;
                    continue;
                }

                let alternative = rule
                    .literals
                    .iter()
                    .copied()
                    .find(|&l| l != literal && l != other_watch && !decisions.conflict(l));

                if let Some(alt) = alternative {
                    self.move_watch(literal, alt, node_idx);
                    continue;
                }

                if decisions.conflict(other_watch) {
                    return Some(rule_id);
                }

                decisions.decide(other_watch, level, rule_id);
            }

            pos += 1;
        }
    }

    fn move_watch(&mut self, from: i32, to: i32, node_idx: usize) {
        self.nodes[node_idx].move_watch(from, to);
        if let Some(chain) = self.chains.get_mut(&from)
            && let Some(pos) = chain.iter().position(|&idx| idx == node_idx)
        {
            chain.remove(pos);
        }
        self.chains.entry(to).or_default().insert(0, node_idx);
    }
}

impl Default for RuleWatchGraph {
    fn default() -> Self {
        RuleWatchGraph::new()
    }
}
