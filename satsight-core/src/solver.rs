//! The solver interface both backends share (plan §5).
//!
//! SatSight ships two solvers behind one [`Solver`] trait:
//!
//! - a fast "run to solution" backend ([`BatSatBackend`](crate::backend_batsat),
//!   milestone 1), also used as the oracle the hand-written solver is tested
//!   against; and
//! - a small, *instrumentable* CDCL (milestone 2) whose distinguishing feature
//!   is a non-blocking `step()` that yields an [`Event`] at a time.
//!
//! The common denominator captured here is: install fixed rule CNF once, then
//! solve to completion under a set of *assumption* literals (the givens/edits,
//! per plan §4), yielding either a model or an UNSAT core over those
//! assumptions. The stepping API lives with the CDCL backend, not on this trait,
//! so a solve-oriented backend need not pretend to support it.

use crate::cnf::{Cnf, Lit, Var};

/// A (possibly partial) truth assignment, indexed by SAT variable.
///
/// This is the backend-neutral form a solved model takes before it is decoded
/// through the [`Registry`](crate::registry::Registry) into puzzle state. `None`
/// means "unassigned / don't-care".
#[derive(Debug, Clone, Default)]
pub struct Assignment {
    values: Vec<Option<bool>>,
}

impl Assignment {
    /// An empty assignment.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty assignment pre-sized for `n` variables.
    #[must_use]
    pub fn with_capacity(n: usize) -> Self {
        Self {
            values: vec![None; n],
        }
    }

    /// An assignment from a per-variable value vector (index = variable index).
    #[must_use]
    pub fn from_values(values: Vec<Option<bool>>) -> Self {
        Self { values }
    }

    /// Record that `var` takes `value`, growing the backing store as needed.
    pub fn set(&mut self, var: Var, value: bool) {
        let i = var.idx();
        if i >= self.values.len() {
            self.values.resize(i + 1, None);
        }
        self.values[i] = Some(value);
    }

    /// The truth value assigned to `var`, or `None` if unassigned.
    #[must_use]
    pub fn var_value(&self, var: Var) -> Option<bool> {
        self.values.get(var.idx()).copied().flatten()
    }

    /// The truth value of `lit` under this assignment (its variable's value
    /// flipped when the literal is negative), or `None` if unassigned.
    #[must_use]
    pub fn lit_value(&self, lit: Lit) -> Option<bool> {
        let base = self.var_value(lit.var())?;
        Some(if lit.is_pos() { base } else { !base })
    }

    /// The number of variable slots (assigned or not).
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the assignment has no variable slots.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// The result of running a solver to completion.
#[derive(Debug, Clone)]
pub enum SolveOutcome {
    /// Satisfiable — carries a model over the problem's variables.
    Sat(Assignment),
    /// Unsatisfiable under the assumptions — carries an UNSAT core that is a
    /// subset of the assumption literals (the conflicting givens; plan §4).
    Unsat(Vec<Lit>),
}

/// A SAT solver over a fixed rule CNF plus per-solve assumption literals.
pub trait Solver {
    /// Install the fixed rules. Rules never change during editing (plan §4), so
    /// this is called once; givens/edits arrive later as assumptions.
    fn load_rules(&mut self, cnf: &Cnf);

    /// Solve to completion under `assumptions`, returning a model or a core.
    fn solve(&mut self, assumptions: &[Lit]) -> SolveOutcome;
}
