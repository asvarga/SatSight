//! The stepping controller — the bridge between the observable CDCL and the grid.
//!
//! This is the frontend half of plan §6: it owns a [`Search`] and drives it one
//! [`Event`] at a time, decoding each move back into Sudoku terms through the
//! [`Registry`] so the UI can render it. Kept deliberately egui-free so the
//! decoding is unit-testable without a window.
//!
//! What it exposes to the renderer:
//!
//! - [`placed`](Stepper::placed) — the digit the search currently forces in a
//!   cell (its tentative value mid-search);
//! - [`candidates`](Stepper::candidates) — the values still Boolean-possible in a
//!   cell (plan §8's center marks: BCP survivors);
//! - [`corner_marks`](Stepper::corner_marks) — per 3×3 box, the values BCP has
//!   confined to a few cells ("the 7 goes in one of these cells");
//! - [`placement`](Stepper::placement) / [`hypo_eliminated`](Stepper::hypo_eliminated)
//!   — the same facts split by [`Certainty`]: entailed by the givens (proven) vs
//!   contingent on a search guess (hypothetical), which the grid draws differently;
//! - [`description`](Stepper::description) / [`emphasis`](Stepper::emphasis) — the
//!   last event, decoded to a human sentence and to the cells to highlight;
//! - [`core_cells`](Stepper::core_cells) — on UNSAT, the conflicting givens to
//!   flag (plan §4's core→clues highlight).

use satsight_core::{Cdcl, Event, Lit, Registry, Search};
use satsight_puzzles::sudoku::Cell;

/// Which cells the last event wants the renderer to emphasize.
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

/// A value confined by propagation to at most this many cells of a 3×3 box is
/// surfaced as a corner mark — the footprint of a hidden pair/triple ("one of
/// these cells"). Wider than that is no hint; a value pinned to a single cell is
/// a hidden single, shown as a placement instead.
const CORNER_MARK_MAX_CELLS: usize = 3;

/// Drives a [`Search`] event-by-event and decodes its state for the grid.
pub struct Stepper {
    reg: Registry<Cell>,
    search: Search,
    last: Option<Event>,
    /// Whether the UI is auto-advancing (owned here so the app can toggle it).
    pub playing: bool,
}

impl Stepper {
    /// Begin a fresh stepped search over `assumptions` (the current givens).
    #[must_use]
    pub fn new(cdcl: &Cdcl, reg: Registry<Cell>, assumptions: &[Lit]) -> Self {
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
        let event = self.search.step();
        self.last = Some(event);
    }

    /// Whether the search has reached SAT or UNSAT.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.search.is_done()
    }

    /// The digit the search currently forces in cell `(r, c)`, if any.
    #[must_use]
    pub fn placed(&self, r: usize, c: usize) -> Option<u8> {
        (1..=9u8).find(|&v| self.value(r, c, v) == Some(true))
    }

    /// The digit forced in `(r, c)` together with whether that placement is a
    /// **proven** fact (entailed by the givens alone via propagation) or a
    /// **hypothetical** one (contingent on a branching guess, undone on
    /// backtrack) — the split the grid colours differently (plan §1).
    #[must_use]
    pub fn placement(&self, r: usize, c: usize) -> Option<(u8, Certainty)> {
        let v = self.placed(r, c)?;
        Some((v, self.certainty(r, c, v)))
    }

    /// Values ruled out in `(r, c)` *only* under the current guess — hypothetical
    /// eliminations, falsified above the givens' base level. Entry `v - 1` is
    /// `true` for such a value. Proven eliminations (forced by the givens) are
    /// omitted: they are known non-facts, so the grid simply leaves them unmarked.
    #[must_use]
    pub fn hypo_eliminated(&self, r: usize, c: usize) -> [bool; 9] {
        let base = self.search.base_level();
        std::array::from_fn(|i| {
            let v = u8::try_from(i + 1).expect("1..=9 fits in u8");
            matches!(self.assigned(r, c, v), Some((false, level)) if level > base)
        })
    }

    /// Whether the current assignment of `Cell { r, c, v }` is proven (set at or
    /// below the givens' base level) or hypothetical (set above it).
    fn certainty(&self, r: usize, c: usize, v: u8) -> Certainty {
        let proven = self
            .assigned(r, c, v)
            .is_some_and(|(_, level)| level <= self.search.base_level());
        if proven {
            Certainty::Proven
        } else {
            Certainty::Hypothetical
        }
    }

    /// Per-value center-mark candidates for `(r, c)`: entry `v - 1` is `true` when
    /// value `v` is still Boolean-possible (not yet falsified).
    #[must_use]
    pub fn candidates(&self, r: usize, c: usize) -> [bool; 9] {
        std::array::from_fn(|i| {
            let v = u8::try_from(i + 1).expect("1..=9 fits in u8");
            self.value(r, c, v) != Some(false)
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

    /// The current truth value of proposition `Cell { r, c, v }`.
    fn value(&self, r: usize, c: usize, v: u8) -> Option<bool> {
        self.reg
            .get(&Cell { r, c, v })
            .and_then(|var| self.search.value_of(var))
    }

    /// The truth value of `Cell { r, c, v }` and the decision level it was set
    /// at, if assigned — the raw material for the proven/hypothetical split.
    fn assigned(&self, r: usize, c: usize, v: u8) -> Option<(bool, u32)> {
        let var = self.reg.get(&Cell { r, c, v })?;
        Some((self.search.value_of(var)?, self.search.level_of(var)?))
    }

    /// The last event, decoded into a one-line status for the UI.
    #[must_use]
    pub fn description(&self) -> String {
        let Some(event) = &self.last else {
            return "Ready — press Step or Play to watch the solver.".to_owned();
        };
        match event {
            Event::Decide { lit } => {
                let (cell, holds) = self.decode(*lit);
                let rel = if holds { "=" } else { "≠" };
                format!("Guess: r{}c{} {rel} {}", cell.r + 1, cell.c + 1, cell.v)
            }
            Event::Propagate { lit, .. } => {
                let (cell, holds) = self.decode(*lit);
                if holds {
                    format!("Forced: r{}c{} = {}", cell.r + 1, cell.c + 1, cell.v)
                } else {
                    format!("Ruled out {} at r{}c{}", cell.v, cell.r + 1, cell.c + 1)
                }
            }
            Event::Conflict { .. } => "Conflict — analyzing and backtracking.".to_owned(),
            Event::Backtrack { to_level } => format!("Backtrack to level {to_level}."),
            Event::Learn { clause } => {
                format!("Learned a clause ({} literals).", clause.len())
            }
            Event::Sat => "Solved by search!".to_owned(),
            Event::Unsat { .. } => "Contradiction — these givens have no solution.".to_owned(),
        }
    }

    /// The cells the last event wants emphasized on the grid.
    #[must_use]
    pub fn emphasis(&self) -> Emphasis {
        match &self.last {
            Some(Event::Decide { lit } | Event::Propagate { lit, .. }) => {
                let (cell, _) = self.decode(*lit);
                Emphasis::Cells(vec![(cell.r, cell.c)])
            }
            Some(Event::Conflict { clause }) => {
                let mut cells: Vec<(usize, usize)> = self
                    .search
                    .clause_lits(*clause)
                    .iter()
                    .map(|&lit| {
                        let (cell, _) = self.decode(lit);
                        (cell.r, cell.c)
                    })
                    .collect();
                cells.sort_unstable();
                cells.dedup();
                Emphasis::Cells(cells)
            }
            _ => Emphasis::None,
        }
    }

    /// On UNSAT, the given cells named by the core (plan §4's core→clues).
    #[must_use]
    pub fn core_cells(&self) -> Vec<(usize, usize)> {
        match &self.last {
            Some(Event::Unsat { core }) => {
                let mut cells: Vec<(usize, usize)> = core
                    .iter()
                    .map(|&lit| {
                        let (cell, _) = self.decode(lit);
                        (cell.r, cell.c)
                    })
                    .collect();
                cells.sort_unstable();
                cells.dedup();
                cells
            }
            _ => Vec::new(),
        }
    }

    /// Decode a literal to its Sudoku cell. Every rule/learned literal is a
    /// `Cell` proposition (Sudoku's pairwise encoding mints no aux variables), so
    /// this never fails in practice.
    fn decode(&self, lit: Lit) -> (Cell, bool) {
        self.reg
            .decode(lit)
            .expect("every literal decodes to a cell")
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
    use satsight_core::{Cdcl, Cnf, Registry};
    use satsight_puzzles::sudoku::{Cell, Sudoku};
    use satsight_puzzles::Puzzle;

    const PUZZLE: &str =
        "53..7....6..195....98....6.8...6...34..8.3..17...2...6.6....28....419..5....8..79";
    const SOLUTION: &str =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

    /// Build the rule encoding and a stepper for `puzzle`.
    fn stepper_for(puzzle: &Sudoku) -> Stepper {
        let mut reg: Registry<Cell> = Registry::new();
        let mut cnf = Cnf::new();
        puzzle.encode_rules(&mut reg, &mut cnf);
        let assumptions = puzzle.assumptions(&reg);
        let cdcl = Cdcl::from_cnf(&cnf);
        Stepper::new(&cdcl, reg, &assumptions)
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
}
