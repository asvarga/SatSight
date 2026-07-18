# Documentation

The project wiki. Every page is plain Markdown under `docs/wiki/`; add a new
page as a `.md` file in this folder and link it here.

## Layout

```
satsight-core/     # Solver-agnostic core: registry, encodings, solver, propagation
satsight-puzzles/  # The Puzzle trait + Sudoku (built on satsight-core)
satsight-app/      # Demo frontend (egui/eframe + wasm in later milestones)
docs/wiki/         # This wiki (see satsight-plan.md for the full design)
bin/               # Repo scripts on PATH (e.g. `main`)
nix/               # Nix dev shell, loaded by direnv (.envrc)
```

## Start here

- **[Overview](index.md)** — this page.
- _Add your own pages and link them above._
