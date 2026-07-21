//! SatSight puzzles — the [`Puzzle`] trait and concrete puzzles.
//!
//! This crate proves the core's reduction abstraction is not Sudoku-shaped: a
//! puzzle is anything that can [`encode_rules`](puzzle::Puzzle::encode_rules),
//! offer its givens as [`assumptions`](puzzle::Puzzle::assumptions), and
//! [`project`](puzzle::Puzzle::project) a decoded solver view back onto a
//! [`Grid`]. Sudoku (the primary puzzle) lives in [`sudoku`]; a second puzzle
//! joins in a later milestone with no changes to `satsight-core`.
//!
//! [`deduce`] is the bidirectional thesis in miniature: it solves a puzzle by
//! *pure logic* (unit propagation + failed-literal probing) and reports the
//! forced placements and proven eliminations in the puzzle's own language — no
//! search, and generic over every [`Puzzle`].
//!
//! [`sudoku`] is the primary puzzle; [`coloring`] (graph coloring) and [`akari`]
//! (Light Up) prove the abstraction is not Sudoku-shaped, each crossing the same
//! bridge with a genuinely different constraint and no changes to `satsight-core`:
//! coloring adds edge-difference clauses, and akari adds **at-most-k / exactly-k**
//! cardinality (numbered walls), the constraint family beyond exactly-one that the
//! plan's second puzzle was chosen to exercise (plan §7).

pub mod akari;
pub mod coloring;
pub mod puzzle;
pub mod sudoku;

pub use puzzle::{backbone, deduce, solve, Deductions, Grid, Puzzle};
