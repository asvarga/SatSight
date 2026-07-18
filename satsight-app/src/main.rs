//! SatSight demo frontend (placeholder).
//!
//! The real UI is an eframe/egui app that steps the observable CDCL and renders
//! the solver's discoveries back onto the grid (plan §9); it arrives in
//! milestone 4 (native) and milestone 6 (wasm via trunk). For now this binary
//! just exercises the forward path so the workspace has a runnable entry point.

use satsight_puzzles::solve;
use satsight_puzzles::sudoku::{render, Sudoku};

fn main() {
    let puzzle = Sudoku::easy_sample();
    match solve(&puzzle) {
        Some(grid) => {
            println!("SatSight — solved the sample Sudoku:\n");
            print!("{}", render(&grid));
        }
        None => println!("SatSight — sample Sudoku was unsatisfiable (unexpected)."),
    }
}
