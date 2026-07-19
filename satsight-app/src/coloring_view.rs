//! The graph-coloring view — the second puzzle, in the demo (plan §7, issue #6).
//!
//! Its whole reason to exist is to make the reduction's *generality* visible: the
//! same core abstractions and the same generic backward maps that drive Sudoku
//! (`deduce`, `solve`, `backbone`) drive a puzzle that isn't a grid at all. A
//! fixed Petersen graph is rendered with its vertices and edges; clicking a vertex
//! cycles its color (a given), and the Deduce / Full solve / Backbone buttons paint
//! decoded solver state straight onto the graph — forced colors, the full coloring,
//! and the colors that hold across *every* coloring — in the puzzle's own language.
//!
//! The observable Step-view CDCL animation is Sudoku-specific in the renderer and
//! is deferred (a note on issue #6); this view covers the search-free and
//! full-solve maps, which is enough to demonstrate the abstraction is not
//! Sudoku-shaped.

use std::f32::consts::PI;

use eframe::egui;
use satsight_core::{BatSatBackend, Cnf, Registry, SolveOutcome, Solver, SolverView};
use satsight_puzzles::coloring::{ColoringCell, GraphColoring, VertexColor};
use satsight_puzzles::{backbone, deduce, Grid, Puzzle};

/// Distinct fills for colors `0..`; the Petersen graph is 3-chromatic, so three
/// are used, with a fourth on hand if the budget is ever raised.
const PALETTE: [egui::Color32; 4] = [
    egui::Color32::from_rgb(224, 96, 96),  // red
    egui::Color32::from_rgb(96, 176, 96),  // green
    egui::Color32::from_rgb(96, 144, 224), // blue
    egui::Color32::from_rgb(216, 176, 72), // amber
];

/// Ring color for a vertex a map *derived* (logic / full / backbone).
const LOGIC_COLOR: egui::Color32 = egui::Color32::from_rgb(80, 160, 255);
const FULL_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);
/// The backbone accent — forced in every coloring. Shared with the Sudoku view.
pub const BACKBONE_COLOR: egui::Color32 = egui::Color32::from_rgb(56, 178, 172);
const CORE_COLOR: egui::Color32 = egui::Color32::from_rgb(200, 70, 70);

/// Which decoded artifact the graph is painting over the givens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Logic,
    Full,
    Backbone,
}

/// The graph-coloring view: a fixed graph, its layout, and cached backward maps.
pub struct ColoringView {
    puzzle: GraphColoring,
    /// Per-vertex layout position, normalized to `[0, 1]²`.
    positions: Vec<egui::Pos2>,
    /// The forward encoding, built once: the registry (bridge) and the rule CNF.
    /// The graph never changes, so only the assumptions (color givens) do.
    reg: Registry<VertexColor>,
    batsat: BatSatBackend,
    overlay: Overlay,
    /// Colors forced by pure logic (propagation + probing), or `None` until run.
    logic: Option<Grid<ColoringCell>>,
    /// The full coloring from BatSat, or `None` until run / on UNSAT.
    full: Option<Grid<ColoringCell>>,
    /// Colors forced across *every* coloring (the backbone), or `None` until run.
    backbone: Option<Grid<ColoringCell>>,
    /// Vertices named by an UNSAT core, flagged in red (plan §4).
    core: Vec<usize>,
    status: String,
}

impl ColoringView {
    /// A fresh view over the Petersen graph with a budget of three colors — the
    /// classic 3-chromatic instance, tight enough that pinning a couple of vertices
    /// forces others (good for watching deduction) yet symmetric enough to have
    /// many colorings (so the backbone has something to say).
    #[must_use]
    pub fn new() -> Self {
        let (puzzle, positions) = petersen();
        let mut reg = Registry::new();
        let mut cnf = Cnf::new();
        puzzle.encode_rules(&mut reg, &mut cnf);
        let mut batsat = BatSatBackend::new();
        batsat.load_rules(&cnf);
        Self {
            puzzle,
            positions,
            reg,
            batsat,
            overlay: Overlay::None,
            logic: None,
            full: None,
            backbone: None,
            core: Vec::new(),
            status: "The Petersen graph, 3 colors. Click a vertex to cycle its color, \
                     then Deduce, Full solve, or Backbone."
                .to_owned(),
        }
    }

    /// Drop cached results after an edit and stop overlaying them.
    fn invalidate(&mut self) {
        self.overlay = Overlay::None;
        self.logic = None;
        self.full = None;
        self.backbone = None;
        self.core.clear();
    }

    /// Cycle the given color at `vertex`: none → 0 → … → n-1 → none.
    fn cycle(&mut self, vertex: usize) {
        let next = match self.puzzle.fixed(vertex) {
            None => Some(0),
            Some(c) if c + 1 < self.puzzle.n_colors() => Some(c + 1),
            Some(_) => None,
        };
        self.puzzle.fix(vertex, next);
        self.invalidate();
        self.status = String::from("Edited — press Deduce, Full solve, or Backbone.");
    }

    /// Run the search-free backward map (propagation + probing).
    fn run_logic(&mut self) {
        let deductions = deduce(&self.puzzle);
        self.status = if deductions.satisfiable {
            let forced = deductions.proven.iter().filter(|(_, holds)| *holds).count();
            format!("Logic: {forced} vertices forced by propagation + probing from the givens.")
        } else {
            String::from("These pre-colorings contradict — no proper coloring (UNSAT).")
        };
        self.logic = Some(self.puzzle.project_deductions(&deductions));
        self.overlay = Overlay::Logic;
        self.core.clear();
    }

    /// Run the full BatSat solve, flagging conflicting vertices on UNSAT.
    fn run_full(&mut self) {
        let assumptions = self.puzzle.assumptions(&self.reg);
        match self.batsat.solve(&assumptions) {
            SolveOutcome::Sat(model) => {
                let view = SolverView::from_model(&self.reg, &model);
                self.full = Some(self.puzzle.project(&view));
                self.core.clear();
                self.status = String::from("A proper coloring, found by full search (BatSat).");
            }
            SolveOutcome::Unsat(core) => {
                self.full = None;
                self.core = self.decode_vertices(&core);
                self.status = String::from(
                    "These pre-colorings contradict — the flagged vertices are the UNSAT core.",
                );
            }
        }
        self.overlay = Overlay::Full;
    }

    /// Run the backbone: colors forced across *every* proper coloring.
    fn run_backbone(&mut self) {
        let bb = backbone(&self.puzzle);
        if bb.satisfiable {
            let grid = self.puzzle.project_deductions(&bb);
            let (mut forced, mut free) = (0usize, 0usize);
            for v in 0..self.puzzle.n_vertices() {
                if self.puzzle.fixed(v).is_some() {
                    continue; // givens are trivially forced; count only the rest
                }
                if GraphColoring::color_at(&grid, v).is_some() {
                    forced += 1;
                } else {
                    free += 1;
                }
            }
            self.status = if free == 0 {
                String::from(
                    "Backbone: every vertex is forced — the coloring is unique given the clues. \
                     Clear a vertex to reveal colors that vary.",
                )
            } else {
                format!(
                    "Backbone: {forced} vertices take the same color in every coloring; \
                     {free} vary. Forced vertices are ringed teal; free ones are hollow."
                )
            };
            self.backbone = Some(grid);
        } else {
            self.backbone = None;
            self.status =
                String::from("These pre-colorings contradict — no colorings, so no backbone.");
        }
        self.overlay = Overlay::Backbone;
        self.core.clear();
    }

    /// Clear every given color.
    fn clear(&mut self) {
        for v in 0..self.puzzle.n_vertices() {
            self.puzzle.fix(v, None);
        }
        self.invalidate();
        self.status = String::from("Cleared. Click a vertex to cycle its color.");
    }

    /// Decode a list of literals to their vertices, sorted and de-duped.
    fn decode_vertices(&self, lits: &[satsight_core::Lit]) -> Vec<usize> {
        let mut vs: Vec<usize> = lits
            .iter()
            .filter_map(|&lit| self.reg.decode(lit).map(|(vc, _)| vc.vertex))
            .collect();
        vs.sort_unstable();
        vs.dedup();
        vs
    }

    /// The grid backing the current overlay, if any.
    fn overlay_grid(&self) -> Option<&Grid<ColoringCell>> {
        match self.overlay {
            Overlay::None => None,
            Overlay::Logic => self.logic.as_ref(),
            Overlay::Full => self.full.as_ref(),
            Overlay::Backbone => self.backbone.as_ref(),
        }
    }

    /// Render the whole view: controls, the graph, and the status/legend bar.
    pub fn ui(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("coloring_controls").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Deduce (logic)").clicked() {
                    self.run_logic();
                }
                if ui.button("Full solve").clicked() {
                    self.run_full();
                }
                if ui
                    .button("Backbone")
                    .on_hover_text("Colors that hold in every proper coloring of the graph.")
                    .clicked()
                {
                    self.run_backbone();
                }
                ui.separator();
                if ui.button("Clear givens").clicked() {
                    self.clear();
                }
                ui.label(format!("{} colors available", self.puzzle.n_colors()));
            });
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("coloring_status").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                swatch(ui, LOGIC_COLOR, "logic-proven");
                swatch(ui, FULL_COLOR, "full-solve");
                swatch(ui, BACKBONE_COLOR, "backbone (all colorings)");
                swatch(ui, CORE_COLOR, "UNSAT core");
            });
            ui.add_space(2.0);
            ui.label(&self.status);
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            ui.vertical_centered(|ui| self.draw_graph(ui));
        });
    }

    /// Draw the graph and handle clicks (cycling a clicked vertex's color).
    fn draw_graph(&mut self, ui: &mut egui::Ui) {
        let visuals = ui.visuals();
        let edge_stroke = egui::Stroke::new(1.5, visuals.weak_text_color());
        let core_stroke = egui::Stroke::new(2.5, CORE_COLOR);
        let given_ring = egui::Stroke::new(3.0, visuals.strong_text_color());
        let free_ring = egui::Stroke::new(1.5, visuals.weak_text_color());
        let hollow = visuals.extreme_bg_color;
        let label_color = visuals.strong_text_color();

        let side = ui
            .available_width()
            .min(ui.available_height())
            .clamp(240.0, 560.0);
        let (response, painter) = ui.allocate_painter(egui::vec2(side, side), egui::Sense::click());
        let rect = response.rect;
        let radius = side * 0.05;
        let to_screen = |p: egui::Pos2| rect.min + egui::vec2(p.x * side, p.y * side);
        let grid = self.overlay_grid();

        // Edges first, so vertices draw on top. An edge between two core vertices
        // is flagged (a same-color clash the core is about).
        for &(u, w) in self.puzzle.edges() {
            let clash = self.core.contains(&u) && self.core.contains(&w);
            painter.line_segment(
                [to_screen(self.positions[u]), to_screen(self.positions[w])],
                if clash { core_stroke } else { edge_stroke },
            );
        }

        for v in 0..self.puzzle.n_vertices() {
            let center = to_screen(self.positions[v]);
            let given = self.puzzle.fixed(v).is_some();
            // The color to show: the given, or whatever the overlay's grid decodes.
            let color = if let Some(g) = grid {
                GraphColoring::color_at(g, v)
            } else {
                self.puzzle.fixed(v)
            };
            let fill = color.map_or(hollow, |c| PALETTE[c % PALETTE.len()]);
            painter.circle_filled(center, radius, fill);

            // The ring encodes provenance: given (strong), derived (overlay accent),
            // or free/undetermined (weak). If a core flags this vertex, ring it red.
            let ring = if self.core.contains(&v) {
                core_stroke
            } else if given {
                given_ring
            } else if color.is_some() {
                let accent = match self.overlay {
                    Overlay::Backbone => BACKBONE_COLOR,
                    Overlay::Full => FULL_COLOR,
                    _ => LOGIC_COLOR,
                };
                egui::Stroke::new(2.5, accent)
            } else {
                free_ring
            };
            painter.circle_stroke(center, radius, ring);
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                v.to_string(),
                egui::FontId::proportional(radius * 0.9),
                label_color,
            );
        }

        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                for v in 0..self.puzzle.n_vertices() {
                    if (to_screen(self.positions[v]) - pos).length() <= radius * 1.3 {
                        self.cycle(v);
                        break;
                    }
                }
            }
        }
    }
}

impl Default for ColoringView {
    fn default() -> Self {
        Self::new()
    }
}

/// The Petersen graph and a symmetric two-ring layout: five outer vertices on a
/// pentagon, five inner ones forming the pentagram, joined by spokes.
fn petersen() -> (GraphColoring, Vec<egui::Pos2>) {
    let mut g = GraphColoring::new(10, 3);
    for i in 0..5 {
        g.add_edge(i, (i + 1) % 5); // outer cycle
        g.add_edge(i, i + 5); // spoke
        g.add_edge(5 + i, 5 + (i + 2) % 5); // inner pentagram
    }
    let center = egui::pos2(0.5, 0.5);
    let ring = |r: f32, i: usize| {
        let a = -PI / 2.0 + (i as f32) * 2.0 * PI / 5.0;
        center + egui::vec2(r * a.cos(), r * a.sin())
    };
    let mut positions = Vec::with_capacity(10);
    for i in 0..5 {
        positions.push(ring(0.42, i));
    }
    for i in 0..5 {
        positions.push(ring(0.20, i));
    }
    (g, positions)
}

/// A small colored swatch + label, for the status legend.
fn swatch(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
    ui.label(label);
}
