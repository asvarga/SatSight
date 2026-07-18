//! SatSight demo frontend (placeholder).
//!
//! The real UI is an eframe/egui app that steps the observable CDCL and renders
//! the solver's discoveries back onto the grid (plan §9); it arrives in
//! milestone 4 (native) and milestone 6 (wasm via trunk). For now this binary
//! exercises the forward path *and* the backward map: it shows how far pure
//! logic (unit propagation + failed-literal probing) gets before any search,
//! then the full BatSat solution.

use satsight_puzzles::sudoku::{render, Sudoku};
use satsight_puzzles::{deduce, solve};

fn main() {
    let puzzle = Sudoku::easy_sample();

    println!("SatSight — the given puzzle:\n");
    print!(
        "{}",
        render(&puzzle.project_deductions(&empty_deductions()))
    );

    // Backward map: what the solver can *prove* in the puzzle's own language,
    // with no search at all.
    let deductions = deduce(&puzzle);
    let report = puzzle.logic_report_from(&deductions);

    println!("\nPure logic (propagation + probing) proves:\n");
    print!("{}", render(&puzzle.project_deductions(&deductions)));
    println!(
        "\n  {} givens + {} deduced = {}/81 cells; {} candidate eliminations proven.",
        report.givens,
        report.placements.len(),
        report.solved_cells(),
        report.eliminations,
    );
    if report.fully_solved() {
        println!("  Solved entirely by logic — no search needed.");
    } else {
        println!("  Search is needed for the remaining cells.");
    }

    // Forward path: the complete model, decoded back to a grid.
    match solve(&puzzle) {
        Some(grid) => {
            println!("\nFull solution (BatSat):\n");
            print!("{}", render(&grid));
        }
        None => println!("\nUnsatisfiable (unexpected for the sample)."),
    }
}

/// An empty deduction set, used to render the bare givens.
fn empty_deductions() -> satsight_puzzles::Deductions<satsight_puzzles::sudoku::Cell> {
    satsight_puzzles::Deductions {
        satisfiable: true,
        proven: Vec::new(),
    }
}
