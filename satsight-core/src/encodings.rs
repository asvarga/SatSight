//! Boolean cardinality encodings used on the forward path.
//!
//! Sudoku is *nothing but* exactly-one constraints (plan §7), so this module
//! stays small: at-most-one in two flavours plus the exactly-one built on top.
//!
//! - **Pairwise** — `O(n²)` binary clauses, no auxiliary variables. The default;
//!   for Sudoku's `n = 9` that is a trivial 36 clauses per constraint.
//! - **Sequential** (Sinz's "ladder") — `O(n)` clauses and `O(n)` auxiliary
//!   variables. Included both because the milestones call for it and because it
//!   is what keeps larger cardinality constraints (a future puzzle) tractable.
//!
//! Every literal here is expected to come from a
//! [`Registry`](crate::registry::Registry); auxiliary variables for the
//! sequential encoding come from a [`VarManager`](crate::cnf::VarManager).

use crate::cnf::{clause, Cnf, Lit, VarManager};

/// At-least-one: a single clause asserting the disjunction of `lits`.
pub fn at_least_one(lits: &[Lit], cnf: &mut Cnf) {
    cnf.add_clause(clause(lits.iter().copied()));
}

/// At-most-one via the pairwise encoding: `¬lᵢ ∨ ¬lⱼ` for every pair `i < j`.
///
/// No auxiliary variables; `n·(n−1)/2` binary clauses.
pub fn at_most_one_pairwise(lits: &[Lit], cnf: &mut Cnf) {
    for (i, &a) in lits.iter().enumerate() {
        for &b in &lits[i + 1..] {
            cnf.add_clause(clause([!a, !b]));
        }
    }
}

/// Exactly-one via pairwise at-most-one plus at-least-one.
pub fn exactly_one_pairwise(lits: &[Lit], cnf: &mut Cnf) {
    at_least_one(lits, cnf);
    at_most_one_pairwise(lits, cnf);
}

/// At-most-one via Sinz's sequential ("ladder") encoding.
///
/// Introduces `n − 1` auxiliary variables `s₀ … s_{n−2}` from `aux`, where `sᵢ`
/// means "at least one of `lits[0..=i]` is true". Adds `3n − 4` clauses (for
/// `n ≥ 2`). For `n ≤ 1` the constraint is vacuous and nothing is emitted.
pub fn at_most_one_sequential(lits: &[Lit], aux: &mut VarManager, cnf: &mut Cnf) {
    let n = lits.len();
    if n <= 1 {
        return;
    }
    // s[i] ("register carry i") is true iff some literal at index <= i is true.
    let s: Vec<Lit> = (0..n - 1).map(|_| aux.fresh().pos_lit()).collect();
    // x0 -> s0
    cnf.add_clause(clause([!lits[0], s[0]]));
    // x_{n-1} -> ¬s_{n-2}   (if the last is set, no earlier one may be)
    cnf.add_clause(clause([!lits[n - 1], !s[n - 2]]));
    for i in 1..n - 1 {
        // x_i -> s_i
        cnf.add_clause(clause([!lits[i], s[i]]));
        // s_{i-1} -> s_i   (the carry propagates)
        cnf.add_clause(clause([!s[i - 1], s[i]]));
        // x_i -> ¬s_{i-1}  (if an earlier one was set, x_i may not be)
        cnf.add_clause(clause([!lits[i], !s[i - 1]]));
    }
}

/// Exactly-one via sequential at-most-one plus at-least-one.
pub fn exactly_one_sequential(lits: &[Lit], aux: &mut VarManager, cnf: &mut Cnf) {
    at_least_one(lits, cnf);
    at_most_one_sequential(lits, aux, cnf);
}

#[cfg(test)]
mod tests {
    use super::{exactly_one_pairwise, exactly_one_sequential};
    use crate::backend_batsat::BatSatBackend;
    use crate::cnf::{Cnf, VarManager};
    use crate::registry::Registry;
    use crate::solver::{SolveOutcome, Solver};

    /// Enumerate all satisfying assignments over the given puzzle variables and
    /// return, for each model, the set of variable indices assigned true.
    fn models(cnf: &Cnf, n_puzzle_vars: usize) -> Vec<Vec<usize>> {
        // Brute force over the puzzle variables by adding blocking assumptions is
        // overkill; instead just repeatedly solve, blocking each model found.
        let mut found = Vec::new();
        let mut work = cnf.clone();
        loop {
            let mut backend = BatSatBackend::new();
            backend.load_rules(&work);
            match backend.solve(&[]) {
                SolveOutcome::Sat(model) => {
                    let mut trues = Vec::new();
                    let mut block = Vec::new();
                    for i in 0..n_puzzle_vars {
                        let var = crate::cnf::Var::new(u32::try_from(i).unwrap());
                        if model.var_value(var) == Some(true) {
                            trues.push(i);
                            block.push(var.neg_lit());
                        } else {
                            block.push(var.pos_lit());
                        }
                    }
                    found.push(trues);
                    // Block this exact assignment over the puzzle vars.
                    work.add_clause(crate::cnf::clause(block));
                }
                SolveOutcome::Unsat(_) => break,
            }
        }
        found.sort();
        found
    }

    #[test]
    fn pairwise_exactly_one_has_n_models() {
        let mut reg: Registry<u32> = Registry::new();
        let lits: Vec<_> = (0..5).map(|i| reg.var(i).pos_lit()).collect();
        let mut cnf = Cnf::new();
        exactly_one_pairwise(&lits, &mut cnf);
        // Exactly-one over 5 vars => exactly the 5 singletons.
        assert_eq!(
            models(&cnf, 5),
            vec![vec![0], vec![1], vec![2], vec![3], vec![4]]
        );
    }

    #[test]
    fn sequential_exactly_one_matches_pairwise() {
        let mut reg: Registry<u32> = Registry::new();
        let lits: Vec<_> = (0..5).map(|i| reg.var(i).pos_lit()).collect();
        let mut cnf = Cnf::new();
        // Auxiliaries start above the 5 puzzle variables.
        let mut aux = VarManager::starting_at(u32::try_from(reg.len()).unwrap());
        exactly_one_sequential(&lits, &mut aux, &mut cnf);
        // Restricting the models to the 5 puzzle vars must give the same 5
        // singletons the pairwise encoding produces.
        assert_eq!(
            models(&cnf, 5),
            vec![vec![0], vec![1], vec![2], vec![3], vec![4]]
        );
    }
}
