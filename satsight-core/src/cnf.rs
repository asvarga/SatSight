//! CNF vocabulary.
//!
//! SatSight adopts rustsat's `Var` / `Lit` / `Clause` / `Cnf` types wholesale
//! (plan §11) so the [`Registry`](crate::registry::Registry), the encodings, and
//! both solver backends all speak one language. This module re-exports those
//! types and adds the two small conveniences the rest of the core needs:
//!
//! - [`clause`], a construction helper that keeps clause-building in one place;
//! - [`VarManager`], a source of *fresh* SAT variables that carry **no** puzzle
//!   meaning. Puzzle propositions are minted by the registry (the forward map);
//!   auxiliary variables introduced by an encoding (e.g. the sequential
//!   at-most-one) are not, so they must come from somewhere the registry does
//!   not own — and they must be numbered *after* every puzzle variable so that
//!   [`Registry::decode`](crate::registry::Registry::decode) cleanly returns
//!   `None` for them.

pub use rustsat::instances::Cnf;
pub use rustsat::types::{Clause, Lit, Var};

/// Build a [`Clause`] from anything yielding [`Lit`]s.
///
/// Centralizing clause construction keeps the (small) rustsat API surface we
/// depend on in exactly one place.
#[must_use]
pub fn clause<I: IntoIterator<Item = Lit>>(lits: I) -> Clause {
    let mut cl = Clause::new();
    for lit in lits {
        cl.add(lit);
    }
    cl
}

/// Hands out fresh SAT variables that have no puzzle-level meaning.
///
/// Encodings that need auxiliary variables (the sequential at-most-one, later
/// puzzles' cardinality encodings) draw them from here. Start it *after* all
/// puzzle variables have been registered (see [`VarManager::starting_at`]) so
/// auxiliaries occupy the variable-index range above the registry's, keeping the
/// registry's reverse map dense and gap-free.
#[derive(Debug, Clone)]
pub struct VarManager {
    next: u32,
}

impl VarManager {
    /// A manager that begins numbering at variable 0.
    #[must_use]
    pub fn new() -> Self {
        Self { next: 0 }
    }

    /// A manager whose first fresh variable is index `used`.
    ///
    /// Pass `registry.len()` so auxiliary variables sit above every puzzle
    /// variable.
    #[must_use]
    pub fn starting_at(used: u32) -> Self {
        Self { next: used }
    }

    /// Allocate and return the next unused variable.
    pub fn fresh(&mut self) -> Var {
        let var = Var::new(self.next);
        self.next += 1;
        var
    }

    /// How many variables have been handed out (i.e. the next index).
    #[must_use]
    pub fn n_used(&self) -> u32 {
        self.next
    }
}

impl Default for VarManager {
    fn default() -> Self {
        Self::new()
    }
}
