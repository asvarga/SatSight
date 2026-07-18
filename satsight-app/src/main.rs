//! SatSight demo frontend — an eframe/egui app (plan §9).
//!
//! Renders the Sudoku grid, lets you edit givens (click a cell, type 1–9, or
//! backspace to clear), and visualizes the two directions of the reduction:
//!
//! - **Deduce (logic)** runs the sound, search-free backward map — unit
//!   propagation + failed-literal probing — and paints every cell it can *prove*
//!   in a distinct colour, with a summary of what logic alone settled.
//! - **Full solve** runs the BatSat backend to completion and fills the rest.
//!
//! Edits are cheap because givens are assumptions, not clauses (plan §4): changing
//! a clue just invalidates the cached results. The animated, single-stepping
//! overlays (trail, conflicts, marks) arrive with the observable CDCL.
//!
//! Pixel maths converts small grid indices to `f32`, so the usual cast lints are
//! allowed for this crate only; the core and puzzles crates stay strict.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use eframe::egui;
use satsight_puzzles::sudoku::{LogicReport, Sudoku, SudokuCell};
use satsight_puzzles::{deduce, solve, Grid};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([560.0, 720.0])
            .with_min_inner_size([420.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SatSight",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}

/// Which decoded artifact the grid is currently painting over the givens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    /// Just the givens.
    None,
    /// Cells the backward map (propagation + probing) proves.
    Logic,
    /// The complete model from the full solve.
    Full,
}

/// Where a displayed digit came from — drives its colour.
enum Source {
    Given,
    Logic,
    Full,
}

/// The application state. All solver outputs are cached and invalidated on edit.
struct App {
    puzzle: Sudoku,
    selected: Option<(usize, usize)>,
    overlay: Overlay,
    /// Givens + logic-proven placements, or `None` until "Deduce" is pressed.
    logic: Option<Grid<SudokuCell>>,
    /// Summary of the last deduction.
    report: Option<LogicReport>,
    /// The full solution, or `None` until "Full solve" is pressed.
    full: Option<Grid<SudokuCell>>,
    status: String,
}

impl App {
    fn new() -> Self {
        Self {
            puzzle: Sudoku::easy_sample(),
            selected: None,
            overlay: Overlay::None,
            logic: None,
            report: None,
            full: None,
            status: "Click a cell and type 1–9 to edit. Then Deduce or Full solve.".to_owned(),
        }
    }

    /// Replace the board and drop every cached result.
    fn load(&mut self, puzzle: Sudoku) {
        self.puzzle = puzzle;
        self.selected = None;
        self.invalidate();
        self.status.clear();
    }

    /// Drop cached solver outputs (after an edit) and stop overlaying them.
    fn invalidate(&mut self) {
        self.overlay = Overlay::None;
        self.logic = None;
        self.report = None;
        self.full = None;
    }

    /// Set or clear the given at the selected cell, invalidating results.
    fn edit_selected(&mut self, value: Option<u8>) {
        let Some((r, c)) = self.selected else { return };
        self.puzzle.set(r, c, value);
        self.invalidate();
        self.status = String::from("Edited — press Deduce or Full solve.");
    }

    /// Run the backward map: propagation + probing, decoded to the grid.
    fn run_logic(&mut self) {
        let deductions = deduce(&self.puzzle);
        let report = self.puzzle.logic_report_from(&deductions);
        self.status = if report.satisfiable {
            let solved = if report.fully_solved() {
                "  Fully solved by logic — no search needed!"
            } else {
                "  Search would be needed for the rest."
            };
            format!(
                "Logic: {} givens + {} deduced = {}/81 cells; {} eliminations proven.{}",
                report.givens,
                report.placements.len(),
                report.solved_cells(),
                report.eliminations,
                solved,
            )
        } else {
            String::from("These givens contradict — no solution (UNSAT).")
        };
        self.logic = Some(self.puzzle.project_deductions(&deductions));
        self.report = Some(report);
        self.overlay = Overlay::Logic;
    }

    /// Run the full BatSat solve.
    fn run_full(&mut self) {
        if let Some(grid) = solve(&self.puzzle) {
            self.full = Some(grid);
            self.status = String::from("Solved by full search (BatSat).");
        } else {
            self.full = None;
            self.status = String::from("These givens contradict — no solution (UNSAT).");
        }
        self.overlay = Overlay::Full;
    }

    /// The digit to show at `(r, c)` and where it came from, if any.
    fn cell_content(&self, r: usize, c: usize) -> Option<(u8, Source)> {
        if let Some(v) = self.puzzle.given(r, c) {
            return Some((v, Source::Given));
        }
        match self.overlay {
            Overlay::None => None,
            Overlay::Logic => self
                .logic
                .as_ref()
                .and_then(|g| g.get(r, c).value)
                .map(|v| (v, Source::Logic)),
            Overlay::Full => self
                .full
                .as_ref()
                .and_then(|g| g.get(r, c).value)
                .map(|v| (v, Source::Full)),
        }
    }

    /// Handle digit / delete / arrow keys for the selected cell.
    fn handle_keys(&mut self, ctx: &egui::Context) {
        let action = ctx.input(|i| {
            if i.key_pressed(egui::Key::Backspace)
                || i.key_pressed(egui::Key::Delete)
                || i.key_pressed(egui::Key::Num0)
            {
                return Some(KeyAction::Clear);
            }
            for d in 1..=9u8 {
                if i.key_pressed(digit_key(d)) {
                    return Some(KeyAction::Set(d));
                }
            }
            for (key, dr, dc) in [
                (egui::Key::ArrowUp, -1, 0),
                (egui::Key::ArrowDown, 1, 0),
                (egui::Key::ArrowLeft, 0, -1),
                (egui::Key::ArrowRight, 0, 1),
            ] {
                if i.key_pressed(key) {
                    return Some(KeyAction::Move(dr, dc));
                }
            }
            None
        });
        match action {
            Some(KeyAction::Set(d)) => self.edit_selected(Some(d)),
            Some(KeyAction::Clear) => self.edit_selected(None),
            Some(KeyAction::Move(dr, dc)) => self.move_selection(dr, dc),
            None => {}
        }
    }

    /// Move the selection by `(dr, dc)` (each in `-1..=1`), clamped to the board.
    fn move_selection(&mut self, dr: i32, dc: i32) {
        let (r, c) = self.selected.unwrap_or((0, 0));
        self.selected = Some((step_index(r, dr), step_index(c, dc)));
    }

    /// Draw the 9×9 grid and handle clicks on it.
    fn draw_grid(&mut self, ui: &mut egui::Ui) {
        let visuals = ui.visuals();
        let bg = visuals.extreme_bg_color;
        let given_color = visuals.strong_text_color();
        let full_color = visuals.weak_text_color();
        let logic_color = egui::Color32::from_rgb(80, 160, 255);
        let sel_color = visuals.selection.bg_fill;
        let thin = egui::Stroke::new(1.0, visuals.weak_text_color());
        let thick = egui::Stroke::new(2.5, visuals.strong_text_color());

        let side = ui
            .available_width()
            .min(ui.available_height())
            .clamp(240.0, 560.0);
        let (response, painter) = ui.allocate_painter(egui::vec2(side, side), egui::Sense::click());
        let rect = response.rect;
        let origin = rect.min;
        let cell = side / 9.0;

        painter.rect_filled(rect, 4.0, bg);

        if let Some((sr, sc)) = self.selected {
            let min = origin + egui::vec2(sc as f32 * cell, sr as f32 * cell);
            let cell_rect = egui::Rect::from_min_size(min, egui::vec2(cell, cell));
            painter.rect_filled(cell_rect, 0.0, sel_color);
        }

        for r in 0..9 {
            for c in 0..9 {
                if let Some((digit, source)) = self.cell_content(r, c) {
                    let color = match source {
                        Source::Given => given_color,
                        Source::Logic => logic_color,
                        Source::Full => full_color,
                    };
                    let center =
                        origin + egui::vec2((c as f32 + 0.5) * cell, (r as f32 + 0.5) * cell);
                    painter.text(
                        center,
                        egui::Align2::CENTER_CENTER,
                        digit.to_string(),
                        egui::FontId::proportional(cell * 0.58),
                        color,
                    );
                }
            }
        }

        for i in 0..=9 {
            let stroke = if i % 3 == 0 { thick } else { thin };
            let offset = i as f32 * cell;
            painter.line_segment(
                [
                    egui::pos2(origin.x + offset, rect.min.y),
                    egui::pos2(origin.x + offset, rect.max.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(rect.min.x, origin.y + offset),
                    egui::pos2(rect.max.x, origin.y + offset),
                ],
                stroke,
            );
        }

        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let c = ((pos.x - origin.x) / cell).floor() as i32;
                let r = ((pos.y - origin.y) / cell).floor() as i32;
                if (0..9).contains(&r) && (0..9).contains(&c) {
                    self.selected = Some((r as usize, c as usize));
                }
            }
        }
    }
}

/// A resolved key press for the selected cell.
enum KeyAction {
    Set(u8),
    Clear,
    Move(i32, i32),
}

/// Step a `0..=8` board index by a unit `delta` (`-1..=1`), clamped to the board.
fn step_index(v: usize, delta: i32) -> usize {
    match delta.cmp(&0) {
        std::cmp::Ordering::Less => v.saturating_sub(1),
        std::cmp::Ordering::Greater => (v + 1).min(8),
        std::cmp::Ordering::Equal => v,
    }
}

/// The egui key for digit `d` (1–9).
fn digit_key(d: u8) -> egui::Key {
    match d {
        1 => egui::Key::Num1,
        2 => egui::Key::Num2,
        3 => egui::Key::Num3,
        4 => egui::Key::Num4,
        5 => egui::Key::Num5,
        6 => egui::Key::Num6,
        7 => egui::Key::Num7,
        8 => egui::Key::Num8,
        _ => egui::Key::Num9,
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_keys(ctx);

        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("SatSight");
                ui.separator();
                if ui.button("Deduce (logic)").clicked() {
                    self.run_logic();
                }
                if ui.button("Full solve").clicked() {
                    self.run_full();
                }
                if ui.button("Clear marks").clicked() {
                    self.overlay = Overlay::None;
                }
                ui.separator();
                if ui.button("Sample").clicked() {
                    self.load(Sudoku::easy_sample());
                }
                if ui.button("Empty").clicked() {
                    self.load(Sudoku::empty());
                }
            });
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                color_key(ui, egui::Color32::from_rgb(80, 160, 255), "logic-proven");
                color_key(ui, ui.visuals().weak_text_color(), "search-filled");
                color_key(ui, ui.visuals().strong_text_color(), "given");
            });
            ui.add_space(2.0);
            ui.label(&self.status);
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            ui.vertical_centered(|ui| {
                self.draw_grid(ui);
            });
        });
    }
}

/// A small coloured swatch + label, for the legend.
fn color_key(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
    ui.label(label);
}
