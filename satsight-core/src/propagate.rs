//! Unit propagation and failed-literal probing — the backward map's engine.
//!
//! This is the part of SatSight where the solver "explains itself" without any
//! search. Two sound (but deliberately *incomplete*) inferences turn rules +
//! assumptions into facts phrased over the very same variables the
//! [`Registry`](crate::registry::Registry) can decode:
//!
//! - **Boolean constraint propagation (BCP)** — iterate unit propagation to a
//!   fixpoint. Everything it forces is *entailed* by the rules and assumptions,
//!   so it holds in every model. Decoded, these are plan §1's "proven facts":
//!   naked and hidden singles.
//! - **Failed-literal probing** — assume a literal, run BCP, and if it conflicts
//!   the literal is *provably false*. This reaches eliminations beyond naked
//!   singles (plan §8's center-mark feed) while staying sound.
//!
//! Both are sound because BCP only ever derives forced consequences and a BCP
//! conflict is a real conflict; both are incomplete because BCP alone does not
//! decide every formula. That is exactly the contract we want for "things the
//! solver can *prove*, in the puzzle's language." The hand-written CDCL
//! (milestone 2) will reuse this same propagation core with search on top.

use crate::cnf::{var_count, Cnf, Lit};
use crate::solver::Assignment;

/// A precompiled formula ready for repeated propagation and probing.
///
/// Holds the clauses plus, for each variable, the indices of the clauses that
/// mention it (an occurrence index), so a newly assigned literal only re-examines
/// the clauses it can actually affect.
pub struct Propagator {
    /// Clauses as literal lists.
    clauses: Vec<Vec<Lit>>,
    /// `occ[var_index]` = indices of clauses mentioning that variable.
    occ: Vec<Vec<usize>>,
    /// Number of variables the formula spans.
    n_vars: usize,
}

/// The result of running BCP to a fixpoint.
pub struct Propagation {
    /// Everything forced (assumptions included), as a partial assignment.
    pub assignment: Assignment,
    /// Whether propagation hit a conflict (the formula is UNSAT under the
    /// assumptions — a *sound* verdict, though BCP will not catch every UNSAT).
    pub conflict: bool,
}

/// What examining one clause under the current assignment reveals.
enum ClauseStatus {
    /// Satisfied, or still has ≥2 unassigned literals — nothing to do.
    Idle,
    /// All literals are false — a conflict.
    Conflict,
    /// Exactly one literal is unassigned and the rest are false — it is forced.
    Unit(Lit),
}

impl Propagator {
    /// Compile a propagator from a CNF.
    #[must_use]
    pub fn from_cnf(cnf: &Cnf) -> Self {
        let n_vars = var_count(cnf);
        let clauses: Vec<Vec<Lit>> = cnf
            .iter()
            .map(|clause| clause.iter().copied().collect())
            .collect();
        let mut occ = vec![Vec::new(); n_vars];
        for (ci, clause) in clauses.iter().enumerate() {
            for lit in clause {
                occ[lit.var().idx()].push(ci);
            }
        }
        Self {
            clauses,
            occ,
            n_vars,
        }
    }

    /// The number of variables the formula spans.
    #[must_use]
    pub fn n_vars(&self) -> usize {
        self.n_vars
    }

    /// Run BCP from `assumptions` to a fixpoint.
    ///
    /// The returned [`Propagation`] carries every literal forced (assumptions
    /// included). On conflict, `conflict` is `true` and the partial assignment is
    /// whatever had been derived when the conflict was found.
    #[must_use]
    pub fn propagate(&self, assumptions: &[Lit]) -> Propagation {
        let mut values: Vec<Option<bool>> = vec![None; self.n_vars];
        // Seed the worklist with every clause so unit clauses already present in
        // the CNF (and those made unit by assumptions) are caught.
        let mut worklist: Vec<usize> = (0..self.clauses.len()).collect();
        let mut conflict = false;

        // A local assign that reports whether it introduced a contradiction and
        // queues the clauses that now need re-examining.
        macro_rules! assign {
            ($lit:expr) => {{
                let lit: Lit = $lit;
                let idx = lit.var().idx();
                match values[idx] {
                    Some(v) if v != lit.is_pos() => {
                        conflict = true;
                    }
                    Some(_) => {}
                    None => {
                        values[idx] = Some(lit.is_pos());
                        worklist.extend(self.occ[idx].iter().copied());
                    }
                }
            }};
        }

        for &a in assumptions {
            assign!(a);
            if conflict {
                return Propagation {
                    assignment: Assignment::from_values(values),
                    conflict,
                };
            }
        }

        while let Some(ci) = worklist.pop() {
            match Self::examine(&self.clauses[ci], &values) {
                ClauseStatus::Idle => {}
                ClauseStatus::Conflict => {
                    conflict = true;
                    break;
                }
                ClauseStatus::Unit(lit) => {
                    assign!(lit);
                    if conflict {
                        break;
                    }
                }
            }
        }

        Propagation {
            assignment: Assignment::from_values(values),
            conflict,
        }
    }

    /// The literals BCP forces *beyond* the assumptions themselves.
    ///
    /// Empty when propagation conflicts (the whole thing is UNSAT, so "implied
    /// facts" is not meaningful). Each returned literal is entailed by the rules
    /// and assumptions, hence true in every model.
    #[must_use]
    pub fn implied(&self, assumptions: &[Lit]) -> Vec<Lit> {
        let result = self.propagate(assumptions);
        if result.conflict {
            return Vec::new();
        }
        let mut assumed = vec![false; self.n_vars];
        for a in assumptions {
            if let Some(slot) = assumed.get_mut(a.var().idx()) {
                *slot = true;
            }
        }
        (0..self.n_vars)
            .filter(|&i| !assumed[i])
            .filter_map(|i| {
                let var = rustsat_var(i);
                result.assignment.var_value(var).map(|value| {
                    if value {
                        var.pos_lit()
                    } else {
                        var.neg_lit()
                    }
                })
            })
            .collect()
    }

    /// Failed-literal probe: is `lit` provably false given `assumptions`?
    ///
    /// Assumes `lit` on top of `assumptions` and runs BCP; a conflict means
    /// `lit` cannot hold, so `¬lit` is entailed. Sound, incomplete.
    #[must_use]
    pub fn probe(&self, assumptions: &[Lit], lit: Lit) -> bool {
        let mut assumps = Vec::with_capacity(assumptions.len() + 1);
        assumps.extend_from_slice(assumptions);
        assumps.push(lit);
        self.propagate(&assumps).conflict
    }

    /// Classify a clause under the current partial assignment.
    fn examine(clause: &[Lit], values: &[Option<bool>]) -> ClauseStatus {
        let mut unit: Option<Lit> = None;
        let mut unassigned = 0u32;
        for &lit in clause {
            match values[lit.var().idx()] {
                Some(v) if v == lit.is_pos() => return ClauseStatus::Idle, // satisfied
                Some(_) => {}                                              // falsified literal
                None => {
                    unassigned += 1;
                    if unassigned > 1 {
                        return ClauseStatus::Idle;
                    }
                    unit = Some(lit);
                }
            }
        }
        match unit {
            Some(lit) if unassigned == 1 => ClauseStatus::Unit(lit),
            _ => ClauseStatus::Conflict,
        }
    }
}

/// Build the rustsat [`Var`](crate::cnf::Var) for a 0-based index.
fn rustsat_var(idx: usize) -> crate::cnf::Var {
    crate::cnf::Var::new(u32::try_from(idx).expect("variable index fits in u32"))
}

#[cfg(test)]
mod tests {
    use super::Propagator;
    use crate::backend_batsat::BatSatBackend;
    use crate::cnf::{clause, Cnf, Var};
    use crate::solver::SolveOutcome;

    /// A tiny formula: (a) ∧ (¬a ∨ b) ∧ (¬b ∨ c). Unit-propagates a,b,c all true.
    fn chain() -> (Cnf, [Var; 3]) {
        let (a, b, c) = (Var::new(0), Var::new(1), Var::new(2));
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.pos_lit()]));
        cnf.add_clause(clause([a.neg_lit(), b.pos_lit()]));
        cnf.add_clause(clause([b.neg_lit(), c.pos_lit()]));
        (cnf, [a, b, c])
    }

    #[test]
    fn bcp_forces_a_unit_chain() {
        let (cnf, [a, b, c]) = chain();
        let prop = Propagator::from_cnf(&cnf);
        let result = prop.propagate(&[]);
        assert!(!result.conflict);
        assert_eq!(result.assignment.var_value(a), Some(true));
        assert_eq!(result.assignment.var_value(b), Some(true));
        assert_eq!(result.assignment.var_value(c), Some(true));
    }

    #[test]
    fn bcp_detects_direct_conflict() {
        // (a) ∧ (¬a): unit propagation must find the contradiction.
        let a = Var::new(0);
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.pos_lit()]));
        cnf.add_clause(clause([a.neg_lit()]));
        let prop = Propagator::from_cnf(&cnf);
        assert!(prop.propagate(&[]).conflict);
    }

    #[test]
    fn probe_proves_a_false_literal() {
        // (¬a ∨ ¬b) with assumption a: probing b must conflict (b provably false).
        let (a, b) = (Var::new(0), Var::new(1));
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.neg_lit(), b.neg_lit()]));
        let prop = Propagator::from_cnf(&cnf);
        assert!(prop.probe(&[a.pos_lit()], b.pos_lit()));
        // …and a itself does not conflict.
        assert!(!prop.probe(&[], a.pos_lit()));
    }

    /// Everything BCP implies and everything probing disproves must agree with a
    /// full solve — the soundness anchor, checked against BatSat.
    #[test]
    fn implied_and_probed_agree_with_batsat() {
        let (cnf, _) = chain();
        let prop = Propagator::from_cnf(&cnf);

        // Solve for the (here unique) model with BatSat.
        let SolveOutcome::Sat(model) = BatSatBackend::solve_cnf(&cnf, &[]) else {
            panic!("chain formula is satisfiable");
        };

        // Every implied literal must hold in the model.
        for lit in prop.implied(&[]) {
            assert_eq!(
                model.lit_value(lit),
                Some(true),
                "implied literal must hold"
            );
        }

        // Every literal a probe disproves must indeed be false in the model.
        for i in 0..prop.n_vars() {
            let var = Var::new(u32::try_from(i).unwrap());
            if prop.probe(&[], var.pos_lit()) {
                assert_eq!(model.var_value(var), Some(false));
            }
            if prop.probe(&[], var.neg_lit()) {
                assert_eq!(model.var_value(var), Some(true));
            }
        }
    }
}
