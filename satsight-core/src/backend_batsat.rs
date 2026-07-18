//! The fast backend: rustsat + BatSat (plan §5).
//!
//! BatSat is a pure-Rust CDCL solver — the recommended choice for WebAssembly —
//! which makes it the natural "run to solution" engine and the oracle the
//! hand-written CDCL (milestone 2) is checked against. It solves to completion;
//! it is *not* the steppable backend.
//!
//! This wrapper adapts BatSat to the core's [`Solver`] trait: it copies the rule
//! CNF in once, then answers [`solve`](Solver::solve) queries under assumption
//! literals, translating BatSat's model / core back into the core's
//! backend-neutral [`Assignment`] / literal-vector forms.

use rustsat::solvers::{Solve, SolveIncremental, SolverResult};
use rustsat::types::TernaryVal;
use rustsat_batsat::BasicSolver;

use crate::cnf::{var_count, Cnf, Lit, Var};
use crate::solver::{Assignment, SolveOutcome, Solver};

/// A [`Solver`] backed by BatSat via rustsat.
pub struct BatSatBackend {
    inner: BasicSolver,
    /// Number of distinct variables seen in the rule CNF; bounds the range we
    /// read back when materializing a model.
    n_vars: usize,
}

impl BatSatBackend {
    /// A fresh backend with no rules loaded.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: BasicSolver::default(),
            n_vars: 0,
        }
    }

    /// Convenience: load `cnf` and solve once under `assumptions`.
    #[must_use]
    pub fn solve_cnf(cnf: &Cnf, assumptions: &[Lit]) -> SolveOutcome {
        let mut backend = Self::new();
        backend.load_rules(cnf);
        backend.solve(assumptions)
    }
}

impl Default for BatSatBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Solver for BatSatBackend {
    fn load_rules(&mut self, cnf: &Cnf) {
        self.n_vars = var_count(cnf);
        self.inner
            .add_cnf(cnf.clone())
            .expect("BatSat accepts the rule CNF");
    }

    fn solve(&mut self, assumptions: &[Lit]) -> SolveOutcome {
        let result = self
            .inner
            .solve_assumps(assumptions)
            .expect("BatSat solves without error");
        match result {
            SolverResult::Sat => {
                let solution = self
                    .inner
                    .full_solution()
                    .expect("a SAT result yields a full model");
                let mut model = Assignment::with_capacity(self.n_vars);
                for i in 0..self.n_vars {
                    let var = Var::new(u32::try_from(i).expect("variable index fits in u32"));
                    match solution.var_value(var) {
                        TernaryVal::True => model.set(var, true),
                        TernaryVal::False => model.set(var, false),
                        // Don't-care: leave unassigned; the puzzle decides.
                        TernaryVal::DontCare => {}
                    }
                }
                SolveOutcome::Sat(model)
            }
            SolverResult::Unsat => {
                let core = self.inner.core().expect("an UNSAT result yields a core");
                SolveOutcome::Unsat(core)
            }
            SolverResult::Interrupted => {
                panic!("BatSat was interrupted, but we set no limits")
            }
        }
    }
}
