# CLAUDE.md

Guidance for Claude Code when working in this repository.

## Work modes

A prompt may select a work mode with `mode=<name>` (e.g. `mode=fast`). If no
mode is given, use `mode=default`.

**Always work in a dedicated git worktree — never check out a branch in the
main working directory.** Create a fresh worktree for every task (e.g. via the
`EnterWorktree` tool, or `git worktree add`) rather than `git checkout`/`git
switch`-ing branches in place. This keeps `main` clean and lets work proceed in
isolation.

| Mode        | Delivery                          | Questions                                                |
| ----------- | --------------------------------- | -------------------------------------------------------- |
| `default`   | Merge to `main` without asking    | Ask as necessary                                         |
| `fast`      | Merge to `main` without asking    | Don't stop — file them as GitHub issues to discuss later |
| `pr`        | Cut a PR                          | Ask as necessary                                         |
| `fast+pr`   | Cut a PR                          | Don't stop — leave them as comments on the PR            |

Details:

- **`mode=default`** — The default. Work in a worktree, then merge to `main`
  without asking. Resolve any merge conflicts as necessary. Ask questions as
  necessary.
- **`mode=fast`** — Like `default`, but don't stop to ask questions. File them
  as GitHub issues to discuss later, labeled `question`.
- **`mode=pr`** — Work in a worktree and cut a PR. Ask questions as necessary.
- **`mode=fast+pr`** — Work in a worktree and cut a PR. Don't stop to ask
  questions; put them in the PR as comments.

## What this is

SatSight: a library for solving puzzles by reduction to SAT that can also project
solver discoveries *back* into the puzzle's language, plus an egui/eframe demo.
The full design is in `docs/wiki/satsight-plan.md` — treat it as the spec. The
crate boundaries keep dependency edges pointing one way (`app → puzzles → core`),
so the core stays solver-agnostic and frontend-free — respect this when adding
code.

## Crates

| Crate              | Role                                                                                   |
| ------------------ | ------------------------------------------------------------------------------------- |
| `satsight-core`    | Solver-agnostic core: the registry (the bridge), CNF/encodings, the `Solver` trait, a BatSat backend, and the propagation/probing deduction engine. No egui, no wasm. |
| `satsight-puzzles` | The `Puzzle` trait plus concrete puzzles (Sudoku primary). Built on `satsight-core`.  |
| `satsight-app`     | The demo frontend — an eframe/egui app (renders the grid, edits givens, shows deductions, the full solve, and the observable CDCL stepped one event at a time). Runs natively and in the browser (wasm via trunk). |

## Commands

```sh
cargo test --workspace                  # build + test everything (what CI runs)
cargo fmt --all -- --check              # formatting (CI-enforced)
cargo clippy --workspace --all-targets  # lints
main                                     # run the app natively (hot-reloads on change)
bin/main                                 # …same, by path, from any cwd
cargo run -p satsight-app                # …or run the app once, directly
web                                      # serve the app in the browser (trunk, live-reload)
bin/web                                  # …same, by path, from any cwd
```

`bin/main` runs the native app under `cargo watch`, rebuilding + relaunching on
source changes. `RELEASE=1` builds optimized; `NO_WATCH=1` runs once without
watching.

`bin/web` serves the wasm build via `trunk serve` (plan §9) with live reload;
`RELEASE=1` runs `wasm-opt` for a smaller bundle, `BUILD=1` does a one-shot build
into `satsight-app/dist`. Trunk must run from `satsight-app/` (it resolves
`index.html`'s manifest relative to its cwd); the launcher handles that. Needs
`trunk` and the `wasm32-unknown-unknown` target (both in the nix dev shell).

With the nix dev shell + direnv active (`.envrc`), `./bin` is on `PATH`, so the
launchers are just `main` and `web`.

## Conventions

- **Toolchain**: stable Rust, edition 2021 (`rust-toolchain.toml`). The nix dev
  shell (`nix/flake.nix`) provides `cargo-watch` and friends; direnv loads it via
  `.envrc`.
- **Warnings are errors in CI** (`RUSTFLAGS: -D warnings`). CI runs `fmt
  --check`, `clippy --workspace --all-targets`, and `test --workspace` on every
  push/PR — make all three clean before merging or cutting a PR.
- **Lints**: `clippy::pedantic` is on (`warn`) with a small allow-list in the
  root `Cargo.toml`; match the existing style rather than fighting it.
