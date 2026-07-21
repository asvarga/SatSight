//! The decoded solver view — the backward map's read model (plan §8).
//!
//! Where the [`Registry`](crate::registry::Registry) decodes *one* literal,
//! [`SolverView`] decodes an *entire* solver state into puzzle terms so a
//! [`Puzzle`](../../satsight_puzzles/puzzle/trait.Puzzle.html)'s `project()` can
//! turn it into a grid. It pairs a registry (to name variables) with the solver
//! artifacts to decode.
//!
//! Two backings, one read model:
//!
//! - [`from_model`](SolverView::from_model) — a *completed* [`Assignment`]. Every
//!   value is a settled fact of that one solution; there is no candidate lattice
//!   left to read and nothing tentative to distinguish.
//! - [`from_search`](SolverView::from_search) — a *live*, mid-search
//!   [`Search`](crate::cdcl::Search). Values, decision levels, and the trail are
//!   still evolving, so the view also exposes the artifacts plan §8 names: the
//!   proven/hypothetical split ([`certainty`](SolverView::certainty)), the BCP
//!   candidate lattice ([`is_candidate`](SolverView::is_candidate)), the trail
//!   ([`trail`](SolverView::trail)), the level-0 fixed facts
//!   ([`fixed`](SolverView::fixed)), and a failed-literal
//!   [`probe`](SolverView::probe).
//!
//! Every method decodes through the registry, so a `project()` reads *only* the
//! puzzle's own vocabulary and never touches the solver directly — the same
//! backward map whether the state is a finished model or a paused search. That is
//! why the corner/center mark split lives in each puzzle's `project()` (generic
//! over these artifacts) rather than in the frontend.

use std::hash::Hash;

use crate::cdcl::Search;
use crate::registry::Registry;
use crate::solver::Assignment;

/// Whether a mid-search fact is entailed by the givens alone or is contingent on
/// a branching guess — the distinction the grid draws differently (plan §1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Certainty {
    /// Forced by the givens (assumptions) via propagation: a **known** fact that
    /// holds in every solution consistent with the clues.
    Proven,
    /// Placed or ruled out only under the current branching guess: a
    /// **hypothetical** fact that may be undone on backtrack.
    Hypothetical,
}

/// What a [`SolverView`] decodes: a completed model or a live, mid-search state.
enum Backing<'a> {
    /// A completed model — every value is a settled fact of that one solution.
    Model(&'a Assignment),
    /// A live search: values, decision levels, and the trail, still evolving.
    Search(&'a Search),
}

/// A decoded window onto solver state, expressed in a puzzle's own vocabulary.
///
/// Borrows the registry and the artifacts it decodes, so it is cheap to build
/// and throw away each frame.
pub struct SolverView<'a, V: Eq + Hash + Clone> {
    registry: &'a Registry<V>,
    backing: Backing<'a>,
}

impl<'a, V: Eq + Hash + Clone> SolverView<'a, V> {
    /// A view over a completed model — the milestone-1 backward path.
    #[must_use]
    pub fn from_model(registry: &'a Registry<V>, assignment: &'a Assignment) -> Self {
        Self {
            registry,
            backing: Backing::Model(assignment),
        }
    }

    /// A view over a live, mid-search [`Search`](crate::cdcl::Search) — the
    /// stepping backward path (plan §8). Reads evolving state, so the richer
    /// artifacts ([`certainty`](Self::certainty), [`is_candidate`](Self::is_candidate),
    /// [`trail`](Self::trail), [`fixed`](Self::fixed), [`probe`](Self::probe))
    /// become meaningful.
    #[must_use]
    pub fn from_search(registry: &'a Registry<V>, search: &'a Search) -> Self {
        Self {
            registry,
            backing: Backing::Search(search),
        }
    }

    /// The registry backing this view.
    #[must_use]
    pub fn registry(&self) -> &Registry<V> {
        self.registry
    }

    /// The truth value assigned to proposition `v`:
    /// `Some(true)` if it holds, `Some(false)` if ruled out, `None` if the
    /// proposition is unregistered or the variable is unassigned.
    #[must_use]
    pub fn value(&self, v: &V) -> Option<bool> {
        let var = self.registry.get(v)?;
        match &self.backing {
            Backing::Model(model) => model.var_value(var),
            Backing::Search(search) => search.value_of(var),
        }
    }

    /// Whether `v` is still **Boolean-possible** — not (yet) ruled out. This is
    /// the candidate lattice the center marks read: `true` unless the proposition
    /// has been falsified. An unregistered proposition is not a candidate.
    #[must_use]
    pub fn is_candidate(&self, v: &V) -> bool {
        self.registry.get(v).is_some() && self.value(v) != Some(false)
    }

    /// The decision level at which `v` currently holds, or `None` if unassigned or
    /// unregistered. Always `Some(0)` for a decided proposition of a completed
    /// model (a model carries no branching structure).
    #[must_use]
    fn level(&self, v: &V) -> Option<u32> {
        let var = self.registry.get(v)?;
        match &self.backing {
            Backing::Model(model) => model.var_value(var).map(|_| 0),
            Backing::Search(search) => search.level_of(var),
        }
    }

    /// The boundary between proven and hypothetical facts (plan §1): a search's
    /// deepest given (assumption) level, or `0` for a completed model.
    #[must_use]
    pub fn base_level(&self) -> u32 {
        match &self.backing {
            Backing::Model(_) => 0,
            Backing::Search(search) => search.base_level(),
        }
    }

    /// Whether `v`'s current assignment is **proven** (entailed by the givens
    /// alone, at or below [`base_level`](Self::base_level)) or **hypothetical**
    /// (placed above it, under a branching guess). Meaningful only for an assigned
    /// proposition; an unassigned or unregistered one reports `Hypothetical`. A
    /// completed model reports every fact `Proven`.
    #[must_use]
    pub fn certainty(&self, v: &V) -> Certainty {
        if self
            .level(v)
            .is_some_and(|level| level <= self.base_level())
        {
            Certainty::Proven
        } else {
            Certainty::Hypothetical
        }
    }

    /// Whether `v` is ruled out **only under the current guess** — a hypothetical
    /// elimination (falsified above the givens' base level). Proven eliminations
    /// (forced by the givens) and completed models both report `false`: a known
    /// non-fact is simply left unmarked.
    #[must_use]
    pub fn eliminated_under_guess(&self, v: &V) -> bool {
        self.value(v) == Some(false) && self.level(v).is_some_and(|level| level > self.base_level())
    }

    /// The proposition trail in the order the search set it — the tentative
    /// puzzle state mid-search (plan §1). Auxiliary variables that name no
    /// proposition are dropped. Empty for a completed model (which has no trail).
    #[must_use]
    pub fn trail(&self) -> Vec<(V, bool)> {
        match &self.backing {
            Backing::Model(_) => Vec::new(),
            Backing::Search(search) => search
                .trail()
                .iter()
                .filter_map(|&lit| self.registry.decode(lit))
                .collect(),
        }
    }

    /// The propositions fixed at **decision level 0** — the facts the *rules
    /// alone* force before any given or guess (plan §8's level-0 fixed literals).
    /// Empty for a completed model.
    #[must_use]
    pub fn fixed(&self) -> Vec<(V, bool)> {
        match &self.backing {
            Backing::Model(_) => Vec::new(),
            Backing::Search(search) => search
                .trail()
                .iter()
                .filter(|&&lit| search.level_of(lit.var()) == Some(0))
                .filter_map(|&lit| self.registry.decode(lit))
                .collect(),
        }
    }

    /// Failed-literal probe: is asserting `v` (with polarity `holds`) refuted by
    /// what is currently known?
    ///
    /// For a completed model this is exact — the literal contradicts the (total)
    /// model. For a live search it is a statement about the **present tentative
    /// state**: `true` means BCP over the current trail (branching guesses
    /// included) immediately conflicts, so `v` is refuted *given the current
    /// guesses*, not necessarily in every solution. An unregistered proposition is
    /// never refuted.
    #[must_use]
    pub fn probe(&self, v: &V, holds: bool) -> bool {
        let Some(var) = self.registry.get(v) else {
            return false;
        };
        let lit = if holds { var.pos_lit() } else { var.neg_lit() };
        match &self.backing {
            Backing::Model(model) => model.lit_value(lit) == Some(false),
            Backing::Search(search) => search.probe(lit),
        }
    }
}
