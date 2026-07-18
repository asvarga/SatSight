//! The registry — the load-bearing bridge of the whole design (plan §1, §3).
//!
//! "Bidirectional reduction" sounds like two translators, but it is really *one
//! object viewed two ways*: a bijection between puzzle-level propositions (`V`,
//! e.g. `Cell { r, c, v }` for Sudoku) and SAT variables. Every **forward** step
//! (encoding rules and givens into SAT) and every **backward** step (interpreting
//! what the solver found) routes through here.
//!
//! - Forward: [`Registry::var`] / [`Registry::lit`] mint the SAT variable/literal
//!   for a proposition (creating it on first sight).
//! - Backward: [`Registry::decode`] turns a solver literal back into
//!   `(proposition, truth value)`.

use std::collections::HashMap;
use std::hash::Hash;

use crate::cnf::{Lit, Var};

/// A bijection between puzzle propositions of type `V` and SAT variables.
///
/// Variables are assigned densely from index 0 in first-seen order, so the
/// reverse map is a simple `Vec`. Auxiliary variables introduced by encodings do
/// **not** live here (they have no puzzle meaning); allocate those from a
/// [`VarManager`](crate::cnf::VarManager) started at [`Registry::len`], and
/// [`decode`](Registry::decode) will correctly report them as `None`.
#[derive(Debug, Clone)]
pub struct Registry<V: Eq + Hash + Clone> {
    /// Forward map: proposition -> SAT variable (the encode side).
    fwd: HashMap<V, Var>,
    /// Reverse map: SAT variable index -> proposition (the decode side).
    rev: Vec<V>,
}

impl<V: Eq + Hash + Clone> Registry<V> {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            fwd: HashMap::new(),
            rev: Vec::new(),
        }
    }

    /// The SAT variable for proposition `v`, allocating a fresh one on first use.
    ///
    /// This is the forward map. Calling it again with an equal `v` returns the
    /// same variable — that is what makes the mapping a bijection.
    pub fn var(&mut self, v: V) -> Var {
        if let Some(&var) = self.fwd.get(&v) {
            return var;
        }
        let idx = u32::try_from(self.rev.len()).expect("SAT variable count fits in u32");
        let var = Var::new(idx);
        self.fwd.insert(v.clone(), var);
        self.rev.push(v);
        var
    }

    /// The literal for proposition `v` with the given polarity, allocating the
    /// variable if needed. `value == true` yields the positive literal.
    pub fn lit(&mut self, v: V, value: bool) -> Lit {
        let var = self.var(v);
        if value {
            var.pos_lit()
        } else {
            var.neg_lit()
        }
    }

    /// The SAT variable already mapped to `v`, or `None` if `v` was never
    /// registered. Unlike [`var`](Registry::var) this never allocates, so it is
    /// safe behind a shared borrow — exactly what
    /// [`Puzzle::assumptions`](../../satsight_puzzles/puzzle/trait.Puzzle.html)
    /// needs.
    #[must_use]
    pub fn get(&self, v: &V) -> Option<Var> {
        self.fwd.get(v).copied()
    }

    /// The proposition a SAT variable stands for, or `None` for an auxiliary
    /// (non-puzzle) variable.
    #[must_use]
    pub fn decode_var(&self, var: Var) -> Option<&V> {
        self.rev.get(var.idx())
    }

    /// Turn a solver literal back into `(proposition, truth value)` — the
    /// backward map. Returns `None` for literals over auxiliary variables.
    ///
    /// A positive literal decodes to `(v, true)` ("`v` holds"), a negative one to
    /// `(v, false)` ("`v` is ruled out").
    #[must_use]
    pub fn decode(&self, lit: Lit) -> Option<(V, bool)> {
        self.rev
            .get(lit.var().idx())
            .map(|v| (v.clone(), lit.is_pos()))
    }

    /// The number of registered puzzle variables.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rev.len()
    }

    /// Whether no propositions have been registered yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rev.is_empty()
    }
}

impl<V: Eq + Hash + Clone> Default for Registry<V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Registry;

    #[test]
    fn var_is_stable_and_dense() {
        let mut reg: Registry<&str> = Registry::new();
        let a = reg.var("a");
        let b = reg.var("b");
        // Re-registering returns the same variable (bijection).
        assert_eq!(reg.var("a"), a);
        assert_ne!(a, b);
        // Dense, first-seen indices.
        assert_eq!(a.idx(), 0);
        assert_eq!(b.idx(), 1);
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn decode_round_trips_polarity() {
        let mut reg: Registry<u32> = Registry::new();
        let v = reg.var(42);
        assert_eq!(reg.decode(v.pos_lit()), Some((42, true)));
        assert_eq!(reg.decode(v.neg_lit()), Some((42, false)));
    }

    #[test]
    fn decode_of_unknown_var_is_none() {
        let reg: Registry<u32> = Registry::new();
        // Variable index 7 was never registered (no propositions at all).
        let stray = crate::cnf::Var::new(7);
        assert_eq!(reg.decode_var(stray), None);
        assert_eq!(reg.decode(stray.pos_lit()), None);
    }
}
