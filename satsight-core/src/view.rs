//! The decoded solver view — the backward map's read model (plan §8).
//!
//! Where the [`Registry`](crate::registry::Registry) decodes *one* literal,
//! [`SolverView`] decodes an *entire* solver state into puzzle terms so a
//! [`Puzzle`](../../satsight_puzzles/puzzle/trait.Puzzle.html)'s `project()` can
//! turn it into a grid. It pairs a registry (to name variables) with the solver
//! artifacts to decode.
//!
//! Milestone 1 exposes only a completed **model**. Later milestones grow this
//! into the full set of artifacts from plan §1's table — the current trail,
//! level-0 fixed literals, the BCP candidate lattice, the learned-clause list,
//! and a `probe(lit)` method — which is why it is a view *object* rather than a
//! bare function.

use std::hash::Hash;

use crate::registry::Registry;
use crate::solver::Assignment;

/// A decoded window onto solver state, expressed in a puzzle's own vocabulary.
///
/// Borrows the registry and the artifacts it decodes, so it is cheap to build
/// and throw away each frame.
pub struct SolverView<'a, V: Eq + Hash + Clone> {
    registry: &'a Registry<V>,
    assignment: &'a Assignment,
}

impl<'a, V: Eq + Hash + Clone> SolverView<'a, V> {
    /// A view over a completed model — the milestone-1 backward path.
    #[must_use]
    pub fn from_model(registry: &'a Registry<V>, assignment: &'a Assignment) -> Self {
        Self {
            registry,
            assignment,
        }
    }

    /// The registry backing this view.
    #[must_use]
    pub fn registry(&self) -> &Registry<V> {
        self.registry
    }

    /// The truth value the model assigns to proposition `v`:
    /// `Some(true)` if it holds, `Some(false)` if ruled out, `None` if the
    /// proposition is unregistered or the variable is unassigned.
    #[must_use]
    pub fn value(&self, v: &V) -> Option<bool> {
        let var = self.registry.get(v)?;
        self.assignment.var_value(var)
    }
}
