//! The [`Puzzle`] trait and the [`Grid`] its backward map produces.
//!
//! A puzzle is defined entirely by how it crosses the bridge (plan §3):
//!
//! - **Forward**: [`Puzzle::encode_rules`] emits the fixed rules as CNF, minting
//!   propositions through the [`Registry`]; [`Puzzle::assumptions`] turns the
//!   givens/edits into assumption literals (not clauses — plan §4).
//! - **Backward**: [`Puzzle::project`] reads a decoded [`SolverView`] and paints
//!   it onto a [`Grid`] of per-cell display state.
//!
//! Nothing here is Sudoku-specific; that is the point. A second puzzle drops in
//! by implementing this trait, with no changes to `satsight-core`.

use std::hash::Hash;

use satsight_core::backend_batsat::BatSatBackend;
use satsight_core::cnf::{Cnf, Lit, Var};
use satsight_core::propagate::Propagator;
use satsight_core::registry::Registry;
use satsight_core::solver::{SolveOutcome, Solver};
use satsight_core::view::SolverView;

/// A puzzle expressible as a SAT reduction with a decodable backward map.
pub trait Puzzle {
    /// A puzzle-level proposition — the vocabulary of the reduction. For Sudoku
    /// this is `Cell { r, c, v }` meaning "cell (r,c) holds value v".
    type Var: Eq + Hash + Clone;

    /// Per-cell display state produced by [`project`](Puzzle::project).
    type Cell;

    /// Forward reduction: emit the puzzle's *rules* as fixed CNF, registering
    /// each proposition through `reg`. Called once; the result never changes as
    /// the user edits (plan §4).
    fn encode_rules(&self, reg: &mut Registry<Self::Var>, cnf: &mut Cnf);

    /// The givens/edits as assumption literals. Because these are assumptions,
    /// not clauses, editing only rebuilds this vector — the CNF is untouched —
    /// and an UNSAT result comes back as a core over exactly these literals.
    fn assumptions(&self, reg: &Registry<Self::Var>) -> Vec<Lit>;

    /// Backward reduction: decode a solver view into a grid of display cells.
    fn project(&self, view: &SolverView<Self::Var>) -> Grid<Self::Cell>;
}

/// A dense row-major grid of per-cell display state.
///
/// The concrete output of every [`Puzzle::project`]; the frontend renders it.
#[derive(Debug, Clone)]
pub struct Grid<C> {
    rows: usize,
    cols: usize,
    cells: Vec<C>,
}

impl<C> Grid<C> {
    /// Build a `rows × cols` grid by calling `f(r, c)` for each cell.
    pub fn from_fn(rows: usize, cols: usize, mut f: impl FnMut(usize, usize) -> C) -> Self {
        let mut cells = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            for c in 0..cols {
                cells.push(f(r, c));
            }
        }
        Self { rows, cols, cells }
    }

    /// The number of rows.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The number of columns.
    #[must_use]
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// The cell at `(r, c)`.
    #[must_use]
    pub fn get(&self, r: usize, c: usize) -> &C {
        &self.cells[r * self.cols + c]
    }

    /// All cells in row-major order.
    #[must_use]
    pub fn cells(&self) -> &[C] {
        &self.cells
    }
}

/// Solve `puzzle` to a completed grid with the fast BatSat backend, or `None`
/// if the givens are contradictory.
///
/// This is the whole forward-and-back round trip in one call: encode rules,
/// gather assumptions, solve, then project the model back through the registry.
/// It works for *any* [`Puzzle`] — proof the pipeline is not Sudoku-shaped.
#[must_use]
pub fn solve<P: Puzzle>(puzzle: &P) -> Option<Grid<P::Cell>> {
    let mut reg = Registry::new();
    let mut cnf = Cnf::new();
    puzzle.encode_rules(&mut reg, &mut cnf);
    let assumptions = puzzle.assumptions(&reg);

    let mut backend = BatSatBackend::new();
    backend.load_rules(&cnf);
    match backend.solve(&assumptions) {
        SolveOutcome::Sat(model) => {
            let view = SolverView::from_model(&reg, &model);
            Some(puzzle.project(&view))
        }
        SolveOutcome::Unsat(_) => None,
    }
}

/// What a puzzle can be *proven* to imply by pure logic, in its own vocabulary.
///
/// The output of [`deduce`] — the backward map applied to sound, search-free
/// inference rather than to a full model.
#[derive(Debug, Clone)]
pub struct Deductions<V> {
    /// Whether the givens are consistent as far as propagation can tell. `false`
    /// is a *sound* contradiction verdict (BCP found a conflict); `true` does not
    /// promise global satisfiability, only that BCP saw no conflict.
    pub satisfiable: bool,
    /// Facts proven *beyond* the givens: `(proposition, holds)`. `holds == true`
    /// is a forced placement; `holds == false` is a proven elimination.
    pub proven: Vec<(V, bool)>,
}

/// Deduce everything pure logic can prove about `puzzle` — no search.
///
/// Runs unit propagation from the givens, then one round of failed-literal
/// probing over the still-undetermined propositions (feeding what it proves back
/// into propagation), and decodes the result through the registry. This is the
/// bidirectional thesis in miniature: the solver's deductions, expressed in the
/// puzzle's own language. It is fully generic over [`Puzzle`] — nothing here is
/// Sudoku-shaped.
#[must_use]
pub fn deduce<P: Puzzle>(puzzle: &P) -> Deductions<P::Var> {
    let mut reg = Registry::new();
    let mut cnf = Cnf::new();
    puzzle.encode_rules(&mut reg, &mut cnf);
    let givens = puzzle.assumptions(&reg);

    let prop = Propagator::from_cnf(&cnf);
    let base = prop.propagate(&givens);
    if base.conflict {
        return Deductions {
            satisfiable: false,
            proven: Vec::new(),
        };
    }

    // Probe each still-undetermined proposition; a probe that conflicts proves
    // that literal's negation, which we add as a (sound) assumption.
    let var_at = |i: usize| Var::new(u32::try_from(i).expect("variable index fits in u32"));
    let mut assumptions = givens.clone();
    for i in 0..reg.len() {
        let var = var_at(i);
        if base.assignment.var_value(var).is_some() {
            continue; // already decided by propagation
        }
        if prop.probe(&givens, var.pos_lit()) {
            assumptions.push(var.neg_lit());
        } else if prop.probe(&givens, var.neg_lit()) {
            assumptions.push(var.pos_lit());
        }
    }
    let closed = prop.propagate(&assumptions);

    // Everything decided that is not itself a given, decoded to puzzle terms.
    let mut is_given = vec![false; reg.len()];
    for g in &givens {
        if let Some(slot) = is_given.get_mut(g.var().idx()) {
            *slot = true;
        }
    }
    let mut proven = Vec::new();
    for (i, &given) in is_given.iter().enumerate() {
        if given {
            continue;
        }
        let var = var_at(i);
        if let Some(value) = closed.assignment.var_value(var) {
            if let Some(prop_var) = reg.decode_var(var) {
                proven.push((prop_var.clone(), value));
            }
        }
    }
    Deductions {
        satisfiable: true,
        proven,
    }
}
