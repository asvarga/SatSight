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

/// At-most-`k` via Sinz's sequential counter — the general-`k` ladder.
///
/// This is the cardinality constraint the plan's second puzzle (Akari) needs
/// beyond exactly-one (plan §7): a numbered wall wants *exactly* `k` adjacent
/// lamps. It generalizes [`at_most_one_sequential`] (the `k = 1` case) to a
/// running counter, introducing `(n − 1)·k` auxiliary registers `sᵢⱼ` ("at least
/// `j` of the first `i` literals are true") drawn from `aux`.
///
/// Degenerate bounds short-circuit: `k ≥ n` is vacuous (nothing emitted), and
/// `k = 0` forbids every literal with a unit clause.
pub fn at_most_k_sequential(lits: &[Lit], k: usize, aux: &mut VarManager, cnf: &mut Cnf) {
    let n = lits.len();
    if k >= n {
        return; // at most n-of-n is always satisfiable
    }
    if k == 0 {
        for &l in lits {
            cnf.add_clause(clause([!l]));
        }
        return;
    }

    // Registers sᵢⱼ (1 ≤ i ≤ n−1, 1 ≤ j ≤ k): "at least j of x₁…xᵢ are true".
    // Kept 1-indexed to mirror the literature; row 0 and column 0 stay unused.
    let mut s = vec![vec![None; k + 1]; n];
    for row in s.iter_mut().take(n).skip(1) {
        for slot in row.iter_mut().take(k + 1).skip(1) {
            *slot = Some(aux.fresh().pos_lit());
        }
    }
    let x = |i: usize| lits[i - 1];
    let reg = |i: usize, j: usize| s[i][j].expect("register (i, j) is in range");

    // x₁ → s₁,₁, and s₁,ⱼ is false for j > 1 (only one literal seen so far).
    cnf.add_clause(clause([!x(1), reg(1, 1)]));
    for j in 2..=k {
        cnf.add_clause(clause([!reg(1, j)]));
    }
    // The carry recurrence for each further literal xᵢ (up to the last register row).
    for i in 2..n {
        cnf.add_clause(clause([!x(i), reg(i, 1)])); // xᵢ → sᵢ,₁
        cnf.add_clause(clause([!reg(i - 1, 1), reg(i, 1)])); // sᵢ₋₁,₁ → sᵢ,₁
        for j in 2..=k {
            // xᵢ ∧ sᵢ₋₁,ⱼ₋₁ → sᵢ,ⱼ  (this literal bumps the count)
            cnf.add_clause(clause([!x(i), !reg(i - 1, j - 1), reg(i, j)]));
            cnf.add_clause(clause([!reg(i - 1, j), reg(i, j)])); // sᵢ₋₁,ⱼ → sᵢ,ⱼ
        }
    }
    // The bound: no literal may push the running count past k, i.e. xᵢ forbids
    // "already k among the earlier ones".
    for i in 2..=n {
        cnf.add_clause(clause([!x(i), !reg(i - 1, k)]));
    }
}

/// At-least-`k`: at least `k` of `lits` are true.
///
/// The dual of [`at_most_k_sequential`]: "at least `k` true" is "at most `n − k`
/// false", so it counts the *negated* literals with the complementary bound.
/// `k = 0` is vacuous; `k > n` is impossible and emits the empty clause (UNSAT).
pub fn at_least_k_sequential(lits: &[Lit], k: usize, aux: &mut VarManager, cnf: &mut Cnf) {
    let n = lits.len();
    if k == 0 {
        return; // at least zero is always satisfiable
    }
    if k > n {
        cnf.add_clause(clause([])); // unsatisfiable: can't have more true than exist
        return;
    }
    let negated: Vec<Lit> = lits.iter().map(|&l| !l).collect();
    at_most_k_sequential(&negated, n - k, aux, cnf);
}

/// Exactly-`k` via at-least-`k` and at-most-`k` — the numbered-wall constraint.
pub fn exactly_k_sequential(lits: &[Lit], k: usize, aux: &mut VarManager, cnf: &mut Cnf) {
    at_least_k_sequential(lits, k, aux, cnf);
    at_most_k_sequential(lits, k, aux, cnf);
}

#[cfg(test)]
mod tests {
    use super::{
        at_least_k_sequential, at_most_k_sequential, exactly_k_sequential, exactly_one_pairwise,
        exactly_one_sequential,
    };
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

    /// Every subset of `0..n` whose size satisfies `keep`, each as a sorted vec of
    /// indices, sorted overall — the brute-force oracle for a cardinality bound.
    fn subsets(n: usize, keep: impl Fn(usize) -> bool) -> Vec<Vec<usize>> {
        let mut out = Vec::new();
        for mask in 0u32..(1 << n) {
            let set: Vec<usize> = (0..n).filter(|&i| mask & (1 << i) != 0).collect();
            if keep(set.len()) {
                out.push(set);
            }
        }
        out.sort();
        out
    }

    /// Encode `build` over `n` fresh puzzle variables (auxiliaries above them) and
    /// return every model restricted to those variables — for comparison against
    /// the brute-force subset oracle.
    fn cardinality_models(
        n: usize,
        build: impl Fn(&[super::Lit], &mut VarManager, &mut Cnf),
    ) -> Vec<Vec<usize>> {
        let mut reg: Registry<u32> = Registry::new();
        let lits: Vec<_> = (0..u32::try_from(n).unwrap())
            .map(|i| reg.var(i).pos_lit())
            .collect();
        let mut cnf = Cnf::new();
        let mut aux = VarManager::starting_at(u32::try_from(reg.len()).unwrap());
        build(&lits, &mut aux, &mut cnf);
        models(&cnf, n)
    }

    #[test]
    fn at_most_k_admits_exactly_the_small_subsets() {
        // Over every (n, k) in a small grid, the sequential at-most-k must accept
        // exactly the subsets of size ≤ k — no more (unsound) and no fewer
        // (incomplete). This pins the counter's indexing against brute force.
        for n in 0..=5 {
            for k in 0..=n + 1 {
                let got = cardinality_models(n, |lits, aux, cnf| {
                    at_most_k_sequential(lits, k, aux, cnf);
                });
                assert_eq!(got, subsets(n, |size| size <= k), "at-most-{k} over {n}");
            }
        }
    }

    #[test]
    fn at_least_k_admits_exactly_the_large_subsets() {
        for n in 0..=5 {
            for k in 0..=n + 1 {
                let got = cardinality_models(n, |lits, aux, cnf| {
                    at_least_k_sequential(lits, k, aux, cnf);
                });
                assert_eq!(got, subsets(n, |size| size >= k), "at-least-{k} over {n}");
            }
        }
    }

    #[test]
    fn exactly_k_admits_exactly_the_k_subsets() {
        for n in 0..=5 {
            for k in 0..=n + 1 {
                let got = cardinality_models(n, |lits, aux, cnf| {
                    exactly_k_sequential(lits, k, aux, cnf);
                });
                assert_eq!(got, subsets(n, |size| size == k), "exactly-{k} over {n}");
            }
        }
    }

    #[test]
    fn at_most_one_is_the_k_equals_one_case() {
        // The general counter at k = 1 must reproduce the dedicated at-most-one.
        let got = cardinality_models(4, |lits, aux, cnf| at_most_k_sequential(lits, 1, aux, cnf));
        assert_eq!(got, subsets(4, |size| size <= 1));
    }
}
