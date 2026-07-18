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
use satsight_core::cnf::{Cnf, Lit};
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
