//! Port of `DependencyResolver/Decisions.php`.
//!
//! Composer's `decisionQueue` entries carry a `Rule` object reference for
//! `DECISION_REASON`; here that's a rule id into the `RuleSet` the solver
//! already owns (`rules.rs`'s dedup table makes ids stable once assigned).

/// One `[literal, level, reason]` triple.
struct Decision {
    literal: i32,
    reason: usize,
}

pub struct Decisions {
    /// `decisionMap`: package id -> signed level (positive = installed,
    /// negative = not installed, 0/absent = undecided).
    level_by_package: Vec<i32>,
    queue: Vec<Decision>,
}

/// `abs($literalOrPackageId)` used as a `decisionMap`/`level_by_package`
/// index: package ids are always small and non-negative, so the
/// `u32`-as-`usize` widening this does is exact, never a truncation.
#[allow(clippy::cast_sign_loss)]
fn package_index(literal_or_package_id: i32) -> usize {
    literal_or_package_id.unsigned_abs() as usize
}

impl Decisions {
    pub fn new(pool_len: usize) -> Decisions {
        Decisions {
            level_by_package: vec![0; pool_len + 1],
            queue: Vec::new(),
        }
    }

    pub fn decide(&mut self, literal: i32, level: i32, reason: usize) {
        self.add_decision(literal, level);
        self.queue.push(Decision { literal, reason });
    }

    pub fn satisfy(&self, literal: i32) -> bool {
        let level = self.level_by_package[package_index(literal)];
        (literal > 0 && level > 0) || (literal < 0 && level < 0)
    }

    pub fn conflict(&self, literal: i32) -> bool {
        let level = self.level_by_package[package_index(literal)];
        (level > 0 && literal < 0) || (level < 0 && literal > 0)
    }

    pub fn decided(&self, literal_or_package_id: i32) -> bool {
        self.level_by_package[package_index(literal_or_package_id)] != 0
    }

    pub fn undecided(&self, literal_or_package_id: i32) -> bool {
        !self.decided(literal_or_package_id)
    }

    pub fn decided_install(&self, literal_or_package_id: i32) -> bool {
        self.level_by_package[package_index(literal_or_package_id)] > 0
    }

    pub fn decision_level(&self, literal_or_package_id: i32) -> i32 {
        self.level_by_package[package_index(literal_or_package_id)].abs()
    }

    /// `Decisions::decisionRule`: the reason for the first decision on this
    /// package id, scanning the queue like the PHP source (a linear scan,
    /// not indexed by package: only called from
    /// `Solver::makeAssertionRuleDecisions`'s conflict path, which fires
    /// once per solve at most).
    pub fn decision_rule(&self, literal_or_package_id: i32) -> usize {
        let package_id = literal_or_package_id.abs();
        self.queue
            .iter()
            .find(|d| d.literal.abs() == package_id)
            .map(|d| d.reason)
            .expect("decision_rule: no decision recorded for this package")
    }

    pub fn at_offset(&self, offset: usize) -> (i32, usize) {
        let d = &self.queue[offset];
        (d.literal, d.reason)
    }

    pub fn valid_offset(&self, offset: usize) -> bool {
        offset < self.queue.len()
    }

    pub fn last_reason(&self) -> usize {
        self.queue.last().expect("no decisions made yet").reason
    }

    pub fn last_literal(&self) -> i32 {
        self.queue.last().expect("no decisions made yet").literal
    }

    /// `Decisions::resetToOffset`, `offset` matching PHP's `int<-1, max>`
    /// (an offset of `-1` drops every decision).
    pub fn reset_to_offset(&mut self, offset: i32) {
        let keep = if offset < 0 {
            0
        } else {
            offset.unsigned_abs() as usize + 1
        };
        while self.queue.len() > keep {
            let d = self.queue.pop().expect("checked non-empty above");
            self.level_by_package[package_index(d.literal)] = 0;
        }
    }

    pub fn revert_last(&mut self) {
        let literal = self.last_literal();
        self.level_by_package[package_index(literal)] = 0;
        self.queue.pop();
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Iterates oldest-first (the PHP `Iterator` implementation walks the
    /// queue back-to-front via `end`/`prev`, but every caller of `foreach
    /// ($decisions as ...)` only reads the reason, order does not affect
    /// the packages collected).
    pub fn iter(&self) -> impl Iterator<Item = (i32, usize)> + '_ {
        self.queue.iter().map(|d| (d.literal, d.reason))
    }

    fn add_decision(&mut self, literal: i32, level: i32) {
        let package_id = package_index(literal);
        debug_assert_eq!(
            self.level_by_package[package_id], 0,
            "package {package_id} decided twice (SolverBugException in Composer)"
        );
        self.level_by_package[package_id] = if literal > 0 { level } else { -level };
    }
}
