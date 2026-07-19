//! Backbone extraction — the facts that hold in *every* model (plan §1).
//!
//! Where [`Propagator`](crate::propagate::Propagator) proves things *without*
//! search (sound but incomplete), the backbone is the complete set of literals
//! entailed by the formula under the assumptions: a literal is in the backbone
//! iff it is true in every satisfying assignment. Decoded through the
//! [`Registry`](crate::registry::Registry) it is plan §1's "facts holding across
//! all solutions" — most interesting on *under-constrained* puzzles, where it
//! reveals which cells are forced regardless of which solution you pick (and, for
//! a uniquely solvable board, it is simply the whole solution).
//!
//! This lives in the core, generic over the [`Solver`] trait, so it works for any
//! backend and any puzzle — the backward map's abstractions stay puzzle-agnostic.

use crate::cnf::{Lit, Var};
use crate::solver::{SolveOutcome, Solver};

/// The backbone under `assumptions`: every literal true in *every* model that
/// satisfies the assumptions. `None` when the formula is UNSAT (no models, so
/// "true in every model" is vacuous and not what a caller wants); `Some(vec)`
/// otherwise, possibly empty when nothing is forced.
///
/// **Algorithm (model rotation).** Solve once for a seed model and take every
/// assigned literal as a backbone *candidate*. Then, for each candidate still
/// standing, ask the solver for a model of its *negation*:
///
/// - **UNSAT** — no model flips it, so the literal is entailed: it joins the
///   backbone.
/// - **SAT** — the refuting model is a witness that the literal is *not* forced;
///   better still, that one model knocks out **every** remaining candidate it
///   disagrees with (including any it leaves unassigned — an unassigned variable
///   can be flipped freely, so it is not forced either).
///
/// This runs `O(backbone size + refutations)` solves. For a uniquely solvable
/// instance the backbone is the entire model, so expect one solve per variable;
/// puzzle instances are tiny, so that stays cheap, and callers with a cheaper
/// uniqueness test (e.g. block-and-resolve) can short-circuit the common case.
#[must_use]
pub fn backbone<S: Solver>(solver: &mut S, assumptions: &[Lit]) -> Option<Vec<Lit>> {
    let seed = match solver.solve(assumptions) {
        SolveOutcome::Sat(model) => model,
        SolveOutcome::Unsat(_) => return None,
    };

    // Candidates: the seed's assigned literals. Unassigned (don't-care) variables
    // are already free, so they are never forced and never candidates.
    let mut candidates: Vec<Lit> = (0..seed.len())
        .filter_map(|i| {
            let var = Var::new(u32::try_from(i).expect("variable index fits in u32"));
            seed.var_value(var)
                .map(|value| if value { var.pos_lit() } else { var.neg_lit() })
        })
        .collect();

    let mut backbone = Vec::new();
    let mut assumps = Vec::with_capacity(assumptions.len() + 1);
    while let Some(lit) = candidates.pop() {
        assumps.clear();
        assumps.extend_from_slice(assumptions);
        assumps.push(!lit);
        match solver.solve(&assumps) {
            // No model flips `lit`: it holds in every model — a backbone literal.
            SolveOutcome::Unsat(_) => backbone.push(lit),
            // A witness that `lit` is free; drop every remaining candidate this
            // model disagrees with (a differing *or* unassigned value both mean
            // "not forced"), collapsing many refutations into one solve.
            SolveOutcome::Sat(other) => {
                candidates.retain(|&c| other.lit_value(c) == Some(true));
            }
        }
    }
    Some(backbone)
}

#[cfg(test)]
mod tests {
    use super::backbone;
    use crate::backend_batsat::BatSatBackend;
    use crate::cnf::{clause, Cnf, Var};
    use crate::solver::Solver;

    /// A unique model forces its whole assignment: the backbone is every literal.
    #[test]
    fn unique_model_has_a_full_backbone() {
        // (a) ∧ (¬a ∨ b) ∧ (¬b ∨ c): the only model is a,b,c all true.
        let (a, b, c) = (Var::new(0), Var::new(1), Var::new(2));
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.pos_lit()]));
        cnf.add_clause(clause([a.neg_lit(), b.pos_lit()]));
        cnf.add_clause(clause([b.neg_lit(), c.pos_lit()]));

        let mut backend = BatSatBackend::new();
        backend.load_rules(&cnf);
        let mut bb = backbone(&mut backend, &[]).expect("satisfiable");
        bb.sort_by_key(|l| l.var().idx());
        assert_eq!(bb, vec![a.pos_lit(), b.pos_lit(), c.pos_lit()]);
    }

    /// A free choice is excluded; only the forced literal survives.
    #[test]
    fn free_variables_are_not_in_the_backbone() {
        // (a) ∧ exactly-one(b, c): a is forced true; b and c each flip between the
        // two models, so neither is backbone. Exercises the SAT-refutation prune.
        let (a, b, c) = (Var::new(0), Var::new(1), Var::new(2));
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.pos_lit()]));
        cnf.add_clause(clause([b.pos_lit(), c.pos_lit()])); // at least one
        cnf.add_clause(clause([b.neg_lit(), c.neg_lit()])); // at most one

        let mut backend = BatSatBackend::new();
        backend.load_rules(&cnf);
        let bb = backbone(&mut backend, &[]).expect("satisfiable");
        assert_eq!(bb, vec![a.pos_lit()], "only a is forced; b and c are free");
    }

    /// Assumptions narrow the backbone: fixing a free choice forces its partner.
    #[test]
    fn assumptions_extend_the_backbone() {
        let (a, b, c) = (Var::new(0), Var::new(1), Var::new(2));
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.pos_lit()]));
        cnf.add_clause(clause([b.pos_lit(), c.pos_lit()]));
        cnf.add_clause(clause([b.neg_lit(), c.neg_lit()]));

        let mut backend = BatSatBackend::new();
        backend.load_rules(&cnf);
        // Assume b: now c is forced false, so the backbone is {a, b, ¬c}.
        let mut bb = backbone(&mut backend, &[b.pos_lit()]).expect("satisfiable");
        bb.sort_by_key(|l| l.var().idx());
        assert_eq!(bb, vec![a.pos_lit(), b.pos_lit(), c.neg_lit()]);
    }

    /// UNSAT formulas have no models, so backbone reports `None`.
    #[test]
    fn unsatisfiable_formula_has_no_backbone() {
        let a = Var::new(0);
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.pos_lit()]));
        cnf.add_clause(clause([a.neg_lit()]));
        let mut backend = BatSatBackend::new();
        backend.load_rules(&cnf);
        assert!(backbone(&mut backend, &[]).is_none());
    }
}
