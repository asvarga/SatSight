//! SatSight puzzles — the [`Puzzle`] trait and concrete puzzles.
//!
//! This crate proves the core's reduction abstraction is not Sudoku-shaped: a
//! puzzle is anything that can [`encode_rules`](puzzle::Puzzle::encode_rules),
//! offer its givens as [`assumptions`](puzzle::Puzzle::assumptions), and
//! [`project`](puzzle::Puzzle::project) a decoded solver view back onto a
//! [`Grid`]. Sudoku (the primary puzzle) lives in [`sudoku`]; a second puzzle
//! joins in a later milestone with no changes to `satsight-core`.

pub mod puzzle;
pub mod sudoku;

pub use puzzle::{solve, Grid, Puzzle};
