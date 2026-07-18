# Documentation

The project wiki. Every page is plain Markdown under `docs/wiki/`; add a new
page as a `.md` file in this folder and link it here.

## Layout

```
template-core/   # Core library — reusable, frontend-free logic
template-gui/    # egui/eframe desktop frontend, built on template-core
docs/wiki/       # This wiki
bin/             # Repo scripts on PATH (e.g. `main`)
nix/             # Nix dev shell, loaded by direnv (.envrc)
```

## Start here

- **[Overview](index.md)** — this page.
- _Add your own pages and link them above._
