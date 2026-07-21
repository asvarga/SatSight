//! Akari / Light Up — the cardinality puzzle (plan §7, issue #10).
//!
//! Where Sudoku and graph coloring are *nothing but* exactly-one plus binary
//! clauses, Akari is the plan's originally preferred second puzzle precisely
//! because it needs **at-most-k / exactly-k** cardinality constraints beyond
//! exactly-one — so it grows [`encodings`](satsight_core::encodings) in the
//! direction the plan intended and is a more convincing "not exactly-one shaped"
//! validator.
//!
//! You place light bulbs (**lamps**) on the white cells of a walled grid so that:
//!
//! - **No two lamps see each other** — a lamp lights its whole row- and
//!   column-run until a wall blocks it, so at most one lamp sits in each maximal
//!   run of white cells ([`at_most_one_pairwise`]).
//! - **Every white cell is lit** — some lamp shares its run, so each cell's
//!   *cross* (its row-run ∪ its column-run) holds at least one lamp
//!   ([`at_least_one`]).
//! - **Numbered walls are satisfied** — a wall carrying `k` has exactly `k` lamps
//!   among its orthogonal white neighbors ([`exactly_k_sequential`], the new
//!   cardinality encoding).
//!
//! The board (walls and numbers) is the fixed instance, so it becomes the *rules*;
//! the reduction's vocabulary is one proposition per white cell, [`Lamp`] ("a lamp
//! sits here"). Hand-placed lamps (and cells marked definitely-empty) are the
//! **givens**, and — exactly as for Sudoku's clues and coloring's pre-colorings —
//! they cross the bridge as [`assumptions`](Akari::assumptions), not clauses (plan
//! §4). So it implements the same [`Puzzle`] trait and rides the generic
//! [`solve`](crate::solve) / [`deduce`](crate::deduce) / [`backbone`](crate::backbone)
//! pipelines with no changes to `satsight-core`.

use satsight_core::cnf::{Cnf, Lit, VarManager};
use satsight_core::encodings::{at_least_one, at_most_one_pairwise, exactly_k_sequential};
use satsight_core::registry::Registry;
use satsight_core::view::SolverView;

use crate::puzzle::{Deductions, Grid, Puzzle};

/// One square of an Akari board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Square {
    /// A white cell — a lamp may go here.
    White,
    /// A black wall. `Some(k)` is a numbered clue wall demanding exactly `k`
    /// adjacent lamps; `None` is a plain wall.
    Wall(Option<u8>),
}

/// An Akari proposition: "a lamp sits on white cell `(r, c)`" — the reduction's
/// whole vocabulary. Only white cells get one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Lamp {
    pub r: usize,
    pub c: usize,
}

/// Per-square display state produced by [`Akari::project`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AkariCell {
    /// A wall, carrying its clue number if any.
    Wall(Option<u8>),
    /// A white cell: whether a lamp sits here (`Some(true)`), it is proven empty
    /// (`Some(false)`), or it is undetermined (`None`); whether it is lit by some
    /// known lamp; and whether its lamp state was a user given.
    White {
        lamp: Option<bool>,
        lit: bool,
        given: bool,
    },
}

/// An Akari instance: a fixed walled board plus any hand-placed lamp givens.
#[derive(Debug, Clone)]
pub struct Akari {
    rows: usize,
    cols: usize,
    /// Row-major squares.
    board: Vec<Square>,
    /// Row-major lamp givens over white cells: `Some(true)` a placed lamp,
    /// `Some(false)` a cell marked empty, `None` unmarked (or a wall).
    givens: Vec<Option<bool>>,
}

impl Akari {
    /// A blank all-white board of `rows × cols`.
    #[must_use]
    pub fn blank(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            board: vec![Square::White; rows * cols],
            givens: vec![None; rows * cols],
        }
    }

    /// Parse a board from ASCII, one row per line: `.` is a white cell, `#` a
    /// plain wall, and a digit `0`–`4` a numbered wall. Whitespace within a line
    /// (other than the newline separator) is ignored, so cells may be spaced out.
    /// Returns `None` unless every line has the same, non-zero cell count and every
    /// character is recognized.
    #[must_use]
    pub fn from_ascii(s: &str) -> Option<Self> {
        let mut board = Vec::new();
        let mut cols = None;
        let mut rows = 0;
        for line in s.lines() {
            let mut row = Vec::new();
            for ch in line.chars() {
                let square = match ch {
                    ' ' | '\t' => continue,
                    '.' => Square::White,
                    '#' => Square::Wall(None),
                    '0'..='4' => Square::Wall(Some(ch as u8 - b'0')),
                    _ => return None,
                };
                row.push(square);
            }
            if row.is_empty() {
                continue; // skip blank lines
            }
            match cols {
                None => cols = Some(row.len()),
                Some(w) if w != row.len() => return None,
                Some(_) => {}
            }
            board.extend(row);
            rows += 1;
        }
        let cols = cols?;
        (rows > 0).then(|| Self {
            rows,
            cols,
            givens: vec![None; rows * cols],
            board,
        })
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

    /// The square at `(r, c)`.
    #[must_use]
    pub fn square(&self, r: usize, c: usize) -> Square {
        self.board[self.idx(r, c)]
    }

    /// Whether `(r, c)` is a white cell.
    #[must_use]
    pub fn is_white(&self, r: usize, c: usize) -> bool {
        matches!(self.square(r, c), Square::White)
    }

    /// The lamp given at white cell `(r, c)`: `Some(true)` a placed lamp,
    /// `Some(false)` a cell marked empty, `None` unmarked.
    #[must_use]
    pub fn given(&self, r: usize, c: usize) -> Option<bool> {
        self.givens[self.idx(r, c)]
    }

    /// Set (or clear, with `None`) the lamp given at white cell `(r, c)`. A no-op
    /// on a wall. Because givens are assumptions, an edit only changes
    /// [`assumptions`](Akari::assumptions), never the rule CNF (plan §4).
    pub fn set_given(&mut self, r: usize, c: usize, value: Option<bool>) {
        let idx = self.idx(r, c);
        if matches!(self.board[idx], Square::White) {
            self.givens[idx] = value;
        }
    }

    /// Clear every lamp given.
    pub fn clear_givens(&mut self) {
        self.givens.iter_mut().for_each(|g| *g = None);
    }

    /// The number of lamp givens placed (either polarity).
    #[must_use]
    pub fn given_count(&self) -> usize {
        self.givens.iter().filter(|g| g.is_some()).count()
    }

    /// Whether `(r, c)` holds a lamp in a projected grid (walls and empty cells are
    /// `false`).
    #[must_use]
    pub fn lamp_at(grid: &Grid<AkariCell>, r: usize, c: usize) -> bool {
        matches!(
            grid.get(r, c),
            AkariCell::White {
                lamp: Some(true),
                ..
            }
        )
    }

    /// The cells a lamp on white cell `(r, c)` would light — its row-run and
    /// column-run (including itself). Public so a frontend can shade lit cells
    /// during a live search, where no projected grid exists yet.
    #[must_use]
    pub fn cross_cells(&self, r: usize, c: usize) -> Vec<(usize, usize)> {
        self.cross(r, c)
    }

    fn idx(&self, r: usize, c: usize) -> usize {
        r * self.cols + c
    }

    /// The white orthogonal neighbors of `(r, c)` — the cells a numbered wall
    /// counts lamps over.
    fn white_neighbors(&self, r: usize, c: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::with_capacity(4);
        if r > 0 && self.is_white(r - 1, c) {
            out.push((r - 1, c));
        }
        if r + 1 < self.rows && self.is_white(r + 1, c) {
            out.push((r + 1, c));
        }
        if c > 0 && self.is_white(r, c - 1) {
            out.push((r, c - 1));
        }
        if c + 1 < self.cols && self.is_white(r, c + 1) {
            out.push((r, c + 1));
        }
        out
    }

    /// The **cross** of white cell `(r, c)`: itself plus every white cell reachable
    /// along its row and column before a wall — exactly the cells a lamp here would
    /// light, and the cells whose lamps could light it. Used for the "every cell
    /// lit" constraint.
    fn cross(&self, r: usize, c: usize) -> Vec<(usize, usize)> {
        let mut out = vec![(r, c)];
        for cc in (0..c).rev() {
            if !self.is_white(r, cc) {
                break;
            }
            out.push((r, cc));
        }
        for cc in c + 1..self.cols {
            if !self.is_white(r, cc) {
                break;
            }
            out.push((r, cc));
        }
        for rr in (0..r).rev() {
            if !self.is_white(rr, c) {
                break;
            }
            out.push((rr, c));
        }
        for rr in r + 1..self.rows {
            if !self.is_white(rr, c) {
                break;
            }
            out.push((rr, c));
        }
        out
    }

    /// Every maximal run of contiguous white cells, both horizontal (each row left
    /// to right) and vertical (each column top to bottom). Two lamps in one run
    /// would see each other, so each run carries an at-most-one constraint.
    fn runs(&self) -> Vec<Vec<(usize, usize)>> {
        let mut runs = Vec::new();
        let flush = |run: &mut Vec<(usize, usize)>, runs: &mut Vec<Vec<(usize, usize)>>| {
            if run.len() > 1 {
                runs.push(std::mem::take(run));
            } else {
                run.clear();
            }
        };
        for r in 0..self.rows {
            let mut run = Vec::new();
            for c in 0..self.cols {
                if self.is_white(r, c) {
                    run.push((r, c));
                } else {
                    flush(&mut run, &mut runs);
                }
            }
            flush(&mut run, &mut runs);
        }
        for c in 0..self.cols {
            let mut run = Vec::new();
            for r in 0..self.rows {
                if self.is_white(r, c) {
                    run.push((r, c));
                } else {
                    flush(&mut run, &mut runs);
                }
            }
            flush(&mut run, &mut runs);
        }
        runs
    }

    /// Build a display grid from a per-cell lamp predicate. `lamp_state(r, c)` is
    /// `Some(true)` for a lamp, `Some(false)` for a known-empty cell, `None` for
    /// undetermined; a cell is lit if some known lamp shares its cross.
    fn render_grid(&self, lamp_state: impl Fn(usize, usize) -> Option<bool>) -> Grid<AkariCell> {
        Grid::from_fn(self.rows, self.cols, |r, c| match self.square(r, c) {
            Square::Wall(n) => AkariCell::Wall(n),
            Square::White => {
                let lit = self
                    .cross(r, c)
                    .into_iter()
                    .any(|(rr, cc)| lamp_state(rr, cc) == Some(true));
                AkariCell::White {
                    lamp: lamp_state(r, c),
                    lit,
                    given: self.given(r, c).is_some(),
                }
            }
        })
    }

    /// A grid showing only what a set of [`Deductions`] proves: givens plus lamps
    /// forced (or forced empty) by logic, with undetermined cells left blank.
    /// Mirrors the other puzzles' `project_deductions`, so the frontend renders
    /// logic and backbone results the same way it renders a full solution.
    #[must_use]
    pub fn project_deductions(&self, deductions: &Deductions<Lamp>) -> Grid<AkariCell> {
        let mut state = self.givens.clone();
        for (lamp, holds) in &deductions.proven {
            state[self.idx(lamp.r, lamp.c)] = Some(*holds);
        }
        self.render_grid(|r, c| state[self.idx(r, c)])
    }

    /// A 7×7 puzzle with a **unique** solution — the demo default. Its numbered
    /// walls span the whole cardinality range (a `0` forbidding neighbors, a `4`
    /// forcing all of them, and `1`/`2` in between), so the exactly-k encoding is
    /// exercised end to end, and pure logic already cracks most of it. Because the
    /// solution is unique, its backbone is the entire board.
    #[must_use]
    pub fn sample() -> Self {
        Self::from_ascii(
            "\
..2....
.4...1.
....2..
0..1..1
..1....
.1...1.
....1..",
        )
        .expect("the built-in sample is a valid board")
    }
}

impl Puzzle for Akari {
    type Var = Lamp;
    type Cell = AkariCell;

    fn encode_rules(&self, reg: &mut Registry<Lamp>, cnf: &mut Cnf) {
        // Register a lamp proposition for every white cell first, so puzzle
        // variables are dense and the cardinality encodings' auxiliaries can start
        // cleanly above them.
        for r in 0..self.rows {
            for c in 0..self.cols {
                if self.is_white(r, c) {
                    reg.var(Lamp { r, c });
                }
            }
        }
        let mut aux =
            VarManager::starting_at(u32::try_from(reg.len()).expect("var count fits u32"));

        // No two lamps see each other: at most one lamp per maximal white run.
        for run in self.runs() {
            let lits: Vec<Lit> = run
                .iter()
                .map(|&(r, c)| reg.var(Lamp { r, c }).pos_lit())
                .collect();
            at_most_one_pairwise(&lits, cnf);
        }

        // Every white cell is lit: at least one lamp in its cross.
        for r in 0..self.rows {
            for c in 0..self.cols {
                if self.is_white(r, c) {
                    let lits: Vec<Lit> = self
                        .cross(r, c)
                        .into_iter()
                        .map(|(rr, cc)| reg.var(Lamp { r: rr, c: cc }).pos_lit())
                        .collect();
                    at_least_one(&lits, cnf);
                }
            }
        }

        // Numbered walls: exactly k lamps among the white orthogonal neighbors —
        // the cardinality constraint that is the whole point of this puzzle.
        for r in 0..self.rows {
            for c in 0..self.cols {
                if let Square::Wall(Some(k)) = self.square(r, c) {
                    let lits: Vec<Lit> = self
                        .white_neighbors(r, c)
                        .into_iter()
                        .map(|(rr, cc)| reg.var(Lamp { r: rr, c: cc }).pos_lit())
                        .collect();
                    exactly_k_sequential(&lits, usize::from(k), &mut aux, cnf);
                }
            }
        }
    }

    fn assumptions(&self, reg: &Registry<Lamp>) -> Vec<Lit> {
        let mut assumps = Vec::new();
        for r in 0..self.rows {
            for c in 0..self.cols {
                if let Some(placed) = self.given(r, c) {
                    if let Some(var) = reg.get(&Lamp { r, c }) {
                        assumps.push(if placed { var.pos_lit() } else { var.neg_lit() });
                    }
                }
            }
        }
        assumps
    }

    fn project(&self, view: &SolverView<Lamp>) -> Grid<AkariCell> {
        self.render_grid(|r, c| view.value(&Lamp { r, c }))
    }
}

/// Render a projected grid as text: `#` a wall, a digit a numbered wall, `*` a
/// lamp, and `.` an unlit / `+` a lit empty cell. Handy for tests and debugging.
#[must_use]
pub fn render(grid: &Grid<AkariCell>) -> String {
    let mut out = String::new();
    for r in 0..grid.rows() {
        for c in 0..grid.cols() {
            out.push(match *grid.get(r, c) {
                AkariCell::Wall(Some(k)) => char::from(b'0' + k),
                AkariCell::Wall(None) => '#',
                AkariCell::White {
                    lamp: Some(true), ..
                } => '*',
                AkariCell::White { lit: true, .. } => '+',
                AkariCell::White { .. } => '.',
            });
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{render, Akari, AkariCell, Lamp, Square};
    use crate::puzzle::{backbone, deduce, solve, Grid, Puzzle};
    use satsight_core::cdcl::Cdcl;
    use satsight_core::cnf::Cnf;
    use satsight_core::registry::Registry;
    use satsight_core::solver::{SolveOutcome, Solver};
    use satsight_core::view::SolverView;
    use satsight_core::BatSatBackend;

    /// Check a projected grid against the three Akari rules, independently of the
    /// encoding: every white cell lit, no two lamps in a run, and every numbered
    /// wall matched. `require_full` also demands every white cell be decided (a lamp
    /// or a definite empty) — true for a complete solve, false for a partial one.
    fn obeys_rules(a: &Akari, grid: &Grid<AkariCell>, require_full: bool) -> bool {
        // Every white cell lit; and if required, decided.
        for r in 0..a.rows() {
            for c in 0..a.cols() {
                if let AkariCell::White { lit, lamp, .. } = *grid.get(r, c) {
                    if !lit {
                        return false;
                    }
                    if require_full && lamp.is_none() {
                        return false;
                    }
                }
            }
        }
        // No two lamps see each other: at most one lamp per maximal run.
        for run in a.runs() {
            let lamps = run
                .iter()
                .filter(|&&(r, c)| Akari::lamp_at(grid, r, c))
                .count();
            if lamps > 1 {
                return false;
            }
        }
        // Numbered walls satisfied.
        for r in 0..a.rows() {
            for c in 0..a.cols() {
                if let Square::Wall(Some(k)) = a.square(r, c) {
                    let lamps = a
                        .white_neighbors(r, c)
                        .into_iter()
                        .filter(|&(rr, cc)| Akari::lamp_at(grid, rr, cc))
                        .count();
                    if lamps != usize::from(k) {
                        return false;
                    }
                }
            }
        }
        true
    }

    #[test]
    fn from_ascii_round_trips_shape() {
        let a = Akari::from_ascii("..#\n.2.\n#..").unwrap();
        assert_eq!((a.rows(), a.cols()), (3, 3));
        assert_eq!(a.square(0, 2), Square::Wall(None));
        assert_eq!(a.square(1, 1), Square::Wall(Some(2)));
        assert!(a.is_white(0, 0));
    }

    #[test]
    fn from_ascii_rejects_ragged_or_bad_input() {
        assert!(Akari::from_ascii("..\n...").is_none()); // ragged rows
        assert!(Akari::from_ascii("..x").is_none()); // unknown char
        assert!(Akari::from_ascii("5..").is_none()); // 5 is not a valid clue
        assert!(Akari::from_ascii("").is_none()); // empty
        assert!(Akari::from_ascii(".#.").is_some()); // one row is fine
    }

    #[test]
    fn a_four_wall_forces_all_neighbors_by_logic() {
        // A center wall numbered 4 must light all four neighbors: pure logic
        // (propagation, no givens) forces a lamp on each — cardinality deduction
        // decoded straight back into the puzzle's own language. Diagonal walls keep
        // the wall's neighborhood to exactly the four orthogonal white cells.
        let a = Akari::from_ascii("#.#\n.4.\n#.#").unwrap();
        let d = deduce(&a);
        assert!(d.satisfiable);
        for (r, c) in [(0, 1), (1, 0), (1, 2), (2, 1)] {
            assert!(
                d.proven.contains(&(Lamp { r, c }, true)),
                "the 4-wall forces a lamp at ({r}, {c})"
            );
        }
    }

    #[test]
    fn a_zero_wall_forbids_neighbor_lamps() {
        // A "0" wall means none of its neighbors may hold a lamp; logic proves the
        // elimination. The neighbors must then be lit from elsewhere, but here we
        // only assert the forced non-placements.
        // A "0" wall at (1, 2) with roomy runs, so its neighbors can be lit from
        // elsewhere rather than needing (forbidden) lamps of their own.
        let a = Akari::from_ascii(".....\n..0..\n.....").unwrap();
        let d = deduce(&a);
        assert!(d.satisfiable);
        for (r, c) in [(0, 2), (1, 1), (1, 3), (2, 2)] {
            assert!(
                d.proven.contains(&(Lamp { r, c }, false)),
                "the 0-wall rules out a lamp at ({r}, {c})"
            );
        }
    }

    #[test]
    fn isolated_white_cell_must_hold_a_lamp() {
        // A lone white cell walled in on all sides can only be lit by itself, so a
        // lamp there is forced by logic (an at-least-one of one literal).
        let a = Akari::from_ascii("###\n#.#\n###").unwrap();
        let d = deduce(&a);
        assert!(d.satisfiable);
        assert!(d.proven.contains(&(Lamp { r: 1, c: 1 }, true)));
    }

    #[test]
    fn sample_solves_to_a_valid_board() {
        let a = Akari::sample();
        let grid = solve(&a).expect("the sample is solvable");
        assert!(
            obeys_rules(&a, &grid, true),
            "the full solve obeys every rule"
        );
    }

    #[test]
    fn sample_has_a_unique_solution() {
        // Backbone forces every cell iff the board has exactly one solution, and,
        // decoded through the reused project_deductions, it must reconstruct a full
        // valid board — the same guarantee the Sudoku suite checks.
        let a = Akari::sample();
        let bb = backbone(&a);
        assert!(bb.satisfiable);
        let grid = a.project_deductions(&bb);
        assert!(
            obeys_rules(&a, &grid, true),
            "a unique board's backbone fills and validates every cell"
        );
    }

    #[test]
    fn both_backends_agree_on_the_sample() {
        // The observable CDCL and BatSat must reach the same verdict, and the CDCL's
        // model must be a legal board (plan §5's oracle check, on the cardinality
        // puzzle).
        let a = Akari::sample();
        let mut reg: Registry<Lamp> = Registry::new();
        let mut cnf = Cnf::new();
        a.encode_rules(&mut reg, &mut cnf);
        let assumptions = a.assumptions(&reg);

        let cdcl = Cdcl::from_cnf(&cnf).solve(&assumptions);
        let batsat = BatSatBackend::solve_cnf(&cnf, &assumptions);
        match (&cdcl, &batsat) {
            (SolveOutcome::Sat(model), SolveOutcome::Sat(_)) => {
                let grid = a.project(&SolverView::from_model(&reg, model));
                assert!(obeys_rules(&a, &grid, true));
            }
            (SolveOutcome::Unsat(_), SolveOutcome::Unsat(_)) => {
                panic!("the sample is solvable")
            }
            _ => panic!("CDCL and BatSat disagree on the sample"),
        }
    }

    #[test]
    fn contradictory_given_is_unsat() {
        // Two hand-placed lamps in the same run see each other — no solution, and
        // propagation alone already spots it.
        let mut a = Akari::from_ascii(".....").unwrap();
        a.set_given(0, 0, Some(true));
        a.set_given(0, 3, Some(true));
        assert!(solve(&a).is_none());
        assert!(!deduce(&a).satisfiable);
    }

    #[test]
    fn placing_a_lamp_lights_its_run() {
        // A hand-placed lamp given must show up as a lamp, and the cells along its
        // run must read as lit in the projected solution.
        let mut a = Akari::from_ascii(".....").unwrap();
        a.set_given(0, 2, Some(true));
        let grid = solve(&a).expect("solvable");
        assert!(Akari::lamp_at(&grid, 0, 2));
        for c in 0..5 {
            assert!(
                matches!(grid.get(0, c), AkariCell::White { lit: true, .. }),
                "cell (0, {c}) is lit by the placed lamp"
            );
        }
    }

    #[test]
    fn empty_given_forbids_a_lamp_there() {
        // Marking a cell empty (a false given) must keep a lamp off it in the solve.
        let mut a = Akari::from_ascii(".....").unwrap();
        a.set_given(0, 0, Some(false));
        let grid = solve(&a).expect("solvable");
        assert!(!Akari::lamp_at(&grid, 0, 0));
    }

    #[test]
    fn set_given_ignores_walls() {
        let mut a = Akari::from_ascii(".#.").unwrap();
        a.set_given(0, 1, Some(true)); // (0,1) is a wall
        assert_eq!(a.given(0, 1), None);
        assert_eq!(a.given_count(), 0);
    }

    #[test]
    fn render_marks_walls_lamps_and_lit_cells() {
        // A single placed lamp: the lamp reads as `*`, the rest of its run as lit
        // `+`, and the wall stays `#`.
        let mut a = Akari::from_ascii("..#.").unwrap();
        a.set_given(0, 0, Some(true));
        let grid = solve(&a).expect("solvable");
        // (0,0) is the placed lamp, (0,1) is lit by it, (0,2) is the wall, and
        // (0,3) is a lone white cell past the wall that must hold its own lamp.
        assert_eq!(render(&grid).lines().next(), Some("*+#*"));
    }
}
