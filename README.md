# SatSight

A Rust library for solving puzzles by reduction to SAT — with the twist that
solver discoveries can be projected **back** into the puzzle's own language. Ships
with a demo (an `egui` app in later milestones) that steps the solver and shows
what it learns.

The full design is in [`docs/wiki/satsight-plan.md`](docs/wiki/satsight-plan.md).

## The idea

"Bidirectional reduction" is really *one object viewed two ways*: a
[`Registry`](satsight-core/src/registry.rs) that is a bijection between
puzzle-level propositions (e.g. Sudoku's `Cell { r, c, v }`) and SAT variables.
Every **forward** step (encoding rules and givens into SAT) and every
**backward** step (interpreting what the solver found) routes through it.

The backward direction is the interesting part: "what the solver discovered" is a
*family* of artifacts, each decoding to a different puzzle-level meaning — forced
facts, still-possible candidates, discovered relationships, contradictory clues.

## Workspace

| Crate              | Role                                                                        |
| ------------------ | -------------------------------------------------------------------------- |
| `satsight-core`    | Solver-agnostic: the registry, CNF/encodings, the `Solver` trait, a BatSat backend, and the propagation/probing deduction engine. No egui, no wasm. |
| `satsight-puzzles` | The `Puzzle` trait plus Sudoku (primary). Proves the abstraction isn't Sudoku-shaped. |
| `satsight-app`     | The demo frontend (a CLI placeholder today; eframe/egui + wasm in later milestones). |

Dependency edges point one way: `app → puzzles → core`.

## Try it

```sh
cargo run -p satsight-app     # solve a Sudoku by pure logic, then by full search
cargo test --workspace        # build + test everything (what CI runs)
cargo fmt --all -- --check    # formatting (CI-enforced)
cargo clippy --workspace --all-targets
```

`cargo run` prints the given puzzle, then how far **pure logic** gets before any
search — unit propagation plus failed-literal probing, with every deduction
phrased in the puzzle's own terms ("30 givens + 51 deduced = 81/81 cells; 648
candidate eliminations proven") — then the full BatSat solution for comparison.

## Deduction engine (the backward map, today)

[`satsight_puzzles::deduce`](satsight-puzzles/src/puzzle.rs) solves a puzzle by
*sound, search-free* inference and reports the result in puzzle language:

- **Unit propagation (BCP)** — forced facts (naked/hidden singles). Everything it
  derives is entailed, so it holds in every solution.
- **Failed-literal probing** — assume a candidate, propagate, and if it conflicts
  the candidate is *provably* eliminated.

It is generic over the `Puzzle` trait, and its results are checked for soundness
against the BatSat backend in the tests. This is the same propagation core the
hand-written, steppable CDCL will build on (milestone 2).

## Status

Milestone 1 (workspace, registry, encodings, Sudoku forward path + round trip) is
complete; the deduction engine above is an early slice of the backward map from
milestones 3/5. Remaining: the observable CDCL, the egui grid + stepping, marks
and overlays, a second puzzle, and the wasm build. See the plan for details.
