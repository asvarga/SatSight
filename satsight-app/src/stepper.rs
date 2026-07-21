//! The stepping controller — the bridge between the observable CDCL and a view.
//!
//! This is the frontend half of plan §6: it owns a [`Search`] and drives it one
//! [`Event`] at a time, so the UI can advance the solver and read its evolving
//! state. Kept deliberately egui-free so the driving is unit-testable without a
//! window.
//!
//! It is **generic over the puzzle's proposition type** `V`, so one stepper drives
//! Sudoku (`V = Cell`) and graph coloring (`V = VertexColor`) alike — the Step view
//! is no longer Sudoku-shaped (issue #12). The generic half exposes the primitives
//! every view needs, in puzzle-neutral terms:
//!
//! - [`value`](Stepper::value) / [`assigned`](Stepper::assigned) — the truth value
//!   (and decision level) the search currently gives a proposition;
//! - [`certainty`](Stepper::certainty) — whether that assignment is entailed by the
//!   givens alone (**proven**) or contingent on a branching guess (**hypothetical**),
//!   the distinction the grid draws differently (plan §1);
//! - [`touched_props`](Stepper::touched_props) / [`core_props`](Stepper::core_props)
//!   — the propositions the last event, or an UNSAT core, names, for a view to map
//!   onto its own coordinates and highlight;
//! - [`last_event`](Stepper::last_event) + [`solver_phase`] — the raw event and the
//!   shared status text for the puzzle-agnostic moves (conflict/backtrack/learn);
//! - [`view`](Stepper::view) — a [`SolverView`] over the live search, so a
//!   [`Puzzle::project`](satsight_puzzles::Puzzle::project) decodes the mid-search
//!   state (candidate lattice, center/corner marks, proven/hypothetical split) in
//!   the puzzle's own vocabulary. The per-cell mark-splitting lives *there*, in the
//!   backward map, not here in the frontend (issue #2).
//!
//! What remains puzzle-shaped in this file is only event *narration* — turning the
//! last [`Event`] into a one-line status and the cells to emphasize — which is
//! inherently frontend and differs per puzzle's vocabulary.

use std::hash::Hash;

use satsight_core::{Cdcl, Event, Lit, Registry, Search, SolverView};
use satsight_puzzles::sudoku::Cell;

/// Whether a mid-search fact is entailed by the givens alone or is contingent on a
/// search assumption — the distinction the grid draws differently (plan §1). Now
/// defined by the core read model; re-exported so the views keep one name for it.
pub use satsight_core::Certainty;

/// Which cells the last event wants the Sudoku renderer to emphasize.
pub enum Emphasis {
    /// Nothing in particular.
    None,
    /// A decision/propagation target, or the cells of a conflict clause.
    Cells(Vec<(usize, usize)>),
}

/// The status line shown before the first step — shared across puzzle views.
pub const READY: &str = "Ready — press Step or Play to watch the solver.";

/// The status phrase for the puzzle-agnostic solver events (conflict, backtrack,
/// learn); `None` for the events a view names in its own vocabulary (decide,
/// propagate, sat, unsat). Shared so every puzzle renders the mechanics
/// identically and supplies only the vocabulary that differs.
#[must_use]
pub fn solver_phase(event: &Event) -> Option<String> {
    match event {
        Event::Conflict { .. } => Some("Conflict — analyzing and backtracking.".to_owned()),
        Event::Backtrack { to_level } => Some(format!("Backtrack to level {to_level}.")),
        Event::Learn { clause } => Some(format!("Learned a clause ({} literals).", clause.len())),
        _ => None,
    }
}

/// Drives a [`Search`] event-by-event and hands its state to a puzzle view.
///
/// Generic over the proposition type `V`: the search machinery is puzzle-agnostic
/// (it works on [`Lit`]s and decodes through the [`Registry<V>`]), so a single
/// stepper serves every [`Puzzle`](satsight_puzzles::Puzzle).
pub struct Stepper<V: Eq + Hash + Clone> {
    reg: Registry<V>,
    search: Search,
    last: Option<Event>,
    /// Whether the UI is auto-advancing (owned here so the app can toggle it).
    pub playing: bool,
}

impl<V: Eq + Hash + Clone> Stepper<V> {
    /// Begin a fresh stepped search over `assumptions` (the current givens).
    #[must_use]
    pub fn new(cdcl: &Cdcl, reg: Registry<V>, assumptions: &[Lit]) -> Self {
        let search = cdcl.search(assumptions);
        Self {
            reg,
            search,
            last: None,
            playing: false,
        }
    }

    /// Advance by one event, unless already terminal.
    pub fn step(&mut self) {
        if self.search.is_done() {
            return;
        }
        self.last = Some(self.search.step());
    }

    /// Whether the search has reached SAT or UNSAT.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.search.is_done()
    }

    /// Whether the search finished by *finding a solution* (SAT), as opposed to
    /// proving none exists (UNSAT) or still running. True exactly when the last
    /// event was [`Event::Sat`] — the signal that the puzzle is fully placed and
    /// its negation can be blocked to hunt for a second solution.
    #[must_use]
    pub fn is_solved(&self) -> bool {
        matches!(self.last, Some(Event::Sat))
    }

    /// A [`SolverView`] over the live search, for a
    /// [`Puzzle::project`](satsight_puzzles::Puzzle::project) to decode the
    /// mid-search state into the puzzle's own display grid — candidate lattice,
    /// center/corner marks, and the proven/hypothetical split, all derived in the
    /// backward map rather than here (issue #2).
    #[must_use]
    pub fn view(&self) -> SolverView<'_, V> {
        SolverView::from_search(&self.reg, &self.search)
    }

    /// The last event, for a view to name in its own vocabulary (the puzzle-neutral
    /// moves come pre-rendered from [`solver_phase`]).
    #[must_use]
    pub fn last_event(&self) -> Option<&Event> {
        self.last.as_ref()
    }

    /// The deepest decision level a given (assumption) occupies — the boundary
    /// between proven and hypothetical mid-search facts (plan §1).
    #[must_use]
    pub fn base_level(&self) -> u32 {
        self.search.base_level()
    }

    /// The truth value the search currently gives proposition `prop`:
    /// `Some(true)` if forced, `Some(false)` if ruled out, `None` if undecided or
    /// unregistered.
    #[must_use]
    pub fn value(&self, prop: &V) -> Option<bool> {
        self.reg.get(prop).and_then(|var| self.search.value_of(var))
    }

    /// The truth value of `prop` and the decision level it was set at, if assigned
    /// — the raw material for the proven/hypothetical split.
    #[must_use]
    pub fn assigned(&self, prop: &V) -> Option<(bool, u32)> {
        let var = self.reg.get(prop)?;
        Some((self.search.value_of(var)?, self.search.level_of(var)?))
    }

    /// Whether `prop`'s current assignment is **proven** (set at or below the
    /// givens' base level) or **hypothetical** (above it). Meaningful only for an
    /// assigned proposition; an unassigned one reports `Hypothetical`. Delegates to
    /// the core read model so the frontend and `project()` agree by construction.
    #[must_use]
    pub fn certainty(&self, prop: &V) -> Certainty {
        self.view().certainty(prop)
    }

    /// The propositions the last event touches: a decision/propagation target, or
    /// the propositions of a conflict clause. Empty for the other events. A view
    /// maps these onto its own coordinates (and de-duplicates) to highlight them.
    #[must_use]
    pub fn touched_props(&self) -> Vec<V> {
        match &self.last {
            Some(Event::Decide { lit } | Event::Propagate { lit, .. }) => {
                self.decode(*lit).into_iter().map(|(v, _)| v).collect()
            }
            Some(Event::Conflict { clause }) => self
                .search
                .clause_lits(*clause)
                .iter()
                .filter_map(|&lit| self.decode(lit).map(|(v, _)| v))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// On UNSAT, the propositions the core names (plan §4's core→givens); empty
    /// otherwise.
    #[must_use]
    pub fn core_props(&self) -> Vec<V> {
        match &self.last {
            Some(Event::Unsat { core }) => core
                .iter()
                .filter_map(|&lit| self.decode(lit).map(|(v, _)| v))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Decode a literal to its proposition and polarity, or `None` for an
    /// auxiliary variable that names no proposition. A positive literal decodes to
    /// `(v, true)` ("v holds"), a negative one to `(v, false)` ("v is ruled out").
    #[must_use]
    pub fn decode(&self, lit: Lit) -> Option<(V, bool)> {
        self.reg.decode(lit)
    }
}

/// Sudoku-specific event *narration*: the last move as a status line and the cells
/// it wants emphasized. The per-cell display state (digits, candidates, corner
/// marks) is no longer here — it is decoded generically by
/// [`Sudoku::project`](satsight_puzzles::sudoku::Sudoku) over [`Stepper::view`].
impl Stepper<Cell> {
    /// The cells the last event wants emphasized on the grid.
    #[must_use]
    pub fn emphasis(&self) -> Emphasis {
        let mut cells: Vec<(usize, usize)> = self
            .touched_props()
            .iter()
            .map(|cell| (cell.r, cell.c))
            .collect();
        cells.sort_unstable();
        cells.dedup();
        if cells.is_empty() {
            Emphasis::None
        } else {
            Emphasis::Cells(cells)
        }
    }

    /// On UNSAT, the given cells named by the core (plan §4's core→clues).
    #[must_use]
    pub fn core_cells(&self) -> Vec<(usize, usize)> {
        let mut cells: Vec<(usize, usize)> = self
            .core_props()
            .iter()
            .map(|cell| (cell.r, cell.c))
            .collect();
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    /// The last event, decoded into a one-line status for the UI.
    #[must_use]
    pub fn description(&self) -> String {
        let Some(event) = self.last_event() else {
            return READY.to_owned();
        };
        if let Some(phase) = solver_phase(event) {
            return phase;
        }
        match event {
            Event::Decide { lit } => {
                let (cell, holds) = self.cell_of(*lit);
                let rel = if holds { "=" } else { "≠" };
                format!("Guess: r{}c{} {rel} {}", cell.r + 1, cell.c + 1, cell.v)
            }
            Event::Propagate { lit, .. } => {
                let (cell, holds) = self.cell_of(*lit);
                if holds {
                    format!("Forced: r{}c{} = {}", cell.r + 1, cell.c + 1, cell.v)
                } else {
                    format!("Ruled out {} at r{}c{}", cell.v, cell.r + 1, cell.c + 1)
                }
            }
            Event::Sat => "Solved by search!".to_owned(),
            Event::Unsat { .. } => "Contradiction — these givens have no solution.".to_owned(),
            // Conflict/Backtrack/Learn are handled by `solver_phase` above.
            _ => unreachable!("solver_phase covers the remaining events"),
        }
    }

    /// Decode a literal to its Sudoku cell. Every rule/learned literal is a `Cell`
    /// proposition (Sudoku's pairwise encoding mints no aux variables), so this
    /// never fails in practice.
    fn cell_of(&self, lit: Lit) -> (Cell, bool) {
        self.decode(lit).expect("every literal decodes to a cell")
    }
}

#[cfg(test)]
mod tests {
    use super::Stepper;
    use satsight_core::{clause, Assignment, Cdcl, Clause, Cnf, Registry, SolveOutcome};
    use satsight_puzzles::sudoku::{Cell, Sudoku};
    use satsight_puzzles::Puzzle;

    const PUZZLE: &str =
        "53..7....6..195....98....6.8...6...34..8.3..17...2...6.6....28....419..5....8..79";
    const SOLUTION: &str =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

    /// Build the rule encoding and a stepper for `puzzle`.
    fn stepper_for(puzzle: &Sudoku) -> Stepper<Cell> {
        let mut reg: Registry<Cell> = Registry::new();
        let mut cnf = Cnf::new();
        puzzle.encode_rules(&mut reg, &mut cnf);
        let assumptions = puzzle.assumptions(&reg);
        let cdcl = Cdcl::from_cnf(&cnf);
        Stepper::new(&cdcl, reg, &assumptions)
    }

    /// The app's "Block solution" no-good: `¬(every placed cell of `model`)`, the
    /// OR of the negations of the true cell-propositions. Any *different* full
    /// board flips at least one of them, so this forbids exactly `model`.
    fn block_model(reg: &Registry<Cell>, model: &Assignment) -> Clause {
        let mut lits = Vec::new();
        for r in 0..9 {
            for c in 0..9 {
                for v in 1..=9u8 {
                    if let Some(var) = reg.get(&Cell { r, c, v }) {
                        if model.var_value(var) == Some(true) {
                            lits.push(var.neg_lit());
                        }
                    }
                }
            }
        }
        clause(lits)
    }

    #[test]
    fn stepping_to_completion_reaches_the_solution() {
        let puzzle = Sudoku::from_ascii(PUZZLE).unwrap();
        let mut stepper = stepper_for(&puzzle);
        let mut guard = 0;
        while !stepper.is_done() {
            stepper.step();
            guard += 1;
            assert!(guard < 1_000_000, "the search must terminate");
        }
        assert_eq!(stepper.description(), "Solved by search!");
        // The live search view projects to the unique solution through the puzzle's
        // own backward map — the accessor the app renders from.
        let grid = puzzle.project(&stepper.view());
        let sol = SOLUTION.as_bytes();
        for r in 0..9 {
            for c in 0..9 {
                assert_eq!(
                    grid.get(r, c).value,
                    Some(sol[r * 9 + c] - b'0'),
                    "cell r{r}c{c} should match the unique solution"
                );
            }
        }
    }

    #[test]
    fn stepper_view_agrees_with_a_direct_search_view() {
        // `Stepper::view` must be exactly a `SolverView::from_search` over the
        // stepper's own registry and search — the frontend read-model accessor.
        let puzzle = Sudoku::from_ascii(PUZZLE).unwrap();
        let mut stepper = stepper_for(&puzzle);
        for _ in 0..200 {
            stepper.step();
        }
        let view = stepper.view();
        // A handful of propositions must decode identically through the accessor.
        for (r, c, v) in [(0, 0, 5), (0, 2, 4), (4, 4, 5), (8, 8, 9)] {
            let cell = Cell { r, c, v };
            assert_eq!(view.value(&cell), stepper.value(&cell));
        }
    }

    #[test]
    fn unsat_core_names_the_conflicting_givens() {
        // Two 5s in row 0 contradict; the core must decode to exactly those cells.
        let mut ascii = String::from("55");
        ascii.push_str(&".".repeat(79));
        let puzzle = Sudoku::from_ascii(&ascii).unwrap();
        let mut stepper = stepper_for(&puzzle);
        while !stepper.is_done() {
            stepper.step();
        }
        assert_eq!(
            stepper.description(),
            "Contradiction — these givens have no solution."
        );
        let cells = stepper.core_cells();
        assert!(!cells.is_empty());
        for (r, c) in cells {
            assert_eq!(r, 0);
            assert!(c == 0 || c == 1);
        }
    }

    #[test]
    fn blocking_the_unique_solution_makes_the_search_unsat() {
        // The mechanism behind the "Block solution" button: the observable CDCL
        // stops at the first model, so to *prove* uniqueness we add ¬(that board)
        // and re-solve. A uniquely-solvable puzzle then comes back UNSAT — the
        // search has proven there is no second solution.
        let puzzle = Sudoku::from_ascii(PUZZLE).unwrap();
        let mut reg: Registry<Cell> = Registry::new();
        let mut cnf = Cnf::new();
        puzzle.encode_rules(&mut reg, &mut cnf);
        let assumptions = puzzle.assumptions(&reg);

        let cdcl = Cdcl::from_cnf(&cnf);
        let SolveOutcome::Sat(model) = cdcl.search(&assumptions).run() else {
            panic!("the sample puzzle is satisfiable");
        };
        // The blocked board must be the known unique solution.
        let sol = SOLUTION.as_bytes();
        for r in 0..9 {
            for c in 0..9 {
                let v = sol[r * 9 + c] - b'0';
                let var = reg.get(&Cell { r, c, v }).unwrap();
                assert_eq!(model.var_value(var), Some(true));
            }
        }

        cnf.add_clause(block_model(&reg, &model));
        let cdcl = Cdcl::from_cnf(&cnf);
        assert!(
            matches!(cdcl.search(&assumptions).run(), SolveOutcome::Unsat(_)),
            "blocking the unique solution must leave the puzzle UNSAT"
        );
    }

    #[test]
    fn blocking_one_of_many_solutions_yields_a_different_one() {
        // The other verdict: an empty board has many solutions, so blocking the
        // first still leaves the problem SAT — and the second model differs from
        // the first in at least one cell (that is what the no-good forces).
        let puzzle = Sudoku::empty();
        let mut reg: Registry<Cell> = Registry::new();
        let mut cnf = Cnf::new();
        puzzle.encode_rules(&mut reg, &mut cnf);
        let assumptions = puzzle.assumptions(&reg);

        let cdcl = Cdcl::from_cnf(&cnf);
        let SolveOutcome::Sat(first) = cdcl.search(&assumptions).run() else {
            panic!("an empty board is satisfiable");
        };

        cnf.add_clause(block_model(&reg, &first));
        let cdcl = Cdcl::from_cnf(&cnf);
        let SolveOutcome::Sat(second) = cdcl.search(&assumptions).run() else {
            panic!("an empty board has more than one solution");
        };

        let differs = (0..9)
            .flat_map(|r| (0..9).map(move |c| (r, c)))
            .any(|(r, c)| {
                (1..=9u8).any(|v| {
                    let var = reg.get(&Cell { r, c, v }).unwrap();
                    first.var_value(var) != second.var_value(var)
                })
            });
        assert!(differs, "the blocked board must not reappear");
    }
}
