# SatSight — Bidirectional SAT Reduction Library + egui Demo

A Rust library for solving puzzles by reduction to SAT, with the distinguishing feature
that solver discoveries can be projected **back** into the puzzle's own language — partially,
incrementally, and live. Ships with an `egui` demo that runs in the browser, steps through the
solver, visualizes what it learns, and lets the user edit the puzzle.

---

## 1. The core insight

"Bidirectional reduction" reads like two translators, but it is really **one object viewed two
ways**: a *variable registry* that is a bijection between puzzle-level propositions and SAT
variables. Every forward step (encoding) and every backward step (interpreting what the solver
found) routes through that same registry.

The forward direction is standard. The interesting design is the backward direction, and the key
realization is that "what the solver discovered" is not one thing — it is a family of **artifacts**,
each decoding to a different puzzle-level meaning:

| Solver artifact                      | Puzzle-level meaning                                                      |
|--------------------------------------|---------------------------------------------------------------------------|
| Literals fixed at decision level 0   | **Proven** facts — "this cell *must* be 5" (forced moves / naked singles) |
| Current partial trail during search  | Tentative puzzle state mid-search (drives animation)                      |
| BCP-surviving candidates             | What is still Boolean-possible for each cell (corner marks)               |
| Learned clauses (short, house-local) | Discovered relationships — "these cells can't all take these values"      |
| Failed-literal probing results       | Proven eliminations beyond naked singles (human-technique equivalents)    |
| Backbone (true in every model)       | Facts holding across all solutions                                        |
| UNSAT core over assumption literals  | Minimal contradictory subset of the user's clues                          |

That table is essentially the reverse-mapping API. The demo's job is to make each row visible on the
grid — with one exception found in practice: the *learned-clauses* row was built and then removed,
because for Sudoku those clauses are far too long to decode into a readable relationship (see §8).

---

## 2. Workspace layout

```
puzzlesat/                    # cargo workspace
├─ puzzlesat-core/            # solver-agnostic, no egui, no_std-friendly where practical
│  ├─ registry.rs             # PuzzleVar <-> SatVar bijection  (THE bridge)
│  ├─ cnf.rs                  # Lit / Clause / Cnf (wrap rustsat types)
│  ├─ encodings.rs            # exactly_one, at_most_one (pairwise + sequential)
│  ├─ solver.rs               # Solver trait + Event enum
│  ├─ cdcl.rs                 # small INSTRUMENTABLE CDCL (the steppable backend)
│  ├─ backend_batsat.rs       # rustsat/BatSat wrapper (fast "run to solution")
│  └─ view.rs                 # SolverView: decoded artifacts + candidate lattice + probe()
├─ puzzlesat-puzzles/         # Puzzle trait + Sudoku (primary) + Akari or graph-coloring (2nd)
└─ puzzlesat-app/             # eframe/egui; compiles to wasm via trunk
```

**Split rationale:** the core is solver-agnostic and testable in isolation; the puzzles crate
proves the abstraction is not Sudoku-shaped; the app is the only crate that touches egui/wasm.

---

## 3. The load-bearing abstractions

```rust
/// The bijection. Both directions of the reduction go through here.
pub struct Registry<V: Eq + Hash + Clone> {
    fwd: HashMap<V, Var>,
    rev: Vec<V>,                           // Var index -> puzzle var
}
impl<V: Eq + Hash + Clone> Registry<V> {
    pub fn var(&mut self, v: V) -> Var;                 // encode side
    pub fn decode(&self, lit: Lit) -> Option<(V, bool)>; // reverse side
}

pub trait Puzzle {
    type Var: Eq + Hash + Clone;           // e.g. Cell { r, c, val }
    type Cell;                             // per-cell display state

    /// Forward reduction: emit the *rules* as fixed CNF.
    fn encode_rules(&self, reg: &mut Registry<Self::Var>, cnf: &mut Cnf);

    /// Givens/edits become *assumptions*, not clauses (see §4).
    fn assumptions(&self, reg: &Registry<Self::Var>) -> Vec<Lit>;

    /// Backward reduction: turn a decoded solver view into puzzle state.
    fn project(&self, view: &SolverView<Self::Var>) -> Grid<Self::Cell>;
}
```

---

## 4. Key architectural decision: rules as CNF, givens as **assumptions**

Encode the puzzle's *rules* as fixed CNF, and feed the *givens/edits* as assumption literals rather
than unit clauses. Consequences that pay off across the whole project:

- **Editing is incremental** — changing a clue changes only the assumption set; no re-encoding, so
  the live editor stays cheap.
- **UNSAT cores are already puzzle-level** — an UNSAT result yields a core that is a subset of the
  givens, so "why is my puzzle broken" highlights the exact conflicting clues for free.
- **Backend-portable** — matches incremental SAT interfaces (`solve_under_assumptions`), so swapping
  in a faster solver later is clean.

---

## 5. Solver choice — two backends behind one trait

The hard constraint is *step through as the solver works*. No mature Rust solver exposes a
fine-grained event stream (decide → propagate → conflict → learn → backtrack); they expose
solve-to-completion plus hooks. So:

1. **Observable backend — a small hand-written CDCL (~600–900 lines) that we own.**
   The stepping UI drives this. Justified because the whole demo thesis *is* observability, puzzle
   instances are tiny (performance irrelevant), and we get to yield exactly the events we want.
   Structure it as a **non-blocking pumpable state machine**: `fn step(&mut self) -> Event` — never
   blocks, which is mandatory for WASM (single thread).

2. **Fast backend — pure-Rust via `rustsat` + BatSat.**
   RustSAT gives a unified `Solve` trait, cardinality encodings, learned-clause tracking, assumption
   propagation, and conflict/decision/propagation limits. **BatSat** is pure Rust and the
   recommended choice for WebAssembly. Used for "run to solution" and as an oracle to test the
   hand-written CDCL against.

**Not** splr as primary (capable pure-Rust CDCL but last released 2+ years ago and oriented at
solve-to-completion, so stepping fights it). CaDiCaL/Kissat via `rustsat-cadical`/`rustsat-kissat`
noted as optional **native-only** fast paths — C/C++, awkward for the browser.

Use rustsat's `Lit`/`Var`/`Cnf` types and encodings in the core (don't reinvent exactly-one), but
define our own `Solver` trait + `Event` enum so the stepping backend isn't constrained by rustsat's
solve-oriented interface.

**Prior art to read first:** Nathan Fenner's `sat_toasty_helper` builds pen-and-paper puzzle solving
on a Rust SAT solver using a custom `Prop` abstraction, with Sudoku set up via a `Num(pos, val)`
proposition — essentially the registry pattern here.

---

## 6. The stepping / event model

```rust
pub enum Event {
    Decide    { lit: Lit },
    Propagate { lit: Lit, reason: ClauseRef },
    Conflict  { clause: ClauseRef },
    Backtrack { to_level: u32 },
    Learn     { clause: Clause },
    Sat,
    Unsat     { core: Vec<Lit> },
}
```

The UI chooses granularity: single-event, run-to-next-decision, run-to-next-conflict, or
run-to-completion (just pump `step()` in a loop). Because everything decodes through the registry,
the app can render *any* event on the grid — a `Propagate` of `Cell{r,c,5}=true` lights that cell as
"solver forced this"; a `Conflict` flashes the involved cells.

Default granularity: **run-to-next-decision**, with single-propagation as an opt-in (raw
per-propagation stepping is mesmerizing for ten seconds and tedious after).

---

## 7. Puzzle choice

**Primary: Sudoku.** Ideal for *this specific* project:

- Clean encoding — 729 variables (`Cell{r,c,v}`), exactly-one per cell/row/col/box. Nothing but
  exactly-one, so the encodings module stays small.
- The reverse map is pedagogically perfect: level-0 unit propagation corresponds exactly to human
  naked/hidden singles. Placing a given and watching forced cells cascade **is** the bidirectionality
  thesis made visible — the solver explaining its deductions in the puzzle's own language.
- Editing is natural (place/erase givens); the UNSAT-core feature becomes "these clues contradict."

**Second puzzle to prove the abstraction:** **Light Up / Akari** (exercises cardinality beyond pure
exactly-one; visually distinct) — or **graph coloring** as the lower-effort validator. Ship Sudoku
first but stub the trait against the second early so no Sudoku assumptions leak into the core.

---

## 8. Center marks vs corner marks

Both mark styles read from **one** data source — the per-cell candidate status the registry already
provides. A mark is just a decoded literal (`Cell{r,c,v}` alive/dead). Two *styles* is a rendering
choice, not a second encoding. What gives them different meaning is binding each to a different
artifact:

- **Corner marks = possibility tier.** Candidates surviving BCP at the current node
  (`Cell{r,c,v}` not yet falsified). Cheap, always available, updates on every `Propagate`. This is
  the classic full-candidate-list convention.
- **Center marks = discovered-relationship / proven-elimination tier.** Two feeds were envisaged:
  - *Learned clauses*, filtered to short (binary/ternary), house-local clauses, decoded back through
    the registry (e.g. `¬Cell{A,3} ∨ ¬Cell{B,3}` → "A,B can't both be 3"). Hook: the `Learn` event.
  - *Failed-literal probing* — assume `Cell{r,c,v}=true`, run BCP, and if it conflicts, `v` is
    provably eliminated. Gives human-meaningful eliminations beyond naked singles.

**Post-implementation note (the learned-clause feed was dropped from the demo).** The honest caveat
below turned out to be fatal in practice for Sudoku, not merely noisy. Measured over a full stepped
solve of a hard puzzle, the hand-written CDCL learned 36 clauses whose *shortest* was 25 literals —
so a "keep only ≤3-literal, readable" filter kept **nothing**, ever, on any realistic board. A
25-literal disjunction isn't a relationship a human would learn from, so there was nothing worth
surfacing even with a looser filter. The demo's learned-clause overlay (the side panel *and* the
center-mark tint that read from it) was therefore removed as permanently-dead code. The `Learn`
event is still narrated in the Step view ("Learned a clause (N literals)") — it just isn't decoded
onto the grid. The probing feed (below) remains the sound route to *proven* eliminations.

Honest caveat (why the above happened): raw CDCL learned clauses are noisy — long and
path-dependent. For Sudoku's pairwise encoding they are consistently far longer than the
binary/ternary a person could read, so lighting center marks from `Learn` surfaces junk or,
after filtering, nothing at all. Prefer probing when the intent is *proven* eliminations.

The two tiers aren't mutually exclusive per digit — a candidate can be alive in corner *and* flagged
in center (part of a discovered pair, not yet eliminated), matching how people actually use the marks.

```rust
pub struct CellMarks {
    corner: BitSet9,   // BCP-surviving candidates
    center: BitSet9,   // probe-proven + filtered-learned candidates
}
```

`SolverView` must therefore expose, alongside the trail and level-0 fixed literals: a **candidate
lattice** (from BCP), the **learned-clause list**, and a **`probe(lit)`** method. `project()` splits
the decoded state into the two mark sets.

---

## 9. egui / WASM demo

- **eframe** targets web via wasm-bindgen; build with **trunk**. Single thread only (WASM has no easy
  threads) — which is *why* the solver is a non-blocking `step()` pump, not a blocking `solve()`. In
  `App::update`, run N steps/frame with a speed slider; "run" = large N.
- **Editing:** click a cell to enter/cycle a given; because givens are assumptions, an edit rebuilds
  the assumption vector and (optionally) re-solves incrementally. No re-encode.
- **Overlays** (each toggleable): proven cells (level-0), tentative cells (trail, dimmed), corner
  marks (BCP), center marks (probe-proven), conflict flash, UNSAT-core highlight. (A side panel
  listing learned clauses decoded into puzzle terms was planned and built, then removed — for Sudoku
  the clauses are far too long to read; see §8's post-implementation note.)

---

## 10. Milestones

1. Core types + registry + exactly-one; encode Sudoku rules; solve to completion via BatSat; prove
   round-trip (model → grid).
2. Hand-written CDCL with `step()`/`Event`; same puzzles solve; **verify against BatSat**.
3. Givens-as-assumptions + UNSAT-core extraction + core→clues highlight.
4. egui grid; editing; single-step + speed slider; level-0 "proven" overlay.
5. Candidate lattice + `probe`; corner + center marks; conflict overlay; second puzzle to validate
   the trait. (The learned-clause overlay was built here, then removed — see §8's note.)
6. WASM build via trunk; polish.

**Decide early (ripples widely):** adopt rustsat's `Lit`/`Cnf` types wholesale (recommended: yes);
default step granularity (recommended: run-to-next-decision).
