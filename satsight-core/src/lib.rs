//! SatSight core — the solver-agnostic half of a *bidirectional* SAT reduction.
//!
//! Solving a puzzle by reduction to SAT is routine in the forward direction. The
//! distinguishing idea of SatSight (see `satsight-plan.md`, §1) is the
//! **backward** direction: projecting what the solver discovers back into the
//! puzzle's own language. Both directions turn out to be one object viewed two
//! ways — a bijective [`Registry`] between puzzle propositions and SAT variables.
//!
//! This crate holds everything that does not depend on a particular puzzle or on
//! egui/wasm:
//!
//! - [`registry`] — the [`Registry`] bijection: **the bridge** both directions
//!   cross.
//! - [`cnf`] — the rustsat `Var`/`Lit`/`Clause`/`Cnf` vocabulary plus a fresh
//!   auxiliary-variable source.
//! - [`encodings`] — exactly-one / at-most-one (pairwise + sequential).
//! - [`solver`] — the [`Solver`] trait, [`Assignment`], and [`SolveOutcome`].
//! - [`backend_batsat`] — the fast BatSat backend and test oracle.
//! - [`cdcl`] — the observable hand-written CDCL: a non-blocking `step()` that
//!   yields one [`Event`](cdcl::Event) at a time (the stepping backend).
//! - [`propagate`] — the [`Propagator`]: BCP + failed-literal probing, the sound
//!   deduction engine behind the backward map.
//! - [`view`] — [`SolverView`], the decoded read model `project()` consumes.
//!
//! The richer [`SolverView`] artifacts (candidate lattice, learned clauses,
//! probing) arrive in later milestones.

pub mod backend_batsat;
pub mod cdcl;
pub mod cnf;
pub mod encodings;
pub mod propagate;
pub mod registry;
pub mod solver;
pub mod view;

pub use backend_batsat::BatSatBackend;
pub use cdcl::{Cdcl, ClauseRef, Event, Search};
pub use cnf::{clause, var_count, Clause, Cnf, Lit, Var, VarManager};
pub use propagate::{Propagation, Propagator};
pub use registry::Registry;
pub use solver::{Assignment, SolveOutcome, Solver};
pub use view::SolverView;
