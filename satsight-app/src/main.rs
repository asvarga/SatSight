//! SatSight demo frontend — an eframe/egui app (plan §9).
//!
//! Renders the Sudoku grid, lets you edit givens (click a cell, type 1–9, or
//! backspace to clear), and visualizes both directions of the reduction across
//! three views:
//!
//! - **Deduce (logic)** runs the sound, search-free backward map — unit
//!   propagation + failed-literal probing — and paints every cell it can *prove*.
//! - **Full solve** runs the fast BatSat backend to completion and fills the rest;
//!   on contradictory givens it flags the conflicting clues (the UNSAT core).
//! - **Step** drives the observable hand-written CDCL one [`Event`] at a time
//!   (plan §6): watch it guess, propagate forced cells, hit conflicts, learn, and
//!   backtrack, with live corner-mark candidates — the bidirectional thesis in
//!   motion. A speed slider runs N steps/frame.
//!
//! Edits are cheap because givens are assumptions, not clauses (plan §4): changing
//! a clue just rebuilds the assumption vector and drops cached results.
//!
//! Pixel maths converts small grid indices to `f32`, so the usual cast lints are
//! allowed for this crate only; the core and puzzles crates stay strict.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

mod stepper;

use eframe::egui;
use satsight_core::{BatSatBackend, Cdcl, Cnf, Registry, SolveOutcome, Solver};
use satsight_puzzles::sudoku::{Cell, LogicReport, Sudoku, SudokuCell};
use satsight_puzzles::{deduce, Grid, Puzzle};

use stepper::{Emphasis, Stepper};

/// The tint on a center mark caught in a learned relationship (plan §8).
const LEARNED_MARK_COLOR: egui::Color32 = egui::Color32::from_rgb(210, 150, 90);
/// Corner marks: digits propagation has confined to a few cells of a 3×3 box.
const CORNER_MARK_COLOR: egui::Color32 = egui::Color32::from_rgb(168, 130, 224);

/// Native entry point: open a window (plan §9; wasm entry point below).
#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([560.0, 760.0])
            .with_min_inner_size([440.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SatSight",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}

/// Web entry point: mount the same app on a `<canvas>` via trunk/wasm-bindgen
/// (plan §9). The solver is a non-blocking `step()` pump precisely so it runs on
/// WASM's single thread.
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    eframe::WebLogger::init(log::LevelFilter::Debug).ok();
    let web_options = eframe::WebOptions::default();
    wasm_bindgen_futures::spawn_local(async {
        let canvas = web_sys::window()
            .expect("a browser window")
            .document()
            .expect("a document")
            .get_element_by_id("satsight_canvas")
            .expect("an element with id `satsight_canvas`")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("`satsight_canvas` is a <canvas>");
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|_cc| Ok(Box::new(App::new()))),
            )
            .await;
        if let Err(error) = result {
            log::error!("failed to start SatSight: {error:?}");
        }
    });
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
    /// The live state of the stepped CDCL search.
    Step,
}

/// Where a displayed digit came from — drives its colour.
enum Source {
    Given,
    Logic,
    Full,
    /// Placed by the stepped search (tentative mid-search value).
    Search,
}

/// The application state. Rules are encoded once (they never change); solver
/// outputs are cached and invalidated on edit.
struct App {
    puzzle: Sudoku,
    selected: Option<(usize, usize)>,
    /// The forward encoding, built once: the registry (bridge) and a CDCL holding
    /// the rule CNF. Editing only changes the assumptions derived from `puzzle`.
    reg: Registry<Cell>,
    cdcl: Cdcl,
    /// The fast backend for "Full solve" (plan §5).
    batsat: BatSatBackend,

    overlay: Overlay,
    /// Givens + logic-proven placements, or `None` until "Deduce" is pressed.
    logic: Option<Grid<SudokuCell>>,
    report: Option<LogicReport>,
    /// The full solution, or `None` until "Full solve" is pressed.
    full: Option<Grid<SudokuCell>>,
    /// The live stepped search, or `None` until "Step"/"Play" is pressed.
    stepper: Option<Stepper>,
    /// Given cells named by an UNSAT core, to flag in red (plan §4).
    core: Vec<(usize, usize)>,
    /// Steps advanced per frame while playing.
    speed: u32,
    status: String,
}

impl App {
    fn new() -> Self {
        // The rules depend only on the puzzle *kind*, not the givens, so encode
        // an empty board once and reuse the registry + CNF for the whole session.
        let mut reg = Registry::new();
        let mut cnf = Cnf::new();
        Sudoku::empty().encode_rules(&mut reg, &mut cnf);
        let cdcl = Cdcl::from_cnf(&cnf);
        let mut batsat = BatSatBackend::new();
        batsat.load_rules(&cnf);

        Self {
            puzzle: Sudoku::hard_sample(),
            selected: None,
            reg,
            cdcl,
            batsat,
            overlay: Overlay::None,
            logic: None,
            report: None,
            full: None,
            stepper: None,
            core: Vec::new(),
            speed: 8,
            status: "Click a cell and type 1–9 to edit. Then Deduce, Full solve, or Step."
                .to_owned(),
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
        self.stepper = None;
        self.core.clear();
    }

    /// Set or clear the given at the selected cell, invalidating results.
    fn edit_selected(&mut self, value: Option<u8>) {
        let Some((r, c)) = self.selected else { return };
        self.puzzle.set(r, c, value);
        self.invalidate();
        self.status = String::from("Edited — press Deduce, Full solve, or Step.");
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
        self.core.clear();
    }

    /// Run the full BatSat solve, flagging the conflicting givens on UNSAT.
    fn run_full(&mut self) {
        let assumptions = self.puzzle.assumptions(&self.reg);
        match self.batsat.solve(&assumptions) {
            SolveOutcome::Sat(model) => {
                let view = satsight_core::SolverView::from_model(&self.reg, &model);
                self.full = Some(self.puzzle.project(&view));
                self.core.clear();
                self.status = String::from("Solved by full search (BatSat).");
            }
            SolveOutcome::Unsat(core) => {
                self.full = None;
                self.core = self.decode_cells(&core);
                self.status =
                    String::from("These givens contradict — the flagged clues are the UNSAT core.");
            }
        }
        self.overlay = Overlay::Full;
    }

    /// Decode a list of literals to their (row, col) cells, sorted and de-duped.
    fn decode_cells(&self, lits: &[satsight_core::Lit]) -> Vec<(usize, usize)> {
        let mut cells: Vec<(usize, usize)> = lits
            .iter()
            .filter_map(|&lit| self.reg.decode(lit).map(|(cell, _)| (cell.r, cell.c)))
            .collect();
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    /// Begin (or restart) a stepped search over the current givens.
    fn start_stepper(&mut self) {
        let assumptions = self.puzzle.assumptions(&self.reg);
        self.stepper = Some(Stepper::new(&self.cdcl, self.reg.clone(), &assumptions));
        self.overlay = Overlay::Step;
        self.core.clear();
    }

    /// Advance the stepper by one event, syncing the UNSAT-core highlight.
    fn step_once(&mut self) {
        if self.stepper.is_none() {
            self.start_stepper();
        }
        if let Some(st) = &mut self.stepper {
            st.step();
        }
        self.sync_core();
    }

    /// Pull the current core cells out of the stepper (empty unless it hit UNSAT).
    fn sync_core(&mut self) {
        self.core = self
            .stepper
            .as_ref()
            .map(Stepper::core_cells)
            .unwrap_or_default();
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
            Overlay::Step => self
                .stepper
                .as_ref()
                .and_then(|st| st.placed(r, c))
                .map(|v| (v, Source::Search)),
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
        let search_color = egui::Color32::from_rgb(120, 200, 120);
        let candidate_color = visuals.weak_text_color().gamma_multiply(0.75);
        let sel_color = visuals.selection.bg_fill;
        let core_color = egui::Color32::from_rgb(200, 70, 70).gamma_multiply(0.30);
        let emphasis_color = egui::Color32::from_rgb(240, 190, 90);
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
        let cell_rect = |r: usize, c: usize| {
            let min = origin + egui::vec2(c as f32 * cell, r as f32 * cell);
            egui::Rect::from_min_size(min, egui::vec2(cell, cell))
        };

        painter.rect_filled(rect, 4.0, bg);

        // Conflicting givens (UNSAT core), flagged in red.
        for &(r, c) in &self.core {
            painter.rect_filled(cell_rect(r, c), 0.0, core_color);
        }

        // The cell the last step touched (decision/propagation/conflict).
        if self.overlay == Overlay::Step {
            if let Some(st) = &self.stepper {
                if let Emphasis::Cells(cells) = st.emphasis() {
                    for (r, c) in cells {
                        painter.rect_stroke(
                            cell_rect(r, c),
                            0.0,
                            egui::Stroke::new(2.5, emphasis_color),
                        );
                    }
                }
            }
        }

        if let Some((sr, sc)) = self.selected {
            painter.rect_filled(cell_rect(sr, sc), 0.0, sel_color);
        }

        for r in 0..9 {
            for c in 0..9 {
                if let Some((digit, source)) = self.cell_content(r, c) {
                    let color = match source {
                        Source::Given => given_color,
                        Source::Logic => logic_color,
                        Source::Full => full_color,
                        Source::Search => search_color,
                    };
                    painter.text(
                        cell_rect(r, c).center(),
                        egui::Align2::CENTER_CENTER,
                        digit.to_string(),
                        egui::FontId::proportional(cell * 0.58),
                        color,
                    );
                } else if self.overlay == Overlay::Step {
                    // No value yet: show the surviving candidates as center marks
                    // (tinting those caught in a learned relationship), plus corner
                    // marks for any digit propagation has confined to a few cells
                    // of the box.
                    if let Some(st) = &self.stepper {
                        draw_candidates(
                            &painter,
                            cell_rect(r, c),
                            cell,
                            &CandidateMarks {
                                candidates: st.candidates(r, c),
                                learned: st.learned_marks(r, c),
                                corner: st.corner_marks(r, c),
                                color: candidate_color,
                                learned_color: LEARNED_MARK_COLOR,
                                corner_color: CORNER_MARK_COLOR,
                            },
                        );
                    }
                }
            }
        }

        draw_grid_lines(&painter, rect, origin, cell, thin, thick);

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

    /// The top control bar: view buttons, stepping controls, board presets.
    fn draw_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("SatSight");
            ui.separator();
            if ui.button("Deduce (logic)").clicked() {
                self.run_logic();
            }
            if ui.button("Full solve").clicked() {
                self.run_full();
            }
            ui.separator();
            if ui.button("Hard sample").clicked() {
                self.load(Sudoku::hard_sample());
            }
            if ui.button("Easy sample").clicked() {
                self.load(Sudoku::easy_sample());
            }
            if ui.button("Empty").clicked() {
                self.load(Sudoku::empty());
            }
        });

        ui.horizontal(|ui| {
            ui.label("Step the CDCL:");
            if ui.button("Step ▶").clicked() {
                self.step_once();
            }
            let playing = self.stepper.as_ref().is_some_and(|st| st.playing);
            let done = self.stepper.as_ref().is_some_and(Stepper::is_done);
            if ui
                .button(if playing { "Pause ⏸" } else { "Play ⏵" })
                .clicked()
            {
                if self.stepper.is_none() || done {
                    self.start_stepper();
                }
                if let Some(st) = &mut self.stepper {
                    st.playing = !st.playing;
                }
            }
            if ui.button("Restart ⟲").clicked() {
                self.start_stepper();
            }
            ui.add(egui::Slider::new(&mut self.speed, 1..=200).text("steps/frame"));
        });
    }

    /// The bottom status bar: the colour legend plus the live/last status line.
    fn draw_status(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                color_key(ui, egui::Color32::from_rgb(80, 160, 255), "logic-proven");
                color_key(ui, egui::Color32::from_rgb(120, 200, 120), "search-placed");
                color_key(ui, ui.visuals().weak_text_color(), "full-solve");
                color_key(ui, ui.visuals().strong_text_color(), "given");
                if self.overlay == Overlay::Step {
                    color_key(ui, CORNER_MARK_COLOR, "corner mark (box-confined)");
                    color_key(ui, LEARNED_MARK_COLOR, "learned relationship");
                }
            });
            ui.add_space(2.0);
            // In Step mode the status tracks the live event; otherwise the last
            // action's summary.
            let line = match (self.overlay, &self.stepper) {
                (Overlay::Step, Some(st)) => st.description(),
                _ => self.status.clone(),
            };
            ui.label(line);
            ui.add_space(4.0);
        });
    }

    /// While stepping, a right-hand panel listing the short learned clauses
    /// decoded to puzzle terms (plan §9). Most CDCL clauses are noisy, so the
    /// stepper keeps only the readable ones.
    fn draw_learned_panel(&self, ctx: &egui::Context) {
        if self.overlay != Overlay::Step {
            return;
        }
        let learned = self
            .stepper
            .as_ref()
            .map(Stepper::learned_relationships)
            .unwrap_or_default();
        egui::SidePanel::right("learned")
            .resizable(false)
            .default_width(212.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.heading("Learned");
                ui.label("Short clauses the solver discovered, in the puzzle's language:");
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if learned.is_empty() {
                        ui.weak("(none yet — these appear when the search backtracks)");
                    } else {
                        for line in learned.iter().rev() {
                            ui.monospace(line);
                        }
                    }
                });
            });
    }
}

/// Draw the 9×9 grid lines, thick every third line to mark the boxes.
fn draw_grid_lines(
    painter: &egui::Painter,
    rect: egui::Rect,
    origin: egui::Pos2,
    cell: f32,
    thin: egui::Stroke,
    thick: egui::Stroke,
) {
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
}

/// A cell's pencil-mark state for the Step view: the per-value flags plus the
/// palette to paint them with.
struct CandidateMarks {
    /// Values still Boolean-possible in the cell (center marks).
    candidates: [bool; 9],
    /// Center marks caught in a learned relationship (tinted).
    learned: [bool; 9],
    /// Values propagation has confined to a few cells of the box (corner marks).
    corner: [bool; 9],
    color: egui::Color32,
    learned_color: egui::Color32,
    corner_color: egui::Color32,
}

/// Draw a cell's pencil marks while the search runs (Step view): the surviving
/// candidate digits laid out as a 3×3 of **center marks**, plus any **corner
/// marks** — digits propagation has confined to a few cells of the 3×3 box,
/// pinned to the cell's corners ("the 7 goes in one of these cells"). A center
/// mark caught in a learned relationship is tinted; a digit promoted to a corner
/// mark is drawn only in its corner, never twice.
fn draw_candidates(painter: &egui::Painter, rect: egui::Rect, cell: f32, marks: &CandidateMarks) {
    // Center marks: the full candidate list as a positional 3×3, but only once
    // the search has meaningfully narrowed the cell — and never a digit promoted
    // to a corner mark (drawn below instead of twice).
    if marks.candidates.iter().filter(|&&b| b).count() <= 6 {
        let sub = cell / 3.0;
        for (i, &alive) in marks.candidates.iter().enumerate() {
            if !alive || marks.corner[i] {
                continue;
            }
            let (sr, sc) = (i / 3, i % 3);
            let pos = rect.min + egui::vec2((sc as f32 + 0.5) * sub, (sr as f32 + 0.5) * sub);
            let digit_color = if marks.learned[i] {
                marks.learned_color
            } else {
                marks.color
            };
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                (i + 1).to_string(),
                egui::FontId::proportional(sub * 0.62),
                digit_color,
            );
        }
    }

    // Corner marks: box-confined digits, one faint chip per corner (up to four),
    // so they read even over the center marks behind them.
    let inset = cell * 0.17;
    let slots = [
        egui::vec2(inset, inset),
        egui::vec2(cell - inset, inset),
        egui::vec2(inset, cell - inset),
        egui::vec2(cell - inset, cell - inset),
    ];
    let chip = cell * 0.30;
    for (slot, digit) in marks
        .corner
        .iter()
        .enumerate()
        .filter_map(|(i, &m)| m.then_some(i))
        .take(slots.len())
        .enumerate()
    {
        let center = rect.min + slots[slot];
        painter.rect_filled(
            egui::Rect::from_center_size(center, egui::vec2(chip, chip)),
            2.0,
            marks.corner_color.gamma_multiply(0.22),
        );
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            (digit + 1).to_string(),
            egui::FontId::proportional(cell * 0.26),
            marks.corner_color,
        );
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

        // Advance a playing stepper N steps this frame, then keep the UI live.
        let speed = self.speed;
        let mut advanced = false;
        if let Some(st) = &mut self.stepper {
            if st.playing && !st.is_done() {
                for _ in 0..speed {
                    st.step();
                    if st.is_done() {
                        st.playing = false;
                        break;
                    }
                }
                advanced = true;
            }
        }
        if advanced {
            self.sync_core();
            ctx.request_repaint();
        }

        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.add_space(4.0);
            self.draw_controls(ui);
            ui.add_space(4.0);
        });

        self.draw_status(ctx);
        self.draw_learned_panel(ctx);

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
