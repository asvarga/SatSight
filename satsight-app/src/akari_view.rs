//! The Akari / Light Up view — the cardinality puzzle, in the demo (plan §7, issue #10).
//!
//! Akari is the second-puzzle the plan originally preferred because it needs
//! **at-most-k / exactly-k** cardinality constraints beyond exactly-one, so it
//! validates the reduction on a genuinely different constraint family. This view
//! makes that visible with the *same* generic machinery the other tabs use: it
//! renders a walled grid, lets you click a white cell to cycle a hand-placed lamp
//! (a given), and the Deduce / Full solve / Backbone / Step controls paint decoded
//! solver state straight onto the board in the puzzle's own language.
//!
//! Nothing here re-implements solving: [`deduce`], [`solve`](satsight_puzzles::solve),
//! [`backbone`] and the observable [`Stepper`] are all generic over the puzzle's
//! proposition type, so `Stepper<Lamp>` drives Akari exactly as `Stepper<Cell>`
//! drives Sudoku — the common generic path (issue #2), now exercising cardinality.

use eframe::egui;
use satsight_core::{BatSatBackend, Cdcl, Cnf, Event, Registry, SolveOutcome, Solver, SolverView};
use satsight_puzzles::akari::{Akari, AkariCell, Lamp};
use satsight_puzzles::{backbone, deduce, Grid, Puzzle};

use crate::coloring_view::BACKBONE_COLOR;
use crate::stepper::{solver_phase, Certainty, Stepper, READY};

/// Blue accent for a lamp a map *derived* by pure logic (Deduce).
const LOGIC_COLOR: egui::Color32 = egui::Color32::from_rgb(80, 160, 255);
/// Gray accent for a lamp from the full BatSat solve.
const FULL_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);
/// Green accent for a lamp the stepped search has placed under the givens alone —
/// a proven fact; matches the Sudoku / coloring Step views.
const SEARCH_COLOR: egui::Color32 = egui::Color32::from_rgb(120, 200, 120);
/// The cell the last Step event touched (decision / propagation / conflict).
const EMPHASIS_COLOR: egui::Color32 = egui::Color32::from_rgb(240, 190, 90);
/// Conflicting givens named by an UNSAT core (plan §4).
const CORE_COLOR: egui::Color32 = egui::Color32::from_rgb(200, 70, 70);
/// The warm wash marking a cell lit by some lamp.
const LIT_COLOR: egui::Color32 = egui::Color32::from_rgb(250, 224, 120);
/// A solid wall block (Akari walls are conventionally black); numbers sit on top.
const WALL_COLOR: egui::Color32 = egui::Color32::from_gray(70);
const WALL_TEXT_COLOR: egui::Color32 = egui::Color32::from_gray(235);

/// Which decoded artifact the board is painting over the givens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Logic,
    Full,
    Backbone,
    /// The live state of the stepped CDCL search.
    Step,
}

/// The Akari view: a fixed board, the forward encoding, and cached backward maps.
pub struct AkariView {
    puzzle: Akari,
    /// The forward encoding, built once: the registry (bridge) and the rule CNF.
    /// The board never changes, so only the assumptions (lamp givens) do (plan §4).
    reg: Registry<Lamp>,
    batsat: BatSatBackend,
    /// The observable CDCL over the rule CNF, spawning a [`Stepper`] per Step run.
    cdcl: Cdcl,
    overlay: Overlay,
    /// Lamps forced by pure logic (propagation + probing), or `None` until run.
    logic: Option<Grid<AkariCell>>,
    /// The full lighting from BatSat, or `None` until run / on UNSAT.
    full: Option<Grid<AkariCell>>,
    /// Lamps forced across *every* solution (the backbone), or `None` until run.
    backbone: Option<Grid<AkariCell>>,
    /// The live stepped search, or `None` until "Step"/"Play" is pressed.
    stepper: Option<Stepper<Lamp>>,
    /// White cells named by an UNSAT core, flagged in red (plan §4).
    core: Vec<(usize, usize)>,
    /// Steps advanced per frame while playing.
    speed: u32,
    status: String,
}

impl AkariView {
    /// A fresh view over the built-in sample board.
    #[must_use]
    pub fn new() -> Self {
        Self::from_puzzle(Akari::sample())
    }

    /// Encode a board's rules once (the walls never change, only the lamp givens)
    /// and load both solver backends.
    fn from_puzzle(puzzle: Akari) -> Self {
        let mut reg = Registry::new();
        let mut cnf = Cnf::new();
        puzzle.encode_rules(&mut reg, &mut cnf);
        let mut batsat = BatSatBackend::new();
        batsat.load_rules(&cnf);
        let cdcl = Cdcl::from_cnf(&cnf);
        Self {
            puzzle,
            reg,
            batsat,
            cdcl,
            overlay: Overlay::None,
            logic: None,
            full: None,
            backbone: None,
            stepper: None,
            core: Vec::new(),
            speed: 8,
            status: "Light Up: place lamps so every white cell is lit, no two lamps see each \
                     other, and each numbered wall touches exactly that many lamps. Click a cell \
                     to cycle lamp / empty, then Deduce, Full solve, Backbone, or Step."
                .to_owned(),
        }
    }

    /// Swap in a different board. Unlike an edit, the walls change, so the rule CNF
    /// and both solver backends must be rebuilt from scratch — [`from_puzzle`] does
    /// exactly that. The speed setting is carried across for continuity.
    fn load(&mut self, puzzle: Akari) {
        let speed = self.speed;
        *self = Self::from_puzzle(puzzle);
        self.speed = speed;
    }

    /// Drop cached results after an edit and stop overlaying them. The rules never
    /// change (only lamp givens, which are assumptions), so the solvers stand.
    fn invalidate(&mut self) {
        self.overlay = Overlay::None;
        self.logic = None;
        self.full = None;
        self.backbone = None;
        self.stepper = None;
        self.core.clear();
    }

    /// Cycle the given at white cell `(r, c)`: unmarked → lamp → empty → unmarked.
    fn cycle(&mut self, r: usize, c: usize) {
        if !self.puzzle.is_white(r, c) {
            return;
        }
        let next = match self.puzzle.given(r, c) {
            None => Some(true),
            Some(true) => Some(false),
            Some(false) => None,
        };
        self.puzzle.set_given(r, c, next);
        self.invalidate();
        self.status = String::from("Edited — press Deduce, Full solve, Backbone, or Step.");
    }

    /// Clear every lamp given.
    fn clear(&mut self) {
        self.puzzle.clear_givens();
        self.invalidate();
        self.status = String::from("Cleared. Click a white cell to place a lamp.");
    }

    /// Run the search-free backward map (propagation + probing).
    fn run_logic(&mut self) {
        let deductions = deduce(&self.puzzle);
        self.status = if deductions.satisfiable {
            let lamps = deductions.proven.iter().filter(|(_, h)| *h).count();
            let empty = deductions.proven.iter().filter(|(_, h)| !*h).count();
            format!(
                "Logic: {lamps} lamps forced and {empty} cells proven empty by propagation + \
                 probing — cardinality deductions (a 4-wall fills, a 0-wall clears) decoded back \
                 into the puzzle."
            )
        } else {
            String::from("These givens contradict — no valid lighting (UNSAT).")
        };
        self.logic = Some(self.puzzle.project_deductions(&deductions));
        self.overlay = Overlay::Logic;
        self.core.clear();
    }

    /// Run the full BatSat solve, flagging conflicting cells on UNSAT.
    fn run_full(&mut self) {
        let assumptions = self.puzzle.assumptions(&self.reg);
        match self.batsat.solve(&assumptions) {
            SolveOutcome::Sat(model) => {
                let view = SolverView::from_model(&self.reg, &model);
                self.full = Some(self.puzzle.project(&view));
                self.core.clear();
                self.status = String::from("A valid lighting, found by full search (BatSat).");
            }
            SolveOutcome::Unsat(core) => {
                self.full = None;
                self.core = self.decode_cells(&core);
                self.status =
                    String::from("These givens contradict — the flagged cells are the UNSAT core.");
            }
        }
        self.overlay = Overlay::Full;
    }

    /// Run the backbone: lamps forced across *every* valid lighting.
    fn run_backbone(&mut self) {
        let bb = backbone(&self.puzzle);
        if bb.satisfiable {
            let grid = self.puzzle.project_deductions(&bb);
            let decided = self.decided_white(&grid);
            let white = self.white_count();
            self.status = if decided == white {
                String::from(
                    "Backbone: every cell is forced — the board has a unique solution, so its \
                     backbone is the whole lighting.",
                )
            } else {
                format!(
                    "Backbone: {decided} of {white} white cells are forced in every solution; the \
                     rest vary between solutions."
                )
            };
            self.backbone = Some(grid);
        } else {
            self.backbone = None;
            self.status = String::from("These givens contradict — no solutions, so no backbone.");
        }
        self.overlay = Overlay::Backbone;
        self.core.clear();
    }

    /// Begin (or restart) a stepped search over the current lamp givens.
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
            .map(step_core_cells)
            .unwrap_or_default();
    }

    /// The stepper's last event, decoded into a one-line status in Akari's own
    /// vocabulary (the solver-mechanics moves come from the shared [`solver_phase`]).
    fn step_status(&self) -> String {
        let Some(st) = &self.stepper else {
            return READY.to_owned();
        };
        let Some(event) = st.last_event() else {
            return READY.to_owned();
        };
        if let Some(phase) = solver_phase(event) {
            return phase;
        }
        match event {
            // A decide/propagate literal may name an auxiliary variable of the
            // cardinality (exactly-k) encoding rather than a lamp — those decode to
            // `None`, and the search does branch and propagate on them, so report
            // them as internal counter bookkeeping instead of unwrapping.
            Event::Decide { lit } => match st.decode(*lit) {
                Some((lamp, holds)) => {
                    let what = if holds { "a lamp at" } else { "no lamp at" };
                    format!("Guess: {what} ({}, {})", lamp.r, lamp.c)
                }
                None => "Guess on an internal wall-count variable.".to_owned(),
            },
            Event::Propagate { lit, .. } => match st.decode(*lit) {
                Some((lamp, true)) => format!("Forced: a lamp at ({}, {})", lamp.r, lamp.c),
                Some((lamp, false)) => format!("Ruled out a lamp at ({}, {})", lamp.r, lamp.c),
                None => "Propagating an internal wall-count variable.".to_owned(),
            },
            Event::Sat => "Every cell lit — a valid lighting, found by search!".to_owned(),
            Event::Unsat { .. } => "These givens contradict — no valid lighting exists.".to_owned(),
            // Conflict / Backtrack / Learn are handled by `solver_phase` above.
            _ => unreachable!("solver_phase covers the remaining events"),
        }
    }

    /// Decode a list of literals to their white cells, sorted and de-duped.
    fn decode_cells(&self, lits: &[satsight_core::Lit]) -> Vec<(usize, usize)> {
        let mut cells: Vec<(usize, usize)> = lits
            .iter()
            .filter_map(|&lit| self.reg.decode(lit).map(|(lamp, _)| (lamp.r, lamp.c)))
            .collect();
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    /// The number of white cells on the board.
    fn white_count(&self) -> usize {
        (0..self.puzzle.rows())
            .flat_map(|r| (0..self.puzzle.cols()).map(move |c| (r, c)))
            .filter(|&(r, c)| self.puzzle.is_white(r, c))
            .count()
    }

    /// How many white cells a projected grid decides (a lamp or a proven empty).
    fn decided_white(&self, grid: &Grid<AkariCell>) -> usize {
        (0..self.puzzle.rows())
            .flat_map(|r| (0..self.puzzle.cols()).map(move |c| (r, c)))
            .filter(|&(r, c)| matches!(grid.get(r, c), AkariCell::White { lamp: Some(_), .. }))
            .count()
    }

    /// The grid backing the current non-step overlay. Step reads live state from
    /// the [`Stepper`] instead; None shows the givens alone.
    fn overlay_grid(&self) -> Option<&Grid<AkariCell>> {
        match self.overlay {
            Overlay::Logic => self.logic.as_ref(),
            Overlay::Full => self.full.as_ref(),
            Overlay::Backbone => self.backbone.as_ref(),
            Overlay::None | Overlay::Step => None,
        }
    }

    /// The top control bar: the backward-map buttons and the stepping controls.
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
                .on_hover_text("Lamps that hold in every valid lighting of the board.")
                .clicked()
            {
                self.run_backbone();
            }
            ui.separator();
            if ui.button("Clear lamps").clicked() {
                self.clear();
            }
            ui.separator();
            if ui
                .button("Easy sample")
                .on_hover_text("The 7\u{00d7}7 number-heavy board that pure logic solves outright.")
                .clicked()
            {
                self.load(Akari::sample());
            }
            if ui
                .button("Hard sample")
                .on_hover_text(
                    "A 10\u{00d7}10 board with few clues: logic fills part of it and stalls, \
                     so the rest needs search (Full solve / Step).",
                )
                .clicked()
            {
                self.load(Akari::hard_sample());
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

    /// Render the whole view: controls, the board, and the status/legend bar.
    pub fn ui(&mut self, ctx: &egui::Context) {
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

        egui::TopBottomPanel::top("akari_controls").show(ctx, |ui| {
            ui.add_space(4.0);
            self.draw_controls(ui);
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("akari_status").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let ruled_out = ui.visuals().weak_text_color();
                swatch(ui, LOGIC_COLOR, "logic-proven");
                swatch(ui, FULL_COLOR, "full-solve");
                swatch(ui, BACKBONE_COLOR, "backbone (all solutions)");
                swatch(ui, LIT_COLOR, "lit");
                x_swatch(ui, ruled_out, "ruled out (no lamp)");
                swatch(ui, CORE_COLOR, "UNSAT core");
                if self.overlay == Overlay::Step {
                    swatch(ui, SEARCH_COLOR, "search-proven");
                    swatch(ui, SEARCH_COLOR.gamma_multiply(0.5), "guess (hypothetical)");
                    swatch(ui, EMPHASIS_COLOR, "last step");
                }
            });
            ui.add_space(2.0);
            let line = if self.overlay == Overlay::Step {
                self.step_status()
            } else {
                self.status.clone()
            };
            ui.label(line);
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            ui.vertical_centered(|ui| self.draw_board(ui));
        });
    }

    /// The lamp state to draw at white cell `(r, c)` and the colour it reads in —
    /// resolved from the current overlay. `None` means "no lamp glyph here".
    fn lamp_paint(
        &self,
        r: usize,
        c: usize,
        grid: Option<&Grid<AkariCell>>,
    ) -> Option<egui::Color32> {
        // A hand-placed lamp given always reads in the strong "given" colour.
        if self.puzzle.given(r, c) == Some(true) && self.overlay != Overlay::Step {
            return Some(egui::Color32::from_gray(230));
        }
        if self.overlay == Overlay::Step {
            let st = self.stepper.as_ref()?;
            if st.value(&Lamp { r, c }) != Some(true) {
                return None;
            }
            return Some(match st.certainty(&Lamp { r, c }) {
                Certainty::Proven => SEARCH_COLOR,
                Certainty::Hypothetical => SEARCH_COLOR.gamma_multiply(0.5),
            });
        }
        let cell = grid?.get(r, c);
        if !matches!(
            cell,
            AkariCell::White {
                lamp: Some(true),
                ..
            }
        ) {
            return None;
        }
        Some(match self.overlay {
            Overlay::Backbone => BACKBONE_COLOR,
            Overlay::Full => FULL_COLOR,
            Overlay::Logic => LOGIC_COLOR,
            // No overlay: only givens are drawn, already handled above.
            Overlay::None | Overlay::Step => egui::Color32::from_gray(230),
        })
    }

    /// Whether white cell `(r, c)` reads as lit under the current overlay.
    fn is_lit(&self, r: usize, c: usize, grid: Option<&Grid<AkariCell>>) -> bool {
        if self.overlay == Overlay::Step {
            // Lit by any currently-placed lamp (proven or hypothetical) in its cross.
            return self.stepper.as_ref().is_some_and(|st| {
                self.puzzle
                    .cross_cells(r, c)
                    .into_iter()
                    .any(|(rr, cc)| st.value(&Lamp { r: rr, c: cc }) == Some(true))
            });
        }
        matches!(
            grid.map(|g| g.get(r, c)),
            Some(AkariCell::White { lit: true, .. })
        )
    }

    /// The ✕ colour to draw at white cell `(r, c)` if it is proven to hold **no**
    /// lamp under the current overlay — a user-marked empty, or a cell the logic map
    /// / full solve / backbone / search has ruled out; `None` if it isn't ruled out.
    /// These were invisible before (only *lamps* were drawn), which left the board
    /// looking near-empty even after a deduction; drawing a ✕ for every ruled-out cell
    /// shows how much a step actually decides. In Step, a merely hypothetical
    /// elimination (contingent on a guess) reads faded, like a hypothetical lamp.
    fn ruled_out_mark(
        &self,
        r: usize,
        c: usize,
        grid: Option<&Grid<AkariCell>>,
        base: egui::Color32,
    ) -> Option<egui::Color32> {
        if self.overlay == Overlay::Step {
            let st = self.stepper.as_ref()?;
            if st.value(&Lamp { r, c }) != Some(false) {
                return None;
            }
            return Some(match st.certainty(&Lamp { r, c }) {
                Certainty::Proven => base,
                Certainty::Hypothetical => base.gamma_multiply(0.5),
            });
        }
        matches!(
            grid.map(|g| g.get(r, c)),
            Some(AkariCell::White {
                lamp: Some(false),
                ..
            })
        )
        .then_some(base)
    }

    /// Draw the board and handle clicks (cycling a clicked white cell's given).
    fn draw_board(&mut self, ui: &mut egui::Ui) {
        let visuals = ui.visuals();
        let white_bg = visuals.extreme_bg_color;
        let empty_mark = visuals.weak_text_color();
        let grid_stroke = egui::Stroke::new(1.0_f32, visuals.weak_text_color());

        let (rows, cols) = (self.puzzle.rows(), self.puzzle.cols());
        let span = rows.max(cols) as f32;
        let side = ui
            .available_width()
            .min(ui.available_height())
            .clamp(240.0, 560.0);
        let cell = side / span;
        let board = egui::vec2(cols as f32 * cell, rows as f32 * cell);
        let (response, painter) = ui.allocate_painter(board, egui::Sense::click());
        let origin = response.rect.min;
        let cell_rect = |r: usize, c: usize| {
            let min = origin + egui::vec2(c as f32 * cell, r as f32 * cell);
            egui::Rect::from_min_size(min, egui::vec2(cell, cell))
        };

        // The grid the non-step overlays read from (None → givens only).
        let base;
        let grid: Option<&Grid<AkariCell>> = match self.overlay {
            Overlay::None => {
                base = self.puzzle.project_deductions(&empty_deductions());
                Some(&base)
            }
            Overlay::Step => None,
            _ => self.overlay_grid(),
        };

        // The cells the last Step event touched (amber outline).
        let emphasis: Vec<(usize, usize)> = if self.overlay == Overlay::Step {
            self.stepper
                .as_ref()
                .map(step_touched_cells)
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        for r in 0..rows {
            for c in 0..cols {
                let rect = cell_rect(r, c);
                if !self.puzzle.is_white(r, c) {
                    painter.rect_filled(rect, 0.0, WALL_COLOR);
                    if let satsight_puzzles::akari::Square::Wall(Some(k)) = self.puzzle.square(r, c)
                    {
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            k.to_string(),
                            egui::FontId::proportional(cell * 0.5),
                            WALL_TEXT_COLOR,
                        );
                    }
                    continue;
                }

                painter.rect_filled(rect, 0.0, white_bg);
                if self.is_lit(r, c, grid) {
                    painter.rect_filled(rect, 0.0, LIT_COLOR.gamma_multiply(0.35));
                }
                // Conflicting givens (UNSAT core) tint the cell red.
                if self.core.contains(&(r, c)) {
                    painter.rect_filled(rect, 0.0, CORE_COLOR.gamma_multiply(0.30));
                }

                if let Some(color) = self.lamp_paint(r, c, grid) {
                    draw_lamp(&painter, rect, color);
                } else if let Some(color) = self.ruled_out_mark(r, c, grid, empty_mark) {
                    // A cell proven to hold no lamp — a user-marked empty, or one the
                    // logic / solve / backbone / search ruled out: a small ✕.
                    draw_empty_mark(&painter, rect, color);
                }

                // The amber outline for the cell the last Step event touched.
                if emphasis.contains(&(r, c)) {
                    painter.rect_stroke(rect, 0.0, egui::Stroke::new(2.5_f32, EMPHASIS_COLOR));
                }
            }
        }

        // Grid lines over the whole board.
        for i in 0..=rows {
            let y = origin.y + i as f32 * cell;
            painter.line_segment(
                [egui::pos2(origin.x, y), egui::pos2(origin.x + board.x, y)],
                grid_stroke,
            );
        }
        for j in 0..=cols {
            let x = origin.x + j as f32 * cell;
            painter.line_segment(
                [egui::pos2(x, origin.y), egui::pos2(x, origin.y + board.y)],
                grid_stroke,
            );
        }

        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let col = ((pos.x - origin.x) / cell).floor();
                let row = ((pos.y - origin.y) / cell).floor();
                if row >= 0.0 && col >= 0.0 {
                    let (r, c) = (row as usize, col as usize);
                    if r < rows && c < cols {
                        self.cycle(r, c);
                    }
                }
            }
        }
    }
}

impl Default for AkariView {
    fn default() -> Self {
        Self::new()
    }
}

/// An empty deduction set — used to project the givens-only base grid.
fn empty_deductions() -> satsight_puzzles::Deductions<Lamp> {
    satsight_puzzles::Deductions {
        satisfiable: true,
        proven: Vec::new(),
    }
}

/// Draw a lamp (light bulb) glyph filling most of a cell: a bright disc with a
/// faint glow, so a lit lamp reads at a glance.
fn draw_lamp(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let center = rect.center();
    let radius = rect.width().min(rect.height()) * 0.30;
    painter.circle_filled(center, radius * 1.35, color.gamma_multiply(0.25));
    painter.circle_filled(center, radius, color);
}

/// Draw a small ✕ marking a cell proven to hold no lamp — whether the user pinned
/// it empty or a deduction / solve / search ruled it out.
fn draw_empty_mark(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let center = rect.center();
    let reach = rect.width().min(rect.height()) * 0.17;
    let stroke = egui::Stroke::new(1.5_f32, color);
    painter.line_segment(
        [
            center - egui::vec2(reach, reach),
            center + egui::vec2(reach, reach),
        ],
        stroke,
    );
    painter.line_segment(
        [
            center - egui::vec2(reach, -reach),
            center + egui::vec2(reach, -reach),
        ],
        stroke,
    );
}

/// A small coloured swatch + label, for the status legend.
fn swatch(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
    ui.label(label);
}

/// A small ✕ mark + label, matching the "ruled out" glyph drawn on the board.
fn x_swatch(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    let center = rect.center();
    let reach = 4.0;
    let stroke = egui::Stroke::new(1.5_f32, color);
    ui.painter().line_segment(
        [
            center - egui::vec2(reach, reach),
            center + egui::vec2(reach, reach),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            center - egui::vec2(reach, -reach),
            center + egui::vec2(reach, -reach),
        ],
        stroke,
    );
    ui.label(label);
}

/// The white cells the stepper's last event touched, sorted and de-duped — the
/// amber "last step" outline.
fn step_touched_cells(st: &Stepper<Lamp>) -> Vec<(usize, usize)> {
    let mut cells: Vec<(usize, usize)> = st.touched_props().iter().map(|l| (l.r, l.c)).collect();
    cells.sort_unstable();
    cells.dedup();
    cells
}

/// The white cells an UNSAT core names, sorted and de-duped — the red core flag.
fn step_core_cells(st: &Stepper<Lamp>) -> Vec<(usize, usize)> {
    let mut cells: Vec<(usize, usize)> = st.core_props().iter().map(|l| (l.r, l.c)).collect();
    cells.sort_unstable();
    cells.dedup();
    cells
}

#[cfg(test)]
mod tests {
    use super::AkariView;
    use crate::stepper::Stepper;

    #[test]
    fn stepping_the_sample_never_panics_on_counter_variables() {
        // Regression: the exactly-k wall encoding introduces auxiliary counter
        // variables, and the CDCL both branches on and propagates them. Those name
        // no lamp, so decoding them yields `None`; `step_status` must render that
        // gracefully rather than unwrap and crash — the panic that killed the Akari
        // Step view on the first click. Driving a full stepped solve exercises the
        // same per-frame decode path the UI runs.
        let mut view = AkariView::new();
        view.start_stepper();
        let mut guard = 0;
        loop {
            // Reads the last event exactly as the status bar does each frame.
            let line = view.step_status();
            assert!(!line.is_empty());
            if view.stepper.as_ref().is_some_and(Stepper::is_done) {
                break;
            }
            view.step_once();
            guard += 1;
            assert!(guard < 1_000_000, "the search must terminate");
        }
        assert!(
            view.stepper.as_ref().is_some_and(Stepper::is_solved),
            "the sample board is solvable, so the stepped search ends in a solution"
        );
    }
}
