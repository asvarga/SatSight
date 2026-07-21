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
//!   backtrack, with live candidate marks — the bidirectional thesis in
//!   motion. Placements and eliminations forced by the givens alone (proven) are
//!   drawn solid; those contingent on a search guess (hypothetical) are drawn
//!   faded and struck, so known facts read apart from tentative ones (plan §1). A
//!   speed slider runs N steps/frame.
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

mod akari_view;
mod coloring_view;
mod stepper;

use eframe::egui;
use satsight_core::{clause, BatSatBackend, Cdcl, Cnf, Lit, Registry, SolveOutcome, Solver};
use satsight_puzzles::sudoku::{Cell, LogicReport, Sudoku, SudokuCell};
use satsight_puzzles::{backbone, deduce, Grid, Puzzle};

use akari_view::AkariView;
use coloring_view::{ColoringView, BACKBONE_COLOR};
use stepper::{Certainty, Emphasis, Stepper};

/// Box-confined candidates: digits propagation has cornered into a few cells of a
/// 3×3 box, highlighted in their own slot rather than drawn in the cell's corners.
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

/// Which puzzle the demo is showing. Sudoku is the primary, rich view; graph
/// coloring (plan §7) proves the same abstractions drive a non-grid puzzle, and
/// Akari (Light Up) proves they drive a *cardinality* puzzle — at-most-k / exactly-k
/// numbered walls beyond exactly-one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PuzzleKind {
    Sudoku,
    Coloring,
    Akari,
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
    /// Cells forced across *every* solution (the backbone, plan §1).
    Backbone,
    /// The live state of the stepped CDCL search.
    Step,
}

/// Where a displayed digit came from — drives its colour.
enum Source {
    Given,
    Logic,
    Full,
    /// A cell forced in every solution consistent with the givens (the backbone).
    Backbone,
    /// Placed by the stepped search and entailed by the givens alone — a proven
    /// fact (plan §1), drawn solid.
    SearchProven,
    /// Placed by the stepped search but contingent on a branching guess — a
    /// hypothetical fact, drawn faded so it reads as tentative.
    SearchGuess,
}

/// The application state. Rules are encoded once (they never change); solver
/// outputs are cached and invalidated on edit.
struct App {
    /// Which puzzle is on screen. The Sudoku state below is always live; the
    /// coloring view keeps its own self-contained state.
    kind: PuzzleKind,
    /// The second-puzzle view (graph coloring), driven by the same generic maps.
    coloring: ColoringView,
    /// The cardinality-puzzle view (Akari / Light Up), same generic maps again.
    akari: AkariView,

    puzzle: Sudoku,
    selected: Option<(usize, usize)>,
    /// The forward encoding, built once: the registry (bridge) and a CDCL holding
    /// the rule CNF. Editing only changes the assumptions derived from `puzzle`.
    reg: Registry<Cell>,
    cdcl: Cdcl,
    /// The fast backend for "Full solve" (plan §5).
    batsat: BatSatBackend,
    /// The working SAT problem the solvers run on: the immutable `rules` plus any
    /// blocking clauses added by "Block solution". `cdcl` and `batsat` are rebuilt
    /// from it whenever it changes.
    cnf: Cnf,
    /// The pristine rule CNF, kept so an edit can reset `cnf` to rules-only and
    /// drop every blocking clause (they name a solution of the *old* board).
    rules: Cnf,
    /// How many distinct solutions have been blocked out (negated into `cnf`). Zero
    /// unless the user has pressed "Block solution"; drives the uniqueness verdict.
    blocked: usize,

    overlay: Overlay,
    /// Givens + logic-proven placements, or `None` until "Deduce" is pressed.
    logic: Option<Grid<SudokuCell>>,
    report: Option<LogicReport>,
    /// The full solution, or `None` until "Full solve" is pressed.
    full: Option<Grid<SudokuCell>>,
    /// Givens + cells forced across every solution, or `None` until "Backbone".
    backbone: Option<Grid<SudokuCell>>,
    /// The live stepped search, or `None` until "Step"/"Play" is pressed.
    stepper: Option<Stepper<Cell>>,
    /// Given cells named by an UNSAT core, to flag in red (plan §4).
    core: Vec<(usize, usize)>,
    /// Steps advanced per frame while playing.
    speed: u32,
    status: String,
    /// Whether the "About / legend" help window is open. Starts open so a
    /// newcomer meets the bidirectional idea and the mark/colour key up front.
    show_help: bool,
}

impl App {
    fn new() -> Self {
        // The rules depend only on the puzzle *kind*, not the givens, so encode
        // an empty board once and reuse the registry + CNF for the whole session.
        let mut reg = Registry::new();
        let mut cnf = Cnf::new();
        Sudoku::empty().encode_rules(&mut reg, &mut cnf);
        let rules = cnf.clone();
        let cdcl = Cdcl::from_cnf(&cnf);
        let mut batsat = BatSatBackend::new();
        batsat.load_rules(&cnf);

        Self {
            kind: PuzzleKind::Sudoku,
            coloring: ColoringView::new(),
            akari: AkariView::new(),
            puzzle: Sudoku::hard_sample(),
            selected: None,
            reg,
            cdcl,
            batsat,
            cnf,
            rules,
            blocked: 0,
            overlay: Overlay::None,
            logic: None,
            report: None,
            full: None,
            backbone: None,
            stepper: None,
            core: Vec::new(),
            speed: 8,
            status: "Click a cell and type 1–9 to edit. Then Deduce, Full solve, or Step."
                .to_owned(),
            show_help: true,
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
        self.backbone = None;
        self.stepper = None;
        self.core.clear();
        // Blocking clauses name a full solution of the board that just changed, so
        // they no longer mean anything — reset the SAT problem to the pure rules.
        if self.blocked > 0 {
            self.cnf = self.rules.clone();
            self.rebuild_solvers();
            self.blocked = 0;
        }
    }

    /// Rebuild the CDCL and BatSat solvers from the current working `cnf`, after a
    /// blocking clause is added or removed. Cheap (the rule CNF is small) and only
    /// happens on a button press or an edit, never per frame.
    fn rebuild_solvers(&mut self) {
        self.cdcl = Cdcl::from_cnf(&self.cnf);
        self.batsat = BatSatBackend::new();
        self.batsat.load_rules(&self.cnf);
    }

    /// Add the negation of the fully-solved board to the SAT problem, so the next
    /// search must find a *different* solution — or prove there is none, which
    /// means the board is unique. A no-op (with a nudge) unless the stepper has
    /// actually reached a full solution.
    ///
    /// This is Option 1 of the uniqueness story: the board is unique iff, after
    /// blocking the solution just found, re-solving is UNSAT. The user drives it:
    /// Play to a solution, Block it, Play again.
    fn block_current_solution(&mut self) {
        if !self.stepper.as_ref().is_some_and(Stepper::is_solved) {
            self.status = String::from("Play (or Step) to a full solution first, then block it.");
            return;
        }
        // Read the placed digits out before mutating self (ends the stepper borrow).
        let placements: Vec<(usize, usize, u8)> = {
            let st = self.stepper.as_ref().expect("is_solved implies a stepper");
            (0..9)
                .flat_map(|r| (0..9).map(move |c| (r, c)))
                .filter_map(|(r, c)| st.placed(r, c).map(|v| (r, c, v)))
                .collect()
        };
        // The no-good: ¬(all these placements hold) = the OR of their negations.
        // Any different full solution flips at least one placed cell, satisfying it.
        let lits: Vec<Lit> = placements
            .into_iter()
            .filter_map(|(r, c, v)| {
                self.reg
                    .get(&Cell { r, c, v })
                    .map(satsight_core::Var::neg_lit)
            })
            .collect();
        self.cnf.add_clause(clause(lits));
        self.blocked += 1;
        self.rebuild_solvers();
        // Drop the finished search; the board falls back to just the givens until
        // the next Play launches a fresh search against the augmented problem.
        self.stepper = None;
        self.status = format!(
            "Blocked solution #{}. Play again — another solution means the board isn't \
             unique; UNSAT (no solution) means it is.",
            self.blocked
        );
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

    /// Run the backbone: cells forced across *every* solution (plan §1).
    ///
    /// On a uniquely solvable board this is the whole solution; on an
    /// under-constrained one it is exactly the cells that never vary — the
    /// interesting case, which the status line points the user toward.
    fn run_backbone(&mut self) {
        let bb = backbone(&self.puzzle);
        if bb.satisfiable {
            let report = self.puzzle.logic_report_from(&bb);
            let solved = report.solved_cells();
            self.status = if solved == 81 {
                format!(
                    "Backbone = the whole board: these {} givens have a unique solution. \
                     Delete a given to reveal cells that vary between solutions.",
                    report.givens,
                )
            } else {
                format!(
                    "Backbone: {} of the open cells are forced in every solution; {} still vary. \
                     ({} eliminations forced across all solutions.)",
                    report.placements.len(),
                    81 - solved,
                    report.eliminations,
                )
            };
            self.backbone = Some(self.puzzle.project_deductions(&bb));
        } else {
            self.backbone = None;
            self.status =
                String::from("These givens contradict — no solutions, so no backbone (UNSAT).");
        }
        self.overlay = Overlay::Backbone;
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
            Overlay::Backbone => self
                .backbone
                .as_ref()
                .and_then(|g| g.get(r, c).value)
                .map(|v| (v, Source::Backbone)),
            Overlay::Step => {
                self.stepper
                    .as_ref()
                    .and_then(|st| st.placement(r, c))
                    .map(|(v, certainty)| {
                        let source = match certainty {
                            Certainty::Proven => Source::SearchProven,
                            Certainty::Hypothetical => Source::SearchGuess,
                        };
                        (v, source)
                    })
            }
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
        // A guess-contingent placement is the same hue, faded, so proven vs
        // hypothetical reads at a glance like pen vs pencil (plan §1).
        let guess_color = search_color.gamma_multiply(0.5);
        let candidate_color = visuals.weak_text_color().gamma_multiply(0.75);
        let sel_color = visuals.selection.bg_fill;
        let core_color = egui::Color32::from_rgb(200, 70, 70).gamma_multiply(0.30);
        let emphasis_color = egui::Color32::from_rgb(240, 190, 90);
        let thin = egui::Stroke::new(1.0_f32, visuals.weak_text_color());
        let thick = egui::Stroke::new(2.5_f32, visuals.strong_text_color());

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
                            egui::Stroke::new(2.5_f32, emphasis_color),
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
                        Source::Backbone => BACKBONE_COLOR,
                        Source::SearchProven => search_color,
                        Source::SearchGuess => guess_color,
                    };
                    painter.text(
                        cell_rect(r, c).center(),
                        egui::Align2::CENTER_CENTER,
                        digit.to_string(),
                        egui::FontId::proportional(cell * 0.58),
                        color,
                    );
                } else if self.overlay == Overlay::Step {
                    // No value yet: show the surviving candidates as center marks,
                    // plus corner marks for any digit propagation has confined to a
                    // few cells of the box.
                    if let Some(st) = &self.stepper {
                        draw_candidates(
                            &painter,
                            cell_rect(r, c),
                            cell,
                            &CandidateMarks {
                                candidates: st.candidates(r, c),
                                corner: st.corner_marks(r, c),
                                guess_eliminated: st.hypo_eliminated(r, c),
                                color: candidate_color,
                                corner_color: CORNER_MARK_COLOR,
                                guess_color,
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
            if ui.button("Deduce (logic)").clicked() {
                self.run_logic();
            }
            if ui.button("Full solve").clicked() {
                self.run_full();
            }
            if ui
                .button("Backbone")
                .on_hover_text(
                    "Cells forced across every solution consistent with the givens \
                     (plan §1). Most telling on an ambiguous board — delete a given first.",
                )
                .clicked()
            {
                self.run_backbone();
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
            let solved = self.stepper.as_ref().is_some_and(Stepper::is_solved);
            if ui
                .add_enabled(solved, egui::Button::new("Block solution ✂"))
                .on_hover_text(
                    "Add ¬(this board) to the SAT problem, then Play again: another \
                     solution means the board isn't unique; UNSAT means it is.",
                )
                .clicked()
            {
                self.block_current_solution();
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
                let search = egui::Color32::from_rgb(120, 200, 120);
                color_key(ui, search, "search-proven");
                color_key(ui, ui.visuals().weak_text_color(), "full-solve");
                color_key(ui, BACKBONE_COLOR, "backbone");
                color_key(ui, ui.visuals().strong_text_color(), "given");
                if self.overlay == Overlay::Step {
                    color_key(ui, search.gamma_multiply(0.5), "guess (hypothetical)");
                    color_key(ui, CORNER_MARK_COLOR, "box-confined candidate");
                }
            });
            ui.add_space(2.0);
            // In Step mode the status tracks the live event; otherwise the last
            // action's summary.
            let line = match (self.overlay, &self.stepper) {
                // After blocking, a finished search delivers the uniqueness verdict
                // rather than the raw event (whose "these givens have no solution"
                // would misread — the givens do have one; there's just no *other*).
                (Overlay::Step, Some(st)) if self.blocked > 0 && st.is_done() => {
                    if st.is_solved() {
                        format!(
                            "A distinct solution #{} — the board is NOT unique. Block it \
                             too, then Play, to look for more.",
                            self.blocked + 1
                        )
                    } else if self.blocked == 1 {
                        "No other solution exists — the board is unique. \u{2713}".to_owned()
                    } else {
                        format!(
                            "No further solutions — the board has exactly {} of them.",
                            self.blocked
                        )
                    }
                }
                (Overlay::Step, Some(st)) => st.description(),
                _ => self.status.clone(),
            };
            ui.label(line);
            ui.add_space(4.0);
        });
    }

    /// A floating "About / legend" window explaining the bidirectional reduction
    /// this demo visualizes and what every colour and pencil mark means. Opened
    /// by the "Help ?" button (and on first launch); closable and movable.
    fn draw_help(&mut self, ctx: &egui::Context) {
        // The window borrows its own open flag, so copy it out and store it back.
        let mut open = self.show_help;
        // Open into the central area, below the top control bars, so the window's
        // default first-launch position never covers the puzzle's buttons. This
        // is only the *default* position — the user can drag it anywhere after.
        // `draw_help` runs after the active view lays out its panels, so
        // `available_rect` is the grid region left between the top bars and the
        // bottom status bar.
        let corner = ctx.available_rect().min + egui::vec2(8.0, 8.0);
        egui::Window::new("About SatSight — the bidirectional reduction")
            .open(&mut open)
            .collapsible(true)
            .resizable(true)
            .default_width(460.0)
            .default_height(560.0)
            .default_pos(corner)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, help_body);
            });
        self.show_help = open;
    }
}

/// The body of the help window: the thesis, the three views, and the full key to
/// every colour and pencil mark on the grid.
fn help_body(ui: &mut egui::Ui) {
    help_thesis(ui);
    help_views(ui);
    help_legend(ui);
    help_editing(ui);
}

/// The opening "one bridge, two directions" explanation of the reduction.
fn help_thesis(ui: &mut egui::Ui) {
    section(ui, "One bridge, two directions");
    ui.label(
        "SatSight solves a puzzle by translating it into Boolean SAT and reading \
         the solver's discoveries back — both directions crossing the same bridge: \
         a variable registry that is a bijection between puzzle facts (\u{201c}cell \
         r,c holds the digit v\u{201d}) and SAT variables.",
    );
    ui.add_space(4.0);
    ui.label(
        "Forward (encoding): the puzzle's rules become fixed CNF, and your givens \
         enter as assumptions rather than clauses. So editing a clue is cheap \
         (only the assumptions change, never the rules), and a contradiction points \
         straight back at the offending clues.",
    );
    ui.add_space(4.0);
    ui.label(
        "Backward (interpreting): whatever the solver finds \u{2014} forced cells, \
         surviving candidates, box confinements, the unsatisfiable core \u{2014} \
         is decoded through that same registry and painted on the grid in the \
         puzzle's own language. That round trip is the \u{201c}bidirectional \
         reduction\u{201d} this demo is about.",
    );
}

/// The Deduce / Full solve / Step view descriptions.
fn help_views(ui: &mut egui::Ui) {
    section(ui, "The three views");
    bullet(
        ui,
        "Deduce (logic)",
        "The sound, search-free backward map \u{2014} unit propagation plus \
         failed-literal probing. It paints only what it can prove from the givens \
         alone. If it fills the whole board, no search was needed.",
    );
    bullet(
        ui,
        "Full solve",
        "Runs the fast BatSat backend to completion and fills every remaining cell. \
         If the givens contradict, no board is drawn \u{2014} instead the conflicting \
         clues (the UNSAT core) are flagged.",
    );
    bullet(
        ui,
        "Backbone",
        "Fills only the cells that hold the same value in \u{2014} every \u{2014} solution \
         consistent with the givens (plan \u{00a7}1's \u{201c}facts across all \
         solutions\u{201d}). A uniquely solvable board's backbone is its whole \
         solution; on an ambiguous board it is exactly the cells that never vary, so \
         delete a given and press it again to see which cells stay pinned.",
    );
    bullet(
        ui,
        "Step",
        "Drives the hand-written CDCL search one event at a time, so you can watch it \
         guess, propagate forced cells, hit conflicts, learn, and backtrack. This is \
         where the pencil marks and the proven-vs-hypothetical distinction come alive.",
    );
    bullet(
        ui,
        "Block solution",
        "The search stops at the first solution it finds \u{2014} it doesn't prove that \
         solution is the only one. To check: Play to a full solution, press Block \
         solution (which adds \u{00ac}(this board) to the SAT problem), then Play again. \
         A second solution means the puzzle is ambiguous; UNSAT means it was unique. \
         Editing any given clears the blocks.",
    );
}

/// The colour/mark key: placed digits, pencil marks, and highlights.
fn help_legend(ui: &mut egui::Ui) {
    let visuals = ui.visuals();
    let given = visuals.strong_text_color();
    let full = visuals.weak_text_color();
    let center = full.gamma_multiply(0.75);
    let logic = egui::Color32::from_rgb(80, 160, 255);
    let search = egui::Color32::from_rgb(120, 200, 120);
    let guess = search.gamma_multiply(0.5);
    let emphasis = egui::Color32::from_rgb(240, 190, 90);
    let core = egui::Color32::from_rgb(200, 70, 70);

    section(ui, "Digits (placed values)");
    legend(ui, given, "given", "the clues you entered.");
    legend(
        ui,
        logic,
        "logic-proven",
        "a placement the Deduce map proved from the givens.",
    );
    legend(
        ui,
        search,
        "search-proven",
        "in Step, a cell forced by the givens alone \u{2014} true in every solution \
         consistent with the clues. Drawn solid, like pen.",
    );
    legend(
        ui,
        guess,
        "guess (hypothetical)",
        "in Step, a cell placed only under the current branching guess \u{2014} it may \
         be undone on backtrack. Drawn faded, like pencil.",
    );
    legend(
        ui,
        full,
        "full-solve",
        "a cell filled in by the complete BatSat solution.",
    );
    legend(
        ui,
        BACKBONE_COLOR,
        "backbone",
        "a cell that takes the same value in every solution consistent with the \
         givens \u{2014} a fact that holds across all solutions, from the Backbone view.",
    );

    section(ui, "Pencil marks (Step view)");
    ui.label(
        "Every candidate digit keeps one fixed home in a 3\u{00d7}3 (1 top-left \
         \u{2026} 9 bottom-right), so a digit never moves \u{2014} its colour and \
         style alone tell its status, and no two marks ever overlap.",
    );
    ui.add_space(4.0);
    legend(
        ui,
        center,
        "candidate",
        "a digit still Boolean-possible in an unsolved cell (a propagation \
         survivor). The full set appears once the cell narrows to a few.",
    );
    legend(
        ui,
        CORNER_MARK_COLOR,
        "box-confined",
        "a candidate propagation has confined to just a few cells of a 3\u{00d7}3 box \
         \u{2014} \u{201c}the 7 goes in one of these cells\u{201d} (a hidden \
         pair/triple footprint). Highlighted in its own slot so the footprint reads \
         across the box; a digit pinned to a single cell is a hidden single and gets \
         placed instead.",
    );
    legend(
        ui,
        guess,
        "struck candidate",
        "a candidate the current guess rules out \u{2014} a hypothetical elimination, \
         drawn struck through in its slot. A proven elimination is simply absent: \
         known non-facts aren't drawn.",
    );

    section(ui, "Highlights");
    legend(
        ui,
        emphasis,
        "amber outline",
        "the cell(s) the last Step event touched \u{2014} a decision, a propagation, \
         or the cells of a conflict clause.",
    );
    legend(
        ui,
        core,
        "red fill",
        "conflicting givens named by the UNSAT core, shown when the clues have no \
         solution.",
    );
}

/// The editing keys, closing the help window.
fn help_editing(ui: &mut egui::Ui) {
    section(ui, "Editing");
    ui.label(
        "Click a cell, then type 1\u{2013}9 to set a given, Backspace or 0 to clear, \
         and the arrow keys to move. Any edit drops the cached results \u{2014} press \
         Deduce, Full solve, or Step again.",
    );
    ui.add_space(6.0);
}

/// A bold section heading with a divider, for the help window.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(title).strong().size(15.0));
    ui.separator();
}

/// A view/term bullet: a bold lead-in followed by its description, wrapped.
fn bullet(ui: &mut egui::Ui, term: &str, desc: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new(format!("{term}:")).strong());
        ui.label(desc);
    });
    ui.add_space(4.0);
}

/// A legend row: a colour swatch, a bold term, then a wrapped description.
fn legend(ui: &mut egui::Ui, color: egui::Color32, term: &str, desc: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, color);
        ui.label(egui::RichText::new(format!("{term} \u{2014}")).strong());
        ui.label(desc);
    });
    ui.add_space(4.0);
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
/// palette to paint them with. Every flagged value shares one fixed slot in the
/// cell's 3×3; the three arrays are mutually exclusive per value, so exactly one
/// style ever lands in a slot.
struct CandidateMarks {
    /// Values still Boolean-possible in the cell (the surviving candidates).
    candidates: [bool; 9],
    /// Values propagation has confined to a few cells of the box — box-confined
    /// candidates (a subset of `candidates`), highlighted in place so the "goes in
    /// one of these cells" footprint reads across the box.
    corner: [bool; 9],
    /// Values the current guess rules out here — hypothetical eliminations, shown
    /// struck through so a contingent elimination reads apart from a proven one.
    guess_eliminated: [bool; 9],
    color: egui::Color32,
    corner_color: egui::Color32,
    guess_color: egui::Color32,
}

/// Draw a cell's pencil marks while the search runs (Step view). Every value has
/// one fixed home in a positional 3×3 (1 top-left … 9 bottom-right) and its
/// **style alone** tells its status, so marks never overlap or move (plan §8):
///
/// - a plain surviving candidate is drawn faint;
/// - a **box-confined** candidate — propagation has cornered the value into a few
///   cells of the 3×3 box ("the 7 goes in one of these") — keeps its slot but is
///   highlighted, so the footprint reads across the box;
/// - a value the current guess rules out is drawn struck through: a *hypothetical*
///   elimination (plan §1). A proven elimination is simply absent.
///
/// The full candidate grid appears once a cell narrows to ≤6 survivors; above that
/// only the box-confined hints show, keeping a busy cell legible.
fn draw_candidates(painter: &egui::Painter, rect: egui::Rect, cell: f32, marks: &CandidateMarks) {
    let sub = cell / 3.0;
    let slot = |i: usize| {
        let (sr, sc) = (i / 3, i % 3);
        rect.min + egui::vec2((sc as f32 + 0.5) * sub, (sr as f32 + 0.5) * sub)
    };
    let font = egui::FontId::proportional(sub * 0.62);
    // Show the full grid only once the cell has narrowed; above that the faint
    // candidates would just be clutter, so we keep only the box-confined hints.
    let show_all = marks.candidates.iter().filter(|&&b| b).count() <= 6;

    for i in 0..9 {
        let pos = slot(i);
        if marks.corner[i] {
            // Box-confined: highlight this digit's slot so the "v goes in one of
            // these cells" footprint reads across the box, then recolour it. Shown
            // even in a busy cell — it is the actionable hint.
            painter.rect_filled(
                egui::Rect::from_center_size(pos, egui::vec2(sub * 0.9, sub * 0.9)),
                2.0,
                marks.corner_color.gamma_multiply(0.20),
            );
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                (i + 1).to_string(),
                font.clone(),
                marks.corner_color,
            );
        } else if show_all && marks.candidates[i] {
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                (i + 1).to_string(),
                font.clone(),
                marks.color,
            );
        } else if show_all && marks.guess_eliminated[i] {
            // Ruled out by the current guess (not the givens): same slot, struck
            // through so it reads as contingent (plan §1).
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                (i + 1).to_string(),
                font.clone(),
                marks.guess_color,
            );
            let reach = sub * 0.3;
            painter.line_segment(
                [pos - egui::vec2(reach, 0.0), pos + egui::vec2(reach, 0.0)],
                egui::Stroke::new(1.0_f32, marks.guess_color),
            );
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

impl App {
    /// The Sudoku view: stepper pump, controls, grid, and status.
    fn update_sudoku(&mut self, ctx: &egui::Context) {
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

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            ui.vertical_centered(|ui| {
                self.draw_grid(ui);
            });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // A shared top bar carries the title, the puzzle switch, and the help
        // toggle; each puzzle then owns the rest of the frame. Switching kind
        // simply routes to a different view — the two hold independent state.
        egui::TopBottomPanel::top("kind").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("SatSight");
                ui.separator();
                ui.selectable_value(&mut self.kind, PuzzleKind::Sudoku, "Sudoku");
                ui.selectable_value(&mut self.kind, PuzzleKind::Coloring, "Graph coloring");
                ui.selectable_value(&mut self.kind, PuzzleKind::Akari, "Akari");
                ui.separator();
                if ui.button("Help ?").clicked() {
                    self.show_help = !self.show_help;
                }
            });
            ui.add_space(4.0);
        });

        match self.kind {
            PuzzleKind::Sudoku => self.update_sudoku(ctx),
            PuzzleKind::Coloring => self.coloring.ui(ctx),
            PuzzleKind::Akari => self.akari.ui(ctx),
        }

        // Draw the help window last so its default position can be measured
        // against the central grid region the active view leaves free.
        self.draw_help(ctx);
    }
}

/// A small coloured swatch + label, for the legend.
fn color_key(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
    ui.label(label);
}
