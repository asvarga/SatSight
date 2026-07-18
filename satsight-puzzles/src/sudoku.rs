//! Sudoku — the primary puzzle (plan §7).
//!
//! Sudoku is the ideal showcase for the bidirectional thesis: its reduction is
//! *nothing but* exactly-one constraints, and level-0 unit propagation
//! corresponds exactly to human naked/hidden singles, so watching forced cells
//! cascade **is** the backward map made visible. This module is the forward and
//! backward halves for Sudoku; the observability lives in later milestones.
//!
//! Encoding (729 variables, `Cell { r, c, v }`):
//!
//! - exactly one value per cell,
//! - each value exactly once per row, column, and 3×3 box.
//!
//! Givens are **assumptions**, not clauses (plan §4), so editing never touches
//! the CNF.

use satsight_core::cnf::{Cnf, Lit};
use satsight_core::encodings::exactly_one_pairwise;
use satsight_core::registry::Registry;
use satsight_core::view::SolverView;

use crate::puzzle::{deduce, Deductions, Grid, Puzzle};

/// Board side length.
const N: usize = 9;

/// A Sudoku proposition: "cell `(r, c)` holds value `v`" (the reduction's
/// vocabulary). `r`, `c` are `0..9`; `v` is `1..=9`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cell {
    pub r: usize,
    pub c: usize,
    pub v: u8,
}

/// Per-cell display state produced by [`Sudoku::project`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SudokuCell {
    /// The value determined for this cell (`1..=9`), if any.
    pub value: Option<u8>,
    /// Whether this cell is a user-supplied given.
    pub given: bool,
}

/// A Sudoku instance: just its givens. The rules are universal and live in
/// [`encode_rules`](Sudoku::encode_rules).
#[derive(Debug, Clone)]
pub struct Sudoku {
    /// `givens[r][c] == Some(v)` marks a clue; `None` is blank.
    givens: [[Option<u8>; N]; N],
}

impl Sudoku {
    /// An empty board (no givens).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            givens: [[None; N]; N],
        }
    }

    /// Parse an 81-cell board from ASCII: digits `1`–`9` are givens, `.` or `0`
    /// are blanks, and whitespace is ignored. Returns `None` unless exactly 81
    /// cells are present and every character is recognized.
    #[must_use]
    pub fn from_ascii(s: &str) -> Option<Self> {
        let mut board = Self::empty();
        let mut idx = 0usize;
        for ch in s.chars() {
            if ch.is_whitespace() {
                continue;
            }
            if idx >= N * N {
                return None;
            }
            let (r, c) = (idx / N, idx % N);
            match ch {
                '.' | '0' => {}
                '1'..='9' => board.givens[r][c] = Some(ch as u8 - b'0'),
                _ => return None,
            }
            idx += 1;
        }
        (idx == N * N).then_some(board)
    }

    /// The given at `(r, c)`, if any.
    #[must_use]
    pub fn given(&self, r: usize, c: usize) -> Option<u8> {
        self.givens[r][c]
    }

    /// Set (or clear, with `None`) the given at `(r, c)`. Because givens are
    /// assumptions, an edit only changes [`assumptions`](Sudoku::assumptions).
    pub fn set(&mut self, r: usize, c: usize, value: Option<u8>) {
        self.givens[r][c] = value;
    }

    /// A well-known Sudoku with a unique solution (the Wikipedia example).
    #[must_use]
    pub fn easy_sample() -> Self {
        Self::from_ascii(
            "53..7....\
             6..195...\
             .98....6.\
             8...6...3\
             4..8.3..1\
             7...2...6\
             .6....28.\
             ...419..5\
             ....8..79",
        )
        .expect("the built-in sample is a valid 81-cell board")
    }

    /// A deliberately hard Sudoku ("AI Escargot", a well-known minimal puzzle
    /// with a unique solution). Unlike [`easy_sample`](Sudoku::easy_sample),
    /// propagation + failed-literal probing places only one cell here, so the
    /// solver *must* search — making it the ideal default for watching the CDCL
    /// step past where pure logic stalls.
    #[must_use]
    pub fn hard_sample() -> Self {
        Self::from_ascii(
            "1....7.9.\
             .3..2...8\
             ..96..5..\
             ..53..9..\
             .1..8...2\
             6....4...\
             3......1.\
             .4......7\
             ..7...3..",
        )
        .expect("the built-in hard sample is a valid 81-cell board")
    }

    /// The nine `(r, c)` coordinates of box `b` (`0..9`, left-to-right,
    /// top-to-bottom).
    fn box_cells(b: usize) -> impl Iterator<Item = (usize, usize)> {
        let (base_r, base_c) = ((b / 3) * 3, (b % 3) * 3);
        (0..3).flat_map(move |i| (0..3).map(move |j| (base_r + i, base_c + j)))
    }
}

impl Default for Sudoku {
    fn default() -> Self {
        Self::empty()
    }
}

impl Puzzle for Sudoku {
    type Var = Cell;
    type Cell = SudokuCell;

    fn encode_rules(&self, reg: &mut Registry<Cell>, cnf: &mut Cnf) {
        // Register every proposition up front so variables are numbered densely
        // in cell-major order (and so any auxiliary-variable encoding could start
        // cleanly above them).
        for r in 0..N {
            for c in 0..N {
                for v in 1..=9u8 {
                    reg.var(Cell { r, c, v });
                }
            }
        }

        // Exactly one value per cell.
        for r in 0..N {
            for c in 0..N {
                let lits: Vec<Lit> = (1..=9u8)
                    .map(|v| reg.var(Cell { r, c, v }).pos_lit())
                    .collect();
                exactly_one_pairwise(&lits, cnf);
            }
        }
        // Each value exactly once per row.
        for r in 0..N {
            for v in 1..=9u8 {
                let lits: Vec<Lit> = (0..N)
                    .map(|c| reg.var(Cell { r, c, v }).pos_lit())
                    .collect();
                exactly_one_pairwise(&lits, cnf);
            }
        }
        // Each value exactly once per column.
        for c in 0..N {
            for v in 1..=9u8 {
                let lits: Vec<Lit> = (0..N)
                    .map(|r| reg.var(Cell { r, c, v }).pos_lit())
                    .collect();
                exactly_one_pairwise(&lits, cnf);
            }
        }
        // Each value exactly once per 3×3 box.
        for b in 0..N {
            for v in 1..=9u8 {
                let lits: Vec<Lit> = Self::box_cells(b)
                    .map(|(r, c)| reg.var(Cell { r, c, v }).pos_lit())
                    .collect();
                exactly_one_pairwise(&lits, cnf);
            }
        }
    }

    fn assumptions(&self, reg: &Registry<Cell>) -> Vec<Lit> {
        let mut assumps = Vec::new();
        for r in 0..N {
            for c in 0..N {
                if let Some(v) = self.givens[r][c] {
                    if let Some(var) = reg.get(&Cell { r, c, v }) {
                        assumps.push(var.pos_lit());
                    }
                }
            }
        }
        assumps
    }

    fn project(&self, view: &SolverView<Cell>) -> Grid<SudokuCell> {
        Grid::from_fn(N, N, |r, c| {
            let mut value = None;
            for v in 1..=9u8 {
                if view.value(&Cell { r, c, v }) == Some(true) {
                    value = Some(v);
                }
            }
            SudokuCell {
                value,
                given: self.givens[r][c].is_some(),
            }
        })
    }
}

/// A human-readable summary of what pure logic proves about a board (see
/// [`Sudoku::logic_report`]).
#[derive(Debug, Clone)]
pub struct LogicReport {
    /// Whether propagation found the givens consistent (see
    /// [`Deductions::satisfiable`]).
    pub satisfiable: bool,
    /// Number of givens on the board.
    pub givens: usize,
    /// Cells proven to a value by logic alone (excluding givens), sorted.
    pub placements: Vec<(usize, usize, u8)>,
    /// Number of candidate eliminations proven ("cell (r,c) can't be v").
    pub eliminations: usize,
}

impl LogicReport {
    /// Cells known after logic: givens plus proven placements.
    #[must_use]
    pub fn solved_cells(&self) -> usize {
        self.givens + self.placements.len()
    }

    /// Whether logic alone determined every one of the 81 cells.
    #[must_use]
    pub fn fully_solved(&self) -> bool {
        self.solved_cells() == N * N
    }
}

impl Sudoku {
    /// Number of givens on the board.
    #[must_use]
    pub fn given_count(&self) -> usize {
        self.givens
            .iter()
            .flat_map(|row| row.iter())
            .filter(|cell| cell.is_some())
            .count()
    }

    /// Summarize a set of [`Deductions`] for this board.
    #[must_use]
    pub fn logic_report_from(&self, deductions: &Deductions<Cell>) -> LogicReport {
        let mut placements = Vec::new();
        let mut eliminations = 0;
        for (cell, holds) in &deductions.proven {
            if *holds {
                placements.push((cell.r, cell.c, cell.v));
            } else {
                eliminations += 1;
            }
        }
        placements.sort_unstable();
        LogicReport {
            satisfiable: deductions.satisfiable,
            givens: self.given_count(),
            placements,
            eliminations,
        }
    }

    /// Solve as far as pure logic allows and summarize it — a one-shot
    /// convenience over [`deduce`] + [`logic_report_from`](Sudoku::logic_report_from).
    #[must_use]
    pub fn logic_report(&self) -> LogicReport {
        self.logic_report_from(&deduce(self))
    }

    /// A grid showing only what logic proves: givens plus forced placements.
    ///
    /// Cells that still need search are left blank — so rendering this next to
    /// the full solution shows exactly how far deduction alone gets.
    #[must_use]
    pub fn project_deductions(&self, deductions: &Deductions<Cell>) -> Grid<SudokuCell> {
        let mut values = self.givens;
        for (cell, holds) in &deductions.proven {
            if *holds {
                values[cell.r][cell.c] = Some(cell.v);
            }
        }
        Grid::from_fn(N, N, |r, c| SudokuCell {
            value: values[r][c],
            given: self.givens[r][c].is_some(),
        })
    }
}

/// Render a projected grid as text, with box separators.
#[must_use]
pub fn render(grid: &Grid<SudokuCell>) -> String {
    let mut out = String::new();
    for r in 0..grid.rows() {
        if r > 0 && r % 3 == 0 {
            out.push_str("------+-------+------\n");
        }
        for c in 0..grid.cols() {
            if c > 0 && c % 3 == 0 {
                out.push_str("| ");
            }
            match grid.get(r, c).value {
                Some(v) => {
                    out.push(char::from(b'0' + v));
                    out.push(' ');
                }
                None => out.push_str(". "),
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{render, Cell, Sudoku, SudokuCell};
    use crate::puzzle::{deduce, solve, Grid, Puzzle};
    use satsight_core::cdcl::Cdcl;
    use satsight_core::registry::Registry;
    use satsight_core::solver::{SolveOutcome, Solver};
    use satsight_core::view::SolverView;
    use satsight_core::Cnf;

    const PUZZLE: &str =
        "53..7....6..195....98....6.8...6...34..8.3..17...2...6.6....28....419..5....8..79";
    const SOLUTION: &str =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

    /// Flatten a projected grid into an 81-char digit string (`.` for blanks).
    fn grid_to_string(grid: &Grid<SudokuCell>) -> String {
        let mut s = String::new();
        for cell in grid.cells() {
            match cell.value {
                Some(v) => s.push(char::from(b'0' + v)),
                None => s.push('.'),
            }
        }
        s
    }

    /// Is every row, column, and box a permutation of 1..=9?
    fn is_valid_solution(grid: &Grid<SudokuCell>) -> bool {
        let digit = |r: usize, c: usize| grid.get(r, c).value;
        let full = |vals: [Option<u8>; 9]| {
            let mut seen = [false; 9];
            for v in vals {
                match v {
                    Some(d @ 1..=9) => {
                        if seen[(d - 1) as usize] {
                            return false;
                        }
                        seen[(d - 1) as usize] = true;
                    }
                    _ => return false,
                }
            }
            true
        };
        for r in 0..9 {
            if !full(std::array::from_fn(|c| digit(r, c))) {
                return false;
            }
        }
        for c in 0..9 {
            if !full(std::array::from_fn(|r| digit(r, c))) {
                return false;
            }
        }
        for b in 0..9 {
            let cells: Vec<_> = Sudoku::box_cells(b).collect();
            if !full(std::array::from_fn(|i| digit(cells[i].0, cells[i].1))) {
                return false;
            }
        }
        true
    }

    #[test]
    fn hard_sample_needs_search_but_stays_solvable() {
        // The default board must actually exercise search: logic alone leaves it
        // unfinished (that's the whole point of shipping it as the default), yet
        // the givens are consistent and the full solver reaches a solution.
        let puzzle = Sudoku::hard_sample();
        let report = puzzle.logic_report();
        assert!(report.satisfiable, "hard sample givens are consistent");
        assert!(
            !report.fully_solved(),
            "hard sample must leave work for search — logic solved {}/{} cells",
            report.solved_cells(),
            super::N * super::N,
        );
        let grid = solve(&puzzle).expect("hard sample is satisfiable");
        assert!(is_valid_solution(&grid), "full solve is a valid grid");
    }

    #[test]
    fn solves_known_puzzle_to_its_unique_solution() {
        let puzzle = Sudoku::from_ascii(PUZZLE).unwrap();
        let grid = solve(&puzzle).expect("the sample puzzle is satisfiable");
        assert_eq!(grid_to_string(&grid), SOLUTION);
        assert!(is_valid_solution(&grid));
    }

    #[test]
    fn observable_cdcl_solves_the_full_sudoku() {
        // Milestone 2: the hand-written stepping CDCL must handle the real
        // 729-variable instance and land on the unique solution — the same answer
        // BatSat gives (plan §5's oracle check, at full scale).
        let puzzle = Sudoku::from_ascii(PUZZLE).unwrap();
        let mut reg = Registry::new();
        let mut cnf = Cnf::new();
        puzzle.encode_rules(&mut reg, &mut cnf);
        let assumptions = puzzle.assumptions(&reg);

        let mut solver = Cdcl::from_cnf(&cnf);
        let SolveOutcome::Sat(model) = solver.solve(&assumptions) else {
            panic!("the sample puzzle is satisfiable");
        };
        let grid = puzzle.project(&SolverView::from_model(&reg, &model));
        assert_eq!(grid_to_string(&grid), SOLUTION);
        assert!(is_valid_solution(&grid));
    }

    #[test]
    fn observable_cdcl_reports_a_core_for_contradictory_givens() {
        // Two 5s in row 0: the stepping CDCL must return UNSAT with a core drawn
        // from the givens (plan §4) — the raw material for the core→clues highlight.
        let mut ascii = String::from("55");
        ascii.push_str(&".".repeat(79));
        let puzzle = Sudoku::from_ascii(&ascii).unwrap();
        let mut reg = Registry::new();
        let mut cnf = Cnf::new();
        puzzle.encode_rules(&mut reg, &mut cnf);
        let assumptions = puzzle.assumptions(&reg);

        let mut solver = Cdcl::from_cnf(&cnf);
        let SolveOutcome::Unsat(core) = solver.solve(&assumptions) else {
            panic!("two 5s in a row contradict");
        };
        assert!(!core.is_empty());
        // Every core literal decodes to one of the two conflicting givens.
        for lit in &core {
            let (cell, holds): (Cell, bool) = reg.decode(*lit).expect("core is over given vars");
            assert!(holds && cell.r == 0 && cell.v == 5 && (cell.c == 0 || cell.c == 1));
        }
    }

    #[test]
    fn model_round_trips_to_a_valid_grid() {
        // An empty board is satisfiable, and its model must decode to a legal
        // completed grid — the milestone-1 round-trip guarantee.
        let puzzle = Sudoku::empty();
        let grid = solve(&puzzle).expect("an empty board is satisfiable");
        assert!(is_valid_solution(&grid));
    }

    #[test]
    fn solution_respects_the_givens() {
        let puzzle = Sudoku::easy_sample();
        let grid = solve(&puzzle).expect("solvable");
        for r in 0..9 {
            for c in 0..9 {
                if let Some(v) = puzzle.given(r, c) {
                    assert_eq!(grid.get(r, c).value, Some(v));
                    assert!(grid.get(r, c).given);
                }
            }
        }
    }

    #[test]
    fn from_ascii_rejects_bad_input() {
        assert!(Sudoku::from_ascii("123").is_none()); // too short
        assert!(Sudoku::from_ascii(&"1".repeat(82)).is_none()); // too long
        assert!(Sudoku::from_ascii(&"x".repeat(81)).is_none()); // bad char
        assert!(Sudoku::from_ascii(&".".repeat(81)).is_some()); // empty board
    }

    #[test]
    fn render_has_grid_shape() {
        let grid = solve(&Sudoku::easy_sample()).unwrap();
        let text = render(&grid);
        // 9 value rows + 2 separator rows.
        assert_eq!(text.lines().count(), 11);
    }

    #[test]
    fn logic_proves_only_correct_facts() {
        // Every fact pure logic proves about the sample must agree with the
        // unique solution: placements are right, eliminations are genuinely
        // absent from the solution.
        let puzzle = Sudoku::easy_sample();
        let deductions = deduce(&puzzle);
        assert!(deductions.satisfiable);
        let sol = SOLUTION.as_bytes();
        for (cell, holds) in &deductions.proven {
            let solution_digit = sol[cell.r * 9 + cell.c] - b'0';
            if *holds {
                assert_eq!(cell.v, solution_digit, "forced placement must be correct");
            } else {
                assert_ne!(
                    cell.v, solution_digit,
                    "eliminated value must not be the answer"
                );
            }
        }
        // The sample is easy enough that logic makes real progress.
        let report = puzzle.logic_report_from(&deductions);
        assert!(report.solved_cells() > report.givens);
    }

    #[test]
    fn deduction_grid_matches_the_solution_where_filled() {
        let puzzle = Sudoku::easy_sample();
        let deductions = deduce(&puzzle);
        let partial = puzzle.project_deductions(&deductions);
        let sol = SOLUTION.as_bytes();
        for r in 0..9 {
            for c in 0..9 {
                if let Some(v) = partial.get(r, c).value {
                    assert_eq!(v, sol[r * 9 + c] - b'0');
                }
            }
        }
    }

    #[test]
    fn empty_board_proves_nothing() {
        // With no givens, logic cannot force anything, but the board is fine.
        let report = Sudoku::empty().logic_report();
        assert!(report.satisfiable);
        assert_eq!(report.givens, 0);
        assert!(report.placements.is_empty());
        assert_eq!(report.eliminations, 0);
    }

    #[test]
    fn contradictory_givens_are_detected_by_logic() {
        // Two 5s in row 0 — propagation alone must spot the contradiction.
        let mut ascii = String::from("55");
        ascii.push_str(&".".repeat(79));
        let puzzle = Sudoku::from_ascii(&ascii).unwrap();
        assert!(!puzzle.logic_report().satisfiable);
        // …and the full backend agrees it is unsolvable.
        assert!(solve(&puzzle).is_none());
    }
}
