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
//! The **Step** view drives the observable CDCL one event at a time on the graph
//! (issue #12), the same animation the Sudoku view shows: it decides, propagates,
//! conflicts, learns, and backtracks, painting decided vertices apart from the
//! candidate-color pips of undecided ones and proven facts apart from hypothetical
//! ones — exactly the generic [`Stepper`] the Sudoku grid uses, now that it is no
//! longer Sudoku-shaped.
//!
//! Two [`Instance`]s are selectable: the symmetric **Petersen** graph, which
//! propagation and a little search dispatch easily, and the harder **Dürer** graph
//! — GP(6,2), the same generalized-Petersen family — which is 3-colorable yet no
//! local logic settles, so the observable search must genuinely guess, conflict,
//! and backtrack to solve it. That contrast is the point: Step the Dürer graph to
//! see the CDCL work for its answer.

use std::f32::consts::PI;

use eframe::egui;
use satsight_core::{BatSatBackend, Cdcl, Cnf, Event, Registry, SolveOutcome, Solver, SolverView};
use satsight_puzzles::coloring::{ColoringCell, GraphColoring, VertexColor};
use satsight_puzzles::{backbone, deduce, Grid, Puzzle};

use crate::stepper::{solver_phase, Certainty, Stepper, READY};

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
/// A vertex the stepped search has colored under the givens alone — a proven
/// fact; the search accent, matching the Sudoku Step view.
const SEARCH_COLOR: egui::Color32 = egui::Color32::from_rgb(120, 200, 120);
/// The vertex the last Step event touched (decision/propagation/conflict).
const EMPHASIS_COLOR: egui::Color32 = egui::Color32::from_rgb(240, 190, 90);

/// Which decoded artifact the graph is painting over the givens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Logic,
    Full,
    Backbone,
    /// The live state of the stepped CDCL search.
    Step,
}

/// The selectable graph instances. Both are generalized Petersen graphs with a
/// three-color budget, so the palette and legend never change; they differ only
/// in how hard the solver has to work.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Instance {
    /// GP(5,2): highly symmetric, dispatched with little search.
    Petersen,
    /// GP(6,2), the Dürer graph: 3-colorable but not by local logic — the Step
    /// view has to guess, conflict, and backtrack (issue: a harder instance).
    Durer,
}

impl Instance {
    /// The tab label for the graph switcher.
    fn label(self) -> &'static str {
        match self {
            Instance::Petersen => "Petersen",
            Instance::Durer => "Dürer",
        }
    }

    /// The graph and its concentric layout.
    fn build(self) -> (GraphColoring, Vec<egui::Pos2>) {
        match self {
            Instance::Petersen => petersen(),
            Instance::Durer => durer(),
        }
    }

    /// The opening status line describing this instance.
    fn blurb(self) -> &'static str {
        match self {
            Instance::Petersen => {
                "The Petersen graph, 3 colors. Click a vertex to cycle its color, then \
                 Deduce, Full solve, Backbone, or Step."
            }
            Instance::Durer => {
                "The Dürer graph GP(6,2), 3 colors — 3-colorable, but no local logic \
                 settles it. Press Step (or Play) to watch the search guess, hit a \
                 conflict, learn, and backtrack its way to a coloring."
            }
        }
    }
}

/// The graph-coloring view: a fixed graph, its layout, and cached backward maps.
pub struct ColoringView {
    /// Which graph instance is on screen (drives the switcher's selection).
    instance: Instance,
    puzzle: GraphColoring,
    /// Per-vertex layout position, normalized to `[0, 1]²`.
    positions: Vec<egui::Pos2>,
    /// The forward encoding, built once: the registry (bridge) and the rule CNF.
    /// The graph never changes, so only the assumptions (color givens) do.
    reg: Registry<VertexColor>,
    batsat: BatSatBackend,
    /// The observable CDCL over the rule CNF, spawning a [`Stepper`] per Step run.
    cdcl: Cdcl,
    overlay: Overlay,
    /// Colors forced by pure logic (propagation + probing), or `None` until run.
    logic: Option<Grid<ColoringCell>>,
    /// The full coloring from BatSat, or `None` until run / on UNSAT.
    full: Option<Grid<ColoringCell>>,
    /// Colors forced across *every* coloring (the backbone), or `None` until run.
    backbone: Option<Grid<ColoringCell>>,
    /// The live stepped search, or `None` until "Step"/"Play" is pressed.
    stepper: Option<Stepper<VertexColor>>,
    /// Vertices named by an UNSAT core, flagged in red (plan §4).
    core: Vec<usize>,
    /// Steps advanced per frame while playing.
    speed: u32,
    status: String,
}

impl ColoringView {
    /// A fresh view over the Petersen graph — the classic 3-chromatic instance,
    /// symmetric enough to have many colorings (so the backbone has something to
    /// say). Switch to the Dürer graph for one that needs real search.
    #[must_use]
    pub fn new() -> Self {
        Self::from_instance(Instance::Petersen)
    }

    /// Build the view for a given graph instance: encode its rules once (the graph
    /// never changes, only the color givens do) and load both solver backends.
    fn from_instance(instance: Instance) -> Self {
        let (puzzle, positions) = instance.build();
        let mut reg = Registry::new();
        let mut cnf = Cnf::new();
        puzzle.encode_rules(&mut reg, &mut cnf);
        let mut batsat = BatSatBackend::new();
        batsat.load_rules(&cnf);
        let cdcl = Cdcl::from_cnf(&cnf);
        Self {
            instance,
            puzzle,
            positions,
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
            status: instance.blurb().to_owned(),
        }
    }

    /// Switch to a different graph instance, rebuilding its rules and dropping
    /// every cached result. Each graph has its own encoding, so the registry and
    /// both solvers are rebuilt; the play speed carries over.
    fn load(&mut self, instance: Instance) {
        let speed = self.speed;
        *self = Self::from_instance(instance);
        self.speed = speed;
    }

    /// Drop cached results after an edit and stop overlaying them.
    fn invalidate(&mut self) {
        self.overlay = Overlay::None;
        self.logic = None;
        self.full = None;
        self.backbone = None;
        self.stepper = None;
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

    /// Begin (or restart) a stepped search over the current pre-colorings.
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

    /// Pull the current core vertices out of the stepper (empty unless it hit
    /// UNSAT).
    fn sync_core(&mut self) {
        self.core = self
            .stepper
            .as_ref()
            .map(step_core_vertices)
            .unwrap_or_default();
    }

    /// The stepper's last event, decoded into a one-line status in the coloring
    /// puzzle's own vocabulary (the solver-mechanics moves come from the shared
    /// [`solver_phase`]).
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
            Event::Decide { lit } => {
                let (vc, holds) = st.decode(*lit).expect("a coloring literal decodes");
                let rel = if holds { "=" } else { "≠" };
                format!("Guess: vertex {} {rel} {}", vc.vertex, color_name(vc.color))
            }
            Event::Propagate { lit, .. } => {
                let (vc, holds) = st.decode(*lit).expect("a coloring literal decodes");
                if holds {
                    format!("Forced: vertex {} = {}", vc.vertex, color_name(vc.color))
                } else {
                    format!("Ruled out {} at vertex {}", color_name(vc.color), vc.vertex)
                }
            }
            Event::Sat => "A proper coloring, found by search!".to_owned(),
            Event::Unsat { .. } => {
                "These pre-colorings contradict — no proper coloring exists.".to_owned()
            }
            // Conflict/Backtrack/Learn are handled by `solver_phase` above.
            _ => unreachable!("solver_phase covers the remaining events"),
        }
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

    /// The grid backing the current overlay, if any. The Step view reads live
    /// state straight from the [`Stepper`] instead of a cached grid, so it has no
    /// grid here.
    fn overlay_grid(&self) -> Option<&Grid<ColoringCell>> {
        match self.overlay {
            Overlay::None | Overlay::Step => None,
            Overlay::Logic => self.logic.as_ref(),
            Overlay::Full => self.full.as_ref(),
            Overlay::Backbone => self.backbone.as_ref(),
        }
    }

    /// The live Step state of vertex `v`: its decided color (proven vs
    /// hypothetical), its still-possible candidate colors, and the colors ruled
    /// out only under the current guess. `None` if no search is running.
    fn step_vertex(&self, vertex: usize, n_colors: usize) -> Option<StepVertex> {
        let st = self.stepper.as_ref()?;
        let placed = (0..n_colors)
            .find(|&color| st.value(&VertexColor { vertex, color }) == Some(true))
            .map(|color| (color, st.certainty(&VertexColor { vertex, color })));
        let candidate = (0..n_colors)
            .map(|color| st.value(&VertexColor { vertex, color }) != Some(false))
            .collect();
        let base = st.base_level();
        let hypo_eliminated = (0..n_colors)
            .map(|color| {
                matches!(
                    st.assigned(&VertexColor { vertex, color }),
                    Some((false, level)) if level > base
                )
            })
            .collect();
        Some(StepVertex {
            placed,
            candidate,
            hypo_eliminated,
        })
    }

    /// The top control bar: graph switcher, the backward-map buttons, and the
    /// stepping controls.
    fn draw_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Graph:");
            let mut chosen = self.instance;
            for inst in [Instance::Petersen, Instance::Durer] {
                ui.selectable_value(&mut chosen, inst, inst.label());
            }
            if chosen != self.instance {
                self.load(chosen);
            }
            ui.separator();
            ui.label(format!("{} colors available", self.puzzle.n_colors()));
        });
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

    /// Render the whole view: controls, the graph, and the status/legend bar.
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

        egui::TopBottomPanel::top("coloring_controls").show(ctx, |ui| {
            ui.add_space(4.0);
            self.draw_controls(ui);
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
                if self.overlay == Overlay::Step {
                    swatch(ui, SEARCH_COLOR, "search-proven");
                    swatch(ui, SEARCH_COLOR.gamma_multiply(0.5), "guess (hypothetical)");
                    swatch(ui, EMPHASIS_COLOR, "last step");
                }
            });
            ui.add_space(2.0);
            // In Step mode the status tracks the live event; otherwise the last
            // action's summary.
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
            ui.vertical_centered(|ui| self.draw_graph(ui));
        });
    }

    /// The fill color and provenance ring for vertex `v`. A given keeps its strong
    /// ring in every view; the Step view reads live search state (a proven vs a
    /// hypothetical placement), the others the cached overlay `grid` (or, with no
    /// overlay, just the givens).
    fn vertex_fill_ring(
        &self,
        v: usize,
        given: bool,
        step_state: Option<&StepVertex>,
        grid: Option<&Grid<ColoringCell>>,
        theme: &GraphTheme,
    ) -> (egui::Color32, egui::Stroke) {
        if given {
            let c = self.puzzle.fixed(v).expect("a given vertex has a color");
            return (PALETTE[c % PALETTE.len()], theme.given_ring);
        }
        if self.overlay == Overlay::Step {
            return match step_state.and_then(|sv| sv.placed) {
                // A vertex colored by the givens alone (proven) vs one colored only
                // under the current guess (hypothetical), drawn faded.
                Some((c, Certainty::Proven)) => (
                    PALETTE[c % PALETTE.len()],
                    egui::Stroke::new(2.5_f32, SEARCH_COLOR),
                ),
                Some((c, Certainty::Hypothetical)) => (
                    PALETTE[c % PALETTE.len()].gamma_multiply(0.5),
                    egui::Stroke::new(2.5_f32, SEARCH_COLOR.gamma_multiply(0.5)),
                ),
                // Undecided: hollow, with candidate-color pips drawn separately.
                None => (theme.hollow, theme.free_ring),
            };
        }
        let color = grid.map_or_else(|| self.puzzle.fixed(v), |g| GraphColoring::color_at(g, v));
        let ring = if color.is_some() {
            let accent = match self.overlay {
                Overlay::Backbone => BACKBONE_COLOR,
                Overlay::Full => FULL_COLOR,
                _ => LOGIC_COLOR,
            };
            egui::Stroke::new(2.5_f32, accent)
        } else {
            theme.free_ring
        };
        (
            color.map_or(theme.hollow, |c| PALETTE[c % PALETTE.len()]),
            ring,
        )
    }

    /// Draw the graph and handle clicks (cycling a clicked vertex's color).
    fn draw_graph(&mut self, ui: &mut egui::Ui) {
        let visuals = ui.visuals();
        let edge_stroke = egui::Stroke::new(1.5_f32, visuals.weak_text_color());
        let core_stroke = egui::Stroke::new(2.5_f32, CORE_COLOR);
        let label_color = visuals.strong_text_color();
        // The faint accent for a struck candidate pip (a hypothetical elimination).
        let pip_weak = visuals.weak_text_color();
        let theme = GraphTheme {
            given_ring: egui::Stroke::new(3.0_f32, visuals.strong_text_color()),
            free_ring: egui::Stroke::new(1.5_f32, visuals.weak_text_color()),
            hollow: visuals.extreme_bg_color,
        };

        let side = ui
            .available_width()
            .min(ui.available_height())
            .clamp(240.0, 560.0);
        let (response, painter) = ui.allocate_painter(egui::vec2(side, side), egui::Sense::click());
        let rect = response.rect;
        let radius = side * 0.05;
        let to_screen = |p: egui::Pos2| rect.min + egui::vec2(p.x * side, p.y * side);
        let grid = self.overlay_grid();
        let stepping = self.overlay == Overlay::Step;
        let n_colors = self.puzzle.n_colors();
        // In the Step view, the vertices the last event touched (amber outline).
        let emphasis: Vec<usize> = if stepping {
            self.stepper
                .as_ref()
                .map(step_touched_vertices)
                .unwrap_or_default()
        } else {
            Vec::new()
        };

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
            // The Step view reads live search state per vertex; the others read the
            // cached overlay grid (or, with no overlay, just the givens).
            let step_state = (stepping && !given)
                .then(|| self.step_vertex(v, n_colors))
                .flatten();

            // Fill + provenance ring. A given keeps its strong ring in every view.
            let (fill, mut ring) =
                self.vertex_fill_ring(v, given, step_state.as_ref(), grid, &theme);
            painter.circle_filled(center, radius, fill);

            // Candidate-color pips for an undecided Step vertex: a filled dot per
            // still-possible color, a struck dot for a hypothetical elimination.
            if let Some(sv) = step_state.as_ref() {
                if sv.placed.is_none() {
                    draw_color_pips(&painter, center, radius, sv, pip_weak);
                }
            }

            // The core (red) and last-touched (amber) outlines win over provenance.
            if self.core.contains(&v) {
                ring = core_stroke;
            } else if emphasis.contains(&v) {
                ring = egui::Stroke::new(2.5_f32, EMPHASIS_COLOR);
            }
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

/// The Dürer graph — the generalized Petersen graph GP(6,2) — and its concentric
/// layout: an outer hexagon, an inner hexagram (two triangles, the inner ring
/// stepping by two), joined by spokes. Same family as the Petersen graph (GP(5,2))
/// but a genuine step up: it is 3-colorable, yet no local logic settles it, so the
/// observable search has to guess, conflict, and backtrack to color it — which is
/// what makes it a good Step-view demo.
fn durer() -> (GraphColoring, Vec<egui::Pos2>) {
    let mut g = GraphColoring::new(12, 3);
    for i in 0..6 {
        g.add_edge(i, (i + 1) % 6); // outer hexagon
        g.add_edge(i, i + 6); // spoke
        g.add_edge(6 + i, 6 + (i + 2) % 6); // inner hexagram (two triangles)
    }
    let center = egui::pos2(0.5, 0.5);
    let ring = |r: f32, i: usize| {
        let a = -PI / 2.0 + (i as f32) * 2.0 * PI / 6.0;
        center + egui::vec2(r * a.cos(), r * a.sin())
    };
    let mut positions = Vec::with_capacity(12);
    for i in 0..6 {
        positions.push(ring(0.42, i));
    }
    for i in 0..6 {
        positions.push(ring(0.22, i));
    }
    (g, positions)
}

/// A small colored swatch + label, for the status legend.
fn swatch(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
    ui.label(label);
}

/// The theme-derived strokes and fill the graph drawing reuses for every vertex.
struct GraphTheme {
    /// The strong ring on a given (pre-colored) vertex.
    given_ring: egui::Stroke,
    /// The weak ring on a free/undetermined vertex.
    free_ring: egui::Stroke,
    /// The fill for a vertex with no color to show.
    hollow: egui::Color32,
}

/// One vertex's live Step state, mirroring the Sudoku grid's per-cell marks:
/// which color (if any) it holds and with what [`Certainty`], its surviving
/// candidate colors, and the colors ruled out only under the current guess.
struct StepVertex {
    /// The decided color and whether it is proven (vs hypothetical), if any.
    placed: Option<(usize, Certainty)>,
    /// Per color: still Boolean-possible here (a candidate).
    candidate: Vec<bool>,
    /// Per color: ruled out only above the givens' base level (a hypothetical
    /// elimination). A proven elimination is simply absent.
    hypo_eliminated: Vec<bool>,
}

/// The vertices the stepper's last event touched, sorted and de-duped — the amber
/// "last step" outline.
fn step_touched_vertices(st: &Stepper<VertexColor>) -> Vec<usize> {
    let mut vs: Vec<usize> = st.touched_props().iter().map(|vc| vc.vertex).collect();
    vs.sort_unstable();
    vs.dedup();
    vs
}

/// The vertices an UNSAT core names, sorted and de-duped — the red core flag.
fn step_core_vertices(st: &Stepper<VertexColor>) -> Vec<usize> {
    let mut vs: Vec<usize> = st.core_props().iter().map(|vc| vc.vertex).collect();
    vs.sort_unstable();
    vs.dedup();
    vs
}

/// Draw an undecided vertex's candidate-color pips: a small filled dot per color
/// still possible, and a struck hollow dot for a color the current guess rules
/// out (a *hypothetical* elimination, plan §1). A proven elimination is left
/// blank — a known non-fact isn't drawn, matching the Sudoku Step view.
fn draw_color_pips(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    sv: &StepVertex,
    weak: egui::Color32,
) {
    let n = sv.candidate.len();
    if n == 0 {
        return;
    }
    let dot_r = radius * 0.24;
    let gap = radius * 0.55;
    let start = -gap * (n as f32 - 1.0) / 2.0;
    for color in 0..n {
        let pos = center + egui::vec2(start + gap * color as f32, 0.0);
        if sv.candidate[color] {
            painter.circle_filled(pos, dot_r, PALETTE[color % PALETTE.len()]);
        } else if sv.hypo_eliminated[color] {
            let stroke = egui::Stroke::new(1.0_f32, weak);
            painter.circle_stroke(pos, dot_r, stroke);
            let reach = dot_r * 0.9;
            painter.line_segment(
                [pos - egui::vec2(reach, 0.0), pos + egui::vec2(reach, 0.0)],
                stroke,
            );
        }
    }
}

/// A human name for color index `c`, matching the [`PALETTE`] order.
fn color_name(c: usize) -> String {
    ["red", "green", "blue", "amber"]
        .get(c)
        .map_or_else(|| format!("color {c}"), |name| (*name).to_owned())
}

#[cfg(test)]
mod tests {
    use super::{color_name, step_core_vertices, Instance};
    use crate::stepper::{Certainty, Stepper};
    use satsight_core::{Cdcl, Cnf, Event, Registry};
    use satsight_puzzles::coloring::{GraphColoring, VertexColor};
    use satsight_puzzles::{deduce, solve, Puzzle};

    /// Encode `g`'s rules and start a stepped search over its pre-colorings —
    /// exactly what [`ColoringView::start_stepper`](super::ColoringView) does.
    fn stepper_for(g: &GraphColoring) -> Stepper<VertexColor> {
        let mut reg = Registry::new();
        let mut cnf = Cnf::new();
        g.encode_rules(&mut reg, &mut cnf);
        let assumptions = g.assumptions(&reg);
        let cdcl = Cdcl::from_cnf(&cnf);
        Stepper::new(&cdcl, reg, &assumptions)
    }

    #[test]
    fn stepping_a_triangle_forces_the_last_color() {
        // K3 with three colors, two vertices pinned to distinct colors: the *same*
        // generic Stepper the Sudoku grid drives must place the third vertex on the
        // one remaining color — and, being entailed by the givens alone, as a
        // proven fact (issue #12: the Step view is no longer Sudoku-shaped).
        let mut g = GraphColoring::complete(3, 3);
        g.fix(0, Some(0));
        g.fix(1, Some(1));
        let mut st = stepper_for(&g);
        let mut guard = 0;
        while !st.is_done() {
            st.step();
            guard += 1;
            assert!(guard < 100_000, "the search must terminate");
        }
        assert!(st.is_solved(), "K3 with two colors pinned is colorable");
        let forced = VertexColor {
            vertex: 2,
            color: 2,
        };
        assert_eq!(st.value(&forced), Some(true), "vertex 2 takes color 2");
        assert_eq!(
            st.certainty(&forced),
            Certainty::Proven,
            "it holds in every coloring, so it reads as proven, not a guess"
        );
        // The other two colors are ruled out at vertex 2.
        for color in [0, 1] {
            assert_eq!(
                st.value(&VertexColor { vertex: 2, color }),
                Some(false),
                "color {color} is ruled out at vertex 2"
            );
        }
    }

    #[test]
    fn contradictory_precoloring_steps_to_an_unsat_core() {
        // Two adjacent vertices pinned to the same color: the stepped search must
        // finish UNSAT and the core must name those conflicting vertices — plan
        // §4's core→givens highlight, decoded on the second puzzle.
        let mut g = GraphColoring::new(2, 3);
        g.add_edge(0, 1);
        g.fix(0, Some(1));
        g.fix(1, Some(1));
        let mut st = stepper_for(&g);
        while !st.is_done() {
            st.step();
        }
        assert!(!st.is_solved(), "a same-color edge is uncolorable");
        let core = step_core_vertices(&st);
        assert!(!core.is_empty(), "UNSAT reports a core");
        assert!(
            core.iter().all(|&v| v == 0 || v == 1),
            "the core names only the conflicting vertices"
        );
    }

    #[test]
    fn color_names_match_the_palette_order() {
        assert_eq!(color_name(0), "red");
        assert_eq!(color_name(2), "blue");
        // Beyond the named palette, fall back to a numeric label.
        assert_eq!(color_name(9), "color 9");
    }

    #[test]
    fn durer_graph_is_solvable_but_needs_search() {
        // The harder instance (GP(6,2), the Dürer graph): it is 3-colorable, yet
        // no local logic settles it — so the observable CDCL must genuinely branch
        // and backtrack. Assert both, so the Step-view demo can't silently regress
        // into something propagation solves outright.
        let (g, positions) = Instance::Durer.build();
        assert_eq!(g.n_vertices(), 12);
        assert_eq!(positions.len(), 12);
        assert!(solve(&g).is_some(), "the Dürer graph is 3-colorable");
        // Pure logic (propagation + probing, no givens) proves nothing here.
        assert!(
            deduce(&g).proven.iter().all(|(_, holds)| !holds),
            "no vertex is forced by logic alone"
        );

        // Drive the same stepper the UI drives, counting real search.
        let mut st = stepper_for(&g);
        let (mut conflicts, mut backtracks) = (0u32, 0u32);
        let mut guard = 0;
        while !st.is_done() {
            if let Some(event) = {
                st.step();
                st.last_event()
            } {
                match event {
                    Event::Conflict { .. } => conflicts += 1,
                    Event::Backtrack { .. } => backtracks += 1,
                    _ => {}
                }
            }
            guard += 1;
            assert!(guard < 1_000_000, "the search must terminate");
        }
        assert!(st.is_solved(), "the search finds a coloring");
        assert!(
            conflicts > 0 && backtracks > 0,
            "coloring it takes real backtracking search: conflicts={conflicts} \
             backtracks={backtracks}"
        );
    }
}
