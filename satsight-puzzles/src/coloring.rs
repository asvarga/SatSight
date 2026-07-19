//! Graph coloring — the second puzzle, validating the abstraction (plan §7).
//!
//! Sudoku is *nothing but* exactly-one constraints, which risks letting
//! Sudoku-shaped assumptions leak into the core. Graph coloring is the low-effort
//! validator the plan calls for: it keeps exactly-one (one color per vertex) but
//! adds a genuinely different clause shape — for every edge and color, a binary
//! "these two endpoints can't share this color". Nothing about it is
//! Sudoku-shaped, yet it crosses the same bridge:
//!
//! - **Forward**: [`encode_rules`](GraphColoring::encode_rules) emits exactly-one
//!   per vertex plus the edge clauses; [`assumptions`](GraphColoring::assumptions)
//!   turns pre-colored vertices (givens) into assumption literals (plan §4).
//! - **Backward**: [`project`](GraphColoring::project) decodes a solver view into
//!   a one-row grid of per-vertex colors.
//!
//! Because it implements the same [`Puzzle`] trait, the generic
//! [`solve`](crate::solve) / [`deduce`](crate::deduce) pipelines, and *both*
//! solver backends, drive it with no changes to `satsight-core` — the proof that
//! the reduction is not Sudoku-specific.

use satsight_core::cnf::{Cnf, Lit};
use satsight_core::encodings::exactly_one_pairwise;
use satsight_core::registry::Registry;
use satsight_core::view::SolverView;

use crate::puzzle::{Deductions, Grid, Puzzle};

/// A graph-coloring proposition: "vertex `vertex` takes color `color`" (the
/// reduction's vocabulary). `vertex` is `0..n_vertices`, `color` is `0..n_colors`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VertexColor {
    pub vertex: usize,
    pub color: usize,
}

/// Per-vertex display state produced by [`GraphColoring::project`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColoringCell {
    /// The color assigned to this vertex (`0..n_colors`), if any.
    pub color: Option<usize>,
    /// Whether this vertex was pre-colored (a given).
    pub fixed: bool,
}

/// A graph-coloring instance: a graph, a color budget, and any pre-colorings.
#[derive(Debug, Clone)]
pub struct GraphColoring {
    n_vertices: usize,
    n_colors: usize,
    /// Undirected edges as `(u, w)` with `u < w` (normalized on insertion).
    edges: Vec<(usize, usize)>,
    /// `fixed[v] == Some(c)` pre-colors vertex `v` (a given).
    fixed: Vec<Option<usize>>,
}

impl GraphColoring {
    /// An edgeless graph on `n_vertices` with a budget of `n_colors`.
    #[must_use]
    pub fn new(n_vertices: usize, n_colors: usize) -> Self {
        Self {
            n_vertices,
            n_colors,
            edges: Vec::new(),
            fixed: vec![None; n_vertices],
        }
    }

    /// Add an undirected edge between `u` and `w` (a no-op self-loop is ignored,
    /// and duplicates are collapsed).
    pub fn add_edge(&mut self, u: usize, w: usize) {
        if u == w {
            return;
        }
        let edge = (u.min(w), u.max(w));
        if !self.edges.contains(&edge) {
            self.edges.push(edge);
        }
    }

    /// Pre-color `vertex` with `color` (a given), or clear it with `None`.
    pub fn fix(&mut self, vertex: usize, color: Option<usize>) {
        self.fixed[vertex] = color;
    }

    /// The complete graph `K_n` (every pair adjacent) with `n_colors` available.
    #[must_use]
    pub fn complete(n_vertices: usize, n_colors: usize) -> Self {
        let mut g = Self::new(n_vertices, n_colors);
        for u in 0..n_vertices {
            for w in u + 1..n_vertices {
                g.add_edge(u, w);
            }
        }
        g
    }

    /// The color assigned to `vertex` in a projected grid, if any.
    #[must_use]
    pub fn color_at(grid: &Grid<ColoringCell>, vertex: usize) -> Option<usize> {
        grid.get(0, vertex).color
    }

    /// The number of vertices.
    #[must_use]
    pub fn n_vertices(&self) -> usize {
        self.n_vertices
    }

    /// The color budget.
    #[must_use]
    pub fn n_colors(&self) -> usize {
        self.n_colors
    }

    /// The undirected edges, each as `(u, w)` with `u < w`.
    #[must_use]
    pub fn edges(&self) -> &[(usize, usize)] {
        &self.edges
    }

    /// The pre-colored (given) color of `vertex`, if any.
    #[must_use]
    pub fn fixed(&self, vertex: usize) -> Option<usize> {
        self.fixed[vertex]
    }

    /// A grid showing only what a set of [`Deductions`] proves: pre-colorings plus
    /// forced colors, with still-undetermined vertices left blank. Mirrors
    /// [`Sudoku::project_deductions`](crate::sudoku::Sudoku::project_deductions),
    /// so the frontend renders logic and backbone results the same way it renders a
    /// full solution.
    #[must_use]
    pub fn project_deductions(&self, deductions: &Deductions<VertexColor>) -> Grid<ColoringCell> {
        let mut colors: Vec<Option<usize>> = self.fixed.clone();
        for (vc, holds) in &deductions.proven {
            if *holds {
                colors[vc.vertex] = Some(vc.color);
            }
        }
        Grid::from_fn(1, self.n_vertices, |_, vertex| ColoringCell {
            color: colors[vertex],
            fixed: self.fixed[vertex].is_some(),
        })
    }
}

impl Puzzle for GraphColoring {
    type Var = VertexColor;
    type Cell = ColoringCell;

    fn encode_rules(&self, reg: &mut Registry<VertexColor>, cnf: &mut Cnf) {
        // Register every proposition up front so variables are dense and
        // vertex-major.
        for vertex in 0..self.n_vertices {
            for color in 0..self.n_colors {
                reg.var(VertexColor { vertex, color });
            }
        }

        // Exactly one color per vertex.
        for vertex in 0..self.n_vertices {
            let lits: Vec<Lit> = (0..self.n_colors)
                .map(|color| reg.var(VertexColor { vertex, color }).pos_lit())
                .collect();
            exactly_one_pairwise(&lits, cnf);
        }

        // Adjacent vertices may not share a color: ¬(u=c) ∨ ¬(w=c).
        for &(u, w) in &self.edges {
            for color in 0..self.n_colors {
                let uc = reg.var(VertexColor { vertex: u, color }).pos_lit();
                let wc = reg.var(VertexColor { vertex: w, color }).pos_lit();
                cnf.add_clause(satsight_core::cnf::clause([!uc, !wc]));
            }
        }
    }

    fn assumptions(&self, reg: &Registry<VertexColor>) -> Vec<Lit> {
        let mut assumps = Vec::new();
        for (vertex, fixed) in self.fixed.iter().enumerate() {
            if let Some(color) = *fixed {
                if let Some(var) = reg.get(&VertexColor { vertex, color }) {
                    assumps.push(var.pos_lit());
                }
            }
        }
        assumps
    }

    fn project(&self, view: &SolverView<VertexColor>) -> Grid<ColoringCell> {
        Grid::from_fn(1, self.n_vertices, |_, vertex| {
            let color = (0..self.n_colors)
                .find(|&color| view.value(&VertexColor { vertex, color }) == Some(true));
            ColoringCell {
                color,
                fixed: self.fixed[vertex].is_some(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ColoringCell, GraphColoring, VertexColor};
    use crate::puzzle::{backbone, deduce, solve, Grid, Puzzle};
    use satsight_core::cdcl::Cdcl;
    use satsight_core::cnf::Cnf;
    use satsight_core::registry::Registry;
    use satsight_core::solver::{SolveOutcome, Solver};
    use satsight_core::view::SolverView;
    use satsight_core::BatSatBackend;

    /// Does every edge join two differently colored (and fully colored) vertices?
    fn is_proper(g: &GraphColoring, grid: &Grid<ColoringCell>) -> bool {
        (0..g.n_vertices).all(|v| GraphColoring::color_at(grid, v).is_some())
            && g.edges
                .iter()
                .all(|&(u, w)| GraphColoring::color_at(grid, u) != GraphColoring::color_at(grid, w))
    }

    #[test]
    fn triangle_is_three_colorable_but_not_two() {
        // K3 needs three colors: 3-coloring is SAT, 2-coloring is UNSAT — and the
        // second puzzle round-trips through the generic pipeline unchanged.
        let three = GraphColoring::complete(3, 3);
        let grid = solve(&three).expect("K3 is 3-colorable");
        assert!(is_proper(&three, &grid));

        let two = GraphColoring::complete(3, 2);
        assert!(solve(&two).is_none(), "K3 is not 2-colorable");
    }

    #[test]
    fn logic_forces_the_last_color_of_a_triangle() {
        // K3 with three colors, two vertices pre-colored to distinct colors: pure
        // logic must force the third vertex to the one remaining color — the
        // backward map working on a non-Sudoku puzzle.
        let mut g = GraphColoring::complete(3, 3);
        g.fix(0, Some(0));
        g.fix(1, Some(1));
        let deductions = deduce(&g);
        assert!(deductions.satisfiable);
        // Vertex 2 must be proven to hold color 2 and to be ruled out of 0 and 1.
        assert!(deductions.proven.contains(&(
            VertexColor {
                vertex: 2,
                color: 2
            },
            true
        )));
        assert!(deductions.proven.contains(&(
            VertexColor {
                vertex: 2,
                color: 0
            },
            false
        )));
    }

    #[test]
    fn both_backends_agree_on_a_coloring_instance() {
        // A 5-cycle with 3 colors is colorable; the observable CDCL and BatSat
        // must agree, and the CDCL's model must be a proper coloring (plan §5's
        // oracle check, on the second puzzle).
        let mut g = GraphColoring::new(5, 3);
        for v in 0..5 {
            g.add_edge(v, (v + 1) % 5);
        }
        let mut reg: Registry<VertexColor> = Registry::new();
        let mut cnf = Cnf::new();
        g.encode_rules(&mut reg, &mut cnf);
        let assumptions = g.assumptions(&reg);

        let cdcl_outcome = Cdcl::from_cnf(&cnf).solve(&assumptions);
        let batsat_outcome = BatSatBackend::solve_cnf(&cnf, &assumptions);
        match (&cdcl_outcome, &batsat_outcome) {
            (SolveOutcome::Sat(model), SolveOutcome::Sat(_)) => {
                let grid = g.project(&SolverView::from_model(&reg, model));
                assert!(is_proper(&g, &grid));
            }
            (SolveOutcome::Unsat(_), SolveOutcome::Unsat(_)) => {
                panic!("the 5-cycle is 3-colorable")
            }
            _ => panic!("CDCL and BatSat disagree on the coloring instance"),
        }
    }

    #[test]
    fn backbone_distinguishes_forced_facts_from_free_choices() {
        // K3, three colors, only vertex 0 pinned to color 0: vertices 1 and 2 swap
        // between the two colorings, so neither is forced to a *specific* color —
        // yet color 0 is ruled out for both in *every* coloring (they neighbor v0).
        // The backbone must capture those forced eliminations but no free placement.
        let mut g = GraphColoring::complete(3, 3);
        g.fix(0, Some(0));
        let bb = backbone(&g);
        assert!(bb.satisfiable);
        assert!(bb.proven.contains(&(
            VertexColor {
                vertex: 1,
                color: 0
            },
            false
        )));
        assert!(bb.proven.contains(&(
            VertexColor {
                vertex: 2,
                color: 0
            },
            false
        )));
        assert!(
            !bb.proven
                .iter()
                .any(|(vc, holds)| *holds && (vc.vertex == 1 || vc.vertex == 2)),
            "v1 and v2 are free, so no positive placement is in the backbone"
        );
    }

    #[test]
    fn backbone_captures_a_forced_vertex() {
        // Pin two of the triangle to distinct colors: the third is forced in every
        // coloring (there is only one), so the backbone includes its placement —
        // the same conclusion `deduce` reaches, via all-solutions reasoning.
        let mut g = GraphColoring::complete(3, 3);
        g.fix(0, Some(0));
        g.fix(1, Some(1));
        let bb = backbone(&g);
        assert!(bb.satisfiable);
        assert!(bb.proven.contains(&(
            VertexColor {
                vertex: 2,
                color: 2
            },
            true
        )));
    }

    #[test]
    fn contradictory_precoloring_is_unsat() {
        // Two adjacent vertices fixed to the same color: no coloring exists.
        let mut g = GraphColoring::new(2, 3);
        g.add_edge(0, 1);
        g.fix(0, Some(1));
        g.fix(1, Some(1));
        assert!(solve(&g).is_none());
        // Logic alone (propagation) already spots it.
        assert!(!deduce(&g).satisfiable);
    }
}
