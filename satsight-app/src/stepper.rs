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
//!   cell (plan §8's corner marks: BCP survivors);
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

/// The most recent short learned clauses to keep for the overlay.
const LEARNED_KEPT: usize = 16;

/// The longest learned clause worth showing as a "discovered relationship"
/// (plan §8: raw CDCL clauses are noisy; filter to short, readable ones).
const LEARNED_MAX_LITS: usize = 3;

/// Drives a [`Search`] event-by-event and decodes its state for the grid.
pub struct Stepper {
    reg: Registry<Cell>,
    search: Search,
    last: Option<Event>,
    /// Recently learned short clauses, decoded to `(cell, holds)` relationships
    /// (plan §8's center-mark / learned-clause feed).
    learned: Vec<Vec<(Cell, bool)>>,
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
            learned: Vec::new(),
            playing: false,
        }
    }

    /// Advance by one event, unless already terminal.
    pub fn step(&mut self) {
        if self.search.is_done() {
            return;
        }
        let event = self.search.step();
        // Retain short learned clauses, decoded to puzzle relationships.
        if let Event::Learn { clause } = &event {
            if clause.len() <= LEARNED_MAX_LITS {
                let rel: Vec<(Cell, bool)> = clause
                    .iter()
                    .filter_map(|&lit| self.reg.decode(lit))
                    .collect();
                if !rel.is_empty() {
                    self.learned.push(rel);
                    if self.learned.len() > LEARNED_KEPT {
                        let overflow = self.learned.len() - LEARNED_KEPT;
                        self.learned.drain(0..overflow);
                    }
                }
            }
        }
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

    /// Per-value corner-mark candidates for `(r, c)`: entry `v - 1` is `true` when
    /// value `v` is still Boolean-possible (not yet falsified).
    #[must_use]
    pub fn candidates(&self, r: usize, c: usize) -> [bool; 9] {
        std::array::from_fn(|i| {
            let v = u8::try_from(i + 1).expect("1..=9 fits in u8");
            self.value(r, c, v) != Some(false)
        })
    }

    /// The current truth value of proposition `Cell { r, c, v }`.
    fn value(&self, r: usize, c: usize, v: u8) -> Option<bool> {
        self.reg
            .get(&Cell { r, c, v })
            .and_then(|var| self.search.value_of(var))
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

    /// The retained learned clauses, each decoded to a one-line relationship in
    /// the puzzle's own language (e.g. `r1c2≠3 ∨ r4c2≠3`) — plan §9's side panel.
    #[must_use]
    pub fn learned_relationships(&self) -> Vec<String> {
        self.learned
            .iter()
            .map(|rel| format_relationship(rel))
            .collect()
    }

    /// Per-value center marks for `(r, c)`: entry `v - 1` is `true` when value `v`
    /// appears in a retained learned clause touching this cell — the
    /// discovered-relationship tier (plan §8), distinct from the corner-mark
    /// candidates.
    #[must_use]
    pub fn center_candidates(&self, r: usize, c: usize) -> [bool; 9] {
        let mut center = [false; 9];
        for rel in &self.learned {
            for (cell, _) in rel {
                if cell.r == r && cell.c == c {
                    center[usize::from(cell.v - 1)] = true;
                }
            }
        }
        center
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

/// Format a decoded learned clause as a readable disjunction, e.g.
/// `r1c2≠3 ∨ r4c2≠3` ("these two cells can't both be 3").
fn format_relationship(rel: &[(Cell, bool)]) -> String {
    rel.iter()
        .map(|(cell, holds)| {
            let op = if *holds { "=" } else { "≠" };
            format!("r{}c{}{op}{}", cell.r + 1, cell.c + 1, cell.v)
        })
        .collect::<Vec<_>>()
        .join(" ∨ ")
}

#[cfg(test)]
mod tests {
    use super::Stepper;
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
    fn learned_clauses_become_readable_relationships_and_center_marks() {
        // Inject a decoded learned clause (as the Learn path would) and check it
        // reads as a disjunction and surfaces as center marks on its cells.
        let mut stepper = stepper_for(&Sudoku::empty());
        stepper.learned.push(vec![
            (Cell { r: 0, c: 1, v: 3 }, false),
            (Cell { r: 3, c: 1, v: 3 }, false),
        ]);
        assert_eq!(
            stepper.learned_relationships(),
            vec!["r1c2≠3 ∨ r4c2≠3".to_string()]
        );
        // Value 3 (index 2) is flagged in both cells, and nowhere else.
        assert!(stepper.center_candidates(0, 1)[2]);
        assert!(stepper.center_candidates(3, 1)[2]);
        assert!(!stepper.center_candidates(0, 0)[2]);
        assert!(!stepper.center_candidates(0, 1)[0]);
    }

    #[test]
    fn stepping_only_ever_retains_short_decodable_relationships() {
        // Whatever the search learns while solving, the retained relationships
        // must all be short (≤3 literals) and decode to real cells — the filter
        // that keeps the overlay readable (plan §8).
        let puzzle = Sudoku::from_ascii(PUZZLE).unwrap();
        let mut stepper = stepper_for(&puzzle);
        while !stepper.is_done() {
            stepper.step();
        }
        for rel in &stepper.learned {
            assert!((1..=3).contains(&rel.len()), "retained clauses are short");
        }
        for line in stepper.learned_relationships() {
            assert!(line.contains('r'), "a relationship names cells");
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
}
