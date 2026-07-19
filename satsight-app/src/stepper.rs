//! The stepping controller — the bridge between the observable CDCL and a view.
//!
//! This is the frontend half of plan §6: it owns a [`Search`] and drives it one
//! [`Event`] at a time, decoding each move back into the puzzle's own vocabulary
//! through the [`Registry`] so the UI can render it. Kept deliberately egui-free
//! so the decoding is unit-testable without a window.
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
//!   shared status text for the puzzle-agnostic moves (conflict/backtrack/learn).
//!
//! Each puzzle view composes these into its domain shapes: Sudoku's per-cell
//! candidate digits and box-confinement corner marks live in the [`Stepper<Cell>`]
//! impl below; graph coloring's per-vertex color pips live in the coloring view.

use std::hash::Hash;

use satsight_core::{Cdcl, Event, Lit, Registry, Search};
use satsight_puzzles::sudoku::Cell;

/// Which cells the last event wants the Sudoku renderer to emphasize.
pub enum Emphasis {
    /// Nothing in particular.
    None,
    /// A decision/propagation target, or the cells of a conflict clause.
    Cells(Vec<(usize, usize)>),
}

/// Whether a mid-search fact is entailed by the givens alone or is contingent on
/// a search assumption — the distinction the grid draws differently (plan §1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Certainty {
    /// Forced by the givens (assumptions) via propagation: a **known** fact that
    /// holds in every solution consistent with the clues.
    Proven,
    /// Placed or ruled out only under the current branching guess: a
    /// **hypothetical** fact that may be undone on backtrack.
    Hypothetical,
}

/// The status line shown before the first step — shared across puzzle views.
pub const READY: &str = "Ready — press Step or Play to watch the solver.";

/// A value confined by propagation to at most this many cells of a 3×3 box is
/// surfaced as a corner mark — the footprint of a hidden pair/triple ("one of
/// these cells"). Wider than that is no hint; a value pinned to a single cell is
/// a hidden single, shown as a placement instead.
const CORNER_MARK_MAX_CELLS: usize = 3;

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

/// Drives a [`Search`] event-by-event and decodes its state for a puzzle view.
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
    /// assigned proposition; an unassigned one reports `Hypothetical`.
    #[must_use]
    pub fn certainty(&self, prop: &V) -> Certainty {
        let proven = self
            .assigned(prop)
            .is_some_and(|(_, level)| level <= self.base_level());
        if proven {
            Certainty::Proven
        } else {
            Certainty::Hypothetical
        }
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

/// Sudoku-specific read model: the per-cell digits, candidates, and box
/// confinement the 9×9 grid renders. Composed from the generic primitives above.
impl Stepper<Cell> {
    /// The digit the search currently forces in cell `(r, c)`, if any.
    #[must_use]
    pub fn placed(&self, r: usize, c: usize) -> Option<u8> {
        (1..=9u8).find(|&v| self.value(&Cell { r, c, v }) == Some(true))
    }

    /// The digit forced in `(r, c)` together with whether that placement is a
    /// **proven** fact (entailed by the givens alone via propagation) or a
    /// **hypothetical** one (contingent on a branching guess, undone on
    /// backtrack) — the split the grid colours differently (plan §1).
    #[must_use]
    pub fn placement(&self, r: usize, c: usize) -> Option<(u8, Certainty)> {
        let v = self.placed(r, c)?;
        Some((v, self.certainty(&Cell { r, c, v })))
    }

    /// Values ruled out in `(r, c)` *only* under the current guess — hypothetical
    /// eliminations, falsified above the givens' base level. Entry `v - 1` is
    /// `true` for such a value. Proven eliminations (forced by the givens) are
    /// omitted: they are known non-facts, so the grid simply leaves them unmarked.
    #[must_use]
    pub fn hypo_eliminated(&self, r: usize, c: usize) -> [bool; 9] {
        let base = self.base_level();
        std::array::from_fn(|i| {
            let v = u8::try_from(i + 1).expect("1..=9 fits in u8");
            matches!(self.assigned(&Cell { r, c, v }), Some((false, level)) if level > base)
        })
    }

    /// Per-value center-mark candidates for `(r, c)`: entry `v - 1` is `true` when
    /// value `v` is still Boolean-possible (not yet falsified).
    #[must_use]
    pub fn candidates(&self, r: usize, c: usize) -> [bool; 9] {
        std::array::from_fn(|i| {
            let v = u8::try_from(i + 1).expect("1..=9 fits in u8");
            self.value(&Cell { r, c, v }) != Some(false)
        })
    }

    /// Per-value corner marks for `(r, c)`: entry `v - 1` is `true` when value
    /// `v` is Boolean-confined within this cell's 3×3 box to a small set of cells
    /// that includes this one — the "the 7 goes in one of these cells in the box"
    /// hint (a hidden pair/triple footprint), distinct from the per-cell center
    /// marks.
    #[must_use]
    pub fn corner_marks(&self, r: usize, c: usize) -> [bool; 9] {
        let (br, bc) = (r - r % 3, c - c % 3);
        let box_candidates: [[bool; 9]; 9] =
            std::array::from_fn(|i| self.candidates(br + i / 3, bc + i % 3));
        let local = (r - br) * 3 + (c - bc);
        box_corner_marks(&box_candidates)[local]
    }

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

/// From the per-cell candidate arrays of a 3×3 box (row-major, cell `i`'s entry
/// `v` telling whether value `v + 1` is still possible there), the corner marks
/// per box-cell: value `v` is a corner mark wherever it's still a candidate,
/// provided the whole box admits it in `2..=CORNER_MARK_MAX_CELLS` cells — few
/// enough to read as "`v` goes in one of these cells." (A value admitted in a
/// single cell is a hidden single, shown as a placement instead.)
fn box_corner_marks(box_candidates: &[[bool; 9]; 9]) -> [[bool; 9]; 9] {
    let mut marks = [[false; 9]; 9];
    for v in 0..9 {
        let count = (0..9).filter(|&i| box_candidates[i][v]).count();
        if (2..=CORNER_MARK_MAX_CELLS).contains(&count) {
            for (i, cell) in marks.iter_mut().enumerate() {
                cell[v] = box_candidates[i][v];
            }
        }
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::{Certainty, Stepper};
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
        let sol = SOLUTION.as_bytes();
        for r in 0..9 {
            for c in 0..9 {
                assert_eq!(
                    stepper.placed(r, c),
                    Some(sol[r * 9 + c] - b'0'),
                    "cell r{r}c{c} should match the unique solution"
                );
            }
        }
    }

    #[test]
    fn candidates_shrink_as_the_search_places_givens() {
        let puzzle = Sudoku::from_ascii(PUZZLE).unwrap();
        let mut stepper = stepper_for(&puzzle);
        // Drive to completion, then every solved cell has exactly one candidate.
        while !stepper.is_done() {
            stepper.step();
        }
        for r in 0..9 {
            for c in 0..9 {
                let live = stepper.candidates(r, c).iter().filter(|&&b| b).count();
                assert_eq!(live, 1, "a solved cell keeps a single candidate");
            }
        }
    }

    #[test]
    fn box_confinement_surfaces_as_corner_marks() {
        // A box where value 7 (index 6) is admitted in exactly three cells is a
        // "one of these cells" hint on those three; a value spread across the
        // whole box, or pinned to a single cell, is not.
        let mut cand = [[true; 9]; 9];
        for row in cand.iter_mut().skip(3) {
            row[6] = false; // value 7 only in box-cells 0, 1, 2
        }
        for (i, row) in cand.iter_mut().enumerate() {
            row[0] = i == 4; // value 1 pinned to a single box-cell (index 4)
        }
        let marks = super::box_corner_marks(&cand);
        assert!(marks[0][6] && marks[1][6] && marks[2][6]);
        assert!(marks.iter().skip(3).all(|cell| !cell[6]));
        // Value 1 sits in one cell (hidden single) → no corner mark anywhere.
        assert!(marks.iter().all(|cell| !cell[0]));
        // Value 2 (index 1) is admitted everywhere (nine cells) → not confined.
        assert!(marks.iter().all(|cell| !cell[1]));
    }

    #[test]
    fn corner_marks_appear_over_a_real_search() {
        // On a real puzzle, propagation confines some value to a few cells of a
        // box at some point during the solve — the corner-mark hint must fire on
        // genuine solver state, not just synthetic candidate arrays.
        let puzzle = Sudoku::from_ascii(PUZZLE).unwrap();
        let mut stepper = stepper_for(&puzzle);
        let mut ever = false;
        while !stepper.is_done() {
            stepper.step();
            ever |= (0..9)
                .flat_map(|r| (0..9).map(move |c| (r, c)))
                .any(|(r, c)| stepper.corner_marks(r, c).iter().any(|&m| m));
            if ever {
                break;
            }
        }
        assert!(
            ever,
            "a real search should surface at least one corner mark"
        );
    }

    #[test]
    fn a_solved_board_leaves_no_corner_marks() {
        // Once solved, each value sits in exactly one cell of every box, so no
        // box confinement remains to surface.
        let puzzle = Sudoku::from_ascii(PUZZLE).unwrap();
        let mut stepper = stepper_for(&puzzle);
        while !stepper.is_done() {
            stepper.step();
        }
        for r in 0..9 {
            for c in 0..9 {
                assert!(
                    stepper.corner_marks(r, c).iter().all(|&m| !m),
                    "a solved board leaves no box-confined corner marks at r{r}c{c}"
                );
            }
        }
    }

    #[test]
    fn given_cells_are_proven_facts() {
        // Givens enter the search as assumptions (base-level decisions), so they
        // and their propagated consequences read as proven — never hypotheses —
        // no matter how much branching the rest of the solve needs.
        let puzzle = Sudoku::from_ascii(PUZZLE).unwrap();
        let mut stepper = stepper_for(&puzzle);
        while !stepper.is_done() {
            stepper.step();
        }
        let bytes = PUZZLE.as_bytes();
        for r in 0..9 {
            for c in 0..9 {
                if bytes[r * 9 + c] != b'.' {
                    assert_eq!(
                        stepper.placement(r, c).map(|(_, cert)| cert),
                        Some(Certainty::Proven),
                        "given r{r}c{c} is a known fact"
                    );
                }
            }
        }
    }

    #[test]
    fn empty_board_search_is_all_hypothetical() {
        // With no givens the base level is 0, so the first cell the search fills —
        // and its knock-on eliminations — are contingent on that guess, not proven.
        let mut stepper = stepper_for(&Sudoku::empty());
        let placed = loop {
            stepper.step();
            if let Some(cell) = (0..9)
                .flat_map(|r| (0..9).map(move |c| (r, c)))
                .find(|&(r, c)| stepper.placed(r, c).is_some())
            {
                break cell;
            }
            assert!(
                !stepper.is_done(),
                "the search fills a cell before finishing"
            );
        };
        let (r, c) = placed;
        assert_eq!(
            stepper.placement(r, c).map(|(_, cert)| cert),
            Some(Certainty::Hypothetical),
            "with no givens, a placed cell is a hypothesis"
        );
        // The value the guess placed is ruled out (hypothetically) elsewhere in
        // its house once propagation runs.
        let v = stepper.placed(r, c).expect("the cell holds a value");
        for _ in 0..40 {
            stepper.step();
        }
        let ruled_out_in_house = (0..9)
            .filter(|&cc| cc != c)
            .any(|cc| stepper.hypo_eliminated(r, cc)[usize::from(v - 1)]);
        assert!(
            ruled_out_in_house,
            "the guess contingently rules its value out along the row"
        );
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
