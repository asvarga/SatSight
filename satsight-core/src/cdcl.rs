//! The observable backend: a small, hand-written CDCL we own (plan §5, §6).
//!
//! No mature Rust SAT solver exposes a fine-grained event stream — they solve to
//! completion and, at best, offer callbacks. But the whole SatSight thesis *is*
//! observability: watch the solver decide, propagate, conflict, learn, and
//! backtrack, and render each move on the grid through the
//! [`Registry`](crate::registry::Registry). So we hand-write a conflict-driven
//! clause-learning (CDCL) solver whose distinguishing feature is a **non-blocking
//! [`step`](Search::step)**: each call advances the search by exactly one
//! [`Event`] and returns — never looping to completion, which is what makes it
//! safe to drive one frame at a time (and, later, from a single WASM thread).
//!
//! Two objects:
//!
//! - [`Cdcl`] holds the fixed rule CNF (loaded once; plan §4) and mints a fresh
//!   [`Search`] per solve. It also implements the [`Solver`] trait by pumping a
//!   search to completion — so it doubles as a normal "run to solution" backend
//!   and can be cross-checked against [`BatSatBackend`](crate::backend_batsat).
//! - [`Search`] is the steppable state machine: the trail, the clause database
//!   (rules plus learned clauses), and the assignment. Drive it with
//!   [`step`](Search::step), or [`run`](Search::run) to completion.
//!
//! Propagation reuses the occurrence-list idea from
//! [`propagate`](crate::propagate): assigning a literal only re-examines the
//! clauses that mention its variable. Conflict analysis is textbook first-UIP
//! learning; givens arrive as **assumptions** (decisions at levels `1..=k`), so an
//! UNSAT result yields a core over those assumption literals (plan §4).

use crate::cnf::{clause, var_count, Clause, Cnf, Lit, Var};
use crate::solver::{Assignment, SolveOutcome, Solver};

/// Index of a clause in a [`Search`]'s clause database (rules first, then learned
/// clauses appended in discovery order).
pub type ClauseRef = usize;

/// One observable move of the solver (plan §6).
///
/// Because every literal decodes through the registry, the frontend can render
/// *any* of these on the grid: a `Propagate` of `Cell{r,c,5}` lights that cell as
/// "solver forced this", a `Conflict` flashes the clause's cells, and so on.
#[derive(Debug, Clone)]
pub enum Event {
    /// A branching choice: `lit` was assumed to open a new decision level. Givens
    /// (assumptions) surface as the first decisions.
    Decide { lit: Lit },
    /// Unit propagation forced `lit`; `reason` is the clause that became unit.
    Propagate { lit: Lit, reason: ClauseRef },
    /// Every literal of `clause` is false under the current assignment.
    Conflict { clause: ClauseRef },
    /// The search unwound to `to_level`, undoing every later assignment.
    Backtrack { to_level: u32 },
    /// A new clause was derived from a conflict (first-UIP) and added.
    Learn { clause: Clause },
    /// Satisfiable: every variable is assigned. Terminal and sticky.
    Sat,
    /// Unsatisfiable under the assumptions; `core` is a subset of them (the
    /// conflicting givens; plan §4). Terminal and sticky.
    Unsat { core: Vec<Lit> },
}

/// A CDCL solver over a fixed rule CNF (plan §5's observable backend).
///
/// Load the rules once, then spawn a [`Search`] per set of assumptions.
#[derive(Debug, Clone, Default)]
pub struct Cdcl {
    /// Rule clauses as literal lists. Never change as the user edits (plan §4).
    rules: Vec<Vec<Lit>>,
    /// Number of variables the rules span; a search grows this to also cover any
    /// assumption variables it is handed.
    n_vars: usize,
}

impl Cdcl {
    /// A solver with no rules loaded.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A solver with `cnf` already installed as its rules.
    #[must_use]
    pub fn from_cnf(cnf: &Cnf) -> Self {
        let mut solver = Self::new();
        solver.load_rules(cnf);
        solver
    }

    /// Begin a fresh, steppable search under `assumptions`.
    #[must_use]
    pub fn search(&self, assumptions: &[Lit]) -> Search {
        Search::new(self.rules.clone(), self.n_vars, assumptions)
    }
}

impl Solver for Cdcl {
    fn load_rules(&mut self, cnf: &Cnf) {
        self.n_vars = var_count(cnf);
        self.rules = cnf.iter().map(|cl| cl.iter().copied().collect()).collect();
    }

    fn solve(&mut self, assumptions: &[Lit]) -> SolveOutcome {
        self.search(assumptions).run()
    }
}

/// What examining one clause under the current assignment reveals.
enum ClauseStatus {
    /// Satisfied, or still has ≥2 unassigned literals — nothing to do.
    Idle,
    /// Every literal is false — a conflict.
    Conflict,
    /// Exactly one literal is unassigned and the rest are false — it is forced.
    Unit(Lit),
}

/// Where the state machine is between [`step`](Search::step) calls.
enum Stage {
    /// Draining the propagation worklist; on fixpoint, decide or declare SAT.
    Propagate,
    /// A conflict clause was just reported; next step analyzes it.
    Analyze(ClauseRef),
    /// A clause was learned; next step unwinds to `to_level` and re-propagates.
    Backtrack { learned: ClauseRef, to_level: u32 },
    /// Terminal outcome; every further step re-emits it.
    Done(Final),
}

/// A terminal search result (mirrors [`SolveOutcome`], kept inside [`Stage`]).
enum Final {
    Sat,
    Unsat(Vec<Lit>),
}

/// A steppable CDCL search over a fixed clause set under fixed assumptions.
///
/// Owns its own copy of the clauses so learned clauses can be appended without
/// disturbing the parent [`Cdcl`], and so several searches can run independently.
/// For the tiny instances SatSight targets, the copy is negligible.
pub struct Search {
    /// Rule clauses followed by learned clauses, as literal lists.
    clauses: Vec<Vec<Lit>>,
    /// Number of leading clauses that are original rules (the rest are learned).
    n_rules: usize,
    /// `occ[var]` = indices of clauses mentioning that variable.
    occ: Vec<Vec<ClauseRef>>,
    /// Number of variables (rules plus assumptions).
    n_vars: usize,

    /// Current truth value per variable (`None` = unassigned).
    value: Vec<Option<bool>>,
    /// Decision level at which each variable was assigned (`0` if unassigned).
    level: Vec<u32>,
    /// Reason clause per assigned variable (`None` = a decision/assumption).
    reason: Vec<Option<ClauseRef>>,
    /// Whether a variable was assigned as an assumption decision.
    is_assumption: Vec<bool>,

    /// Assignment stack in chronological order.
    trail: Vec<Lit>,
    /// `trail_lim[i]` is the trail length when decision level `i + 1` opened; its
    /// length is the current decision level.
    trail_lim: Vec<usize>,
    /// Clause indices still needing examination for units/conflicts.
    dirty: Vec<ClauseRef>,

    /// The givens/edits as assumption literals, placed as the first decisions.
    assumptions: Vec<Lit>,

    /// The state-machine cursor.
    stage: Stage,
}

/// Build the [`Var`] for a 0-based index.
fn var_of(i: usize) -> Var {
    Var::new(u32::try_from(i).expect("variable index fits in u32"))
}

impl Search {
    /// Start a search over `rules` (spanning at least `rule_n_vars` variables)
    /// under `assumptions`.
    fn new(rules: Vec<Vec<Lit>>, rule_n_vars: usize, assumptions: &[Lit]) -> Self {
        let n_vars = rules
            .iter()
            .flat_map(|cl| cl.iter())
            .chain(assumptions.iter())
            .map(|lit| lit.var().idx() + 1)
            .max()
            .unwrap_or(0)
            .max(rule_n_vars);

        let n_rules = rules.len();
        let mut occ = vec![Vec::new(); n_vars];
        for (ci, cl) in rules.iter().enumerate() {
            for lit in cl {
                occ[lit.var().idx()].push(ci);
            }
        }
        // Seed every clause so unit clauses already present in the rules propagate
        // at level 0 before any decision is made.
        let dirty = (0..rules.len()).collect();

        Self {
            clauses: rules,
            n_rules,
            occ,
            n_vars,
            value: vec![None; n_vars],
            level: vec![0; n_vars],
            reason: vec![None; n_vars],
            is_assumption: vec![false; n_vars],
            trail: Vec::new(),
            trail_lim: Vec::new(),
            dirty,
            assumptions: assumptions.to_vec(),
            stage: Stage::Propagate,
        }
    }

    /// The current decision level (number of open decisions).
    #[must_use]
    pub fn decision_level(&self) -> u32 {
        u32::try_from(self.trail_lim.len()).expect("decision level fits in u32")
    }

    /// The deepest decision level currently occupied by a given (assumption
    /// literal), or `0` if none is placed as a decision.
    ///
    /// This is the boundary between the two kinds of mid-search fact (plan §1):
    /// an assignment at or below it is entailed by the givens alone via
    /// propagation — a **proven** fact that holds in every solution consistent
    /// with the clues — while an assignment above it is contingent on a
    /// branching guess and may be undone on backtrack — a **hypothetical** one.
    /// Assumptions always occupy the lowest levels (they are placed before any
    /// branch and re-placed first after each backtrack), so the split is a clean
    /// threshold on [`level_of`](Search::level_of).
    #[must_use]
    pub fn base_level(&self) -> u32 {
        self.is_assumption
            .iter()
            .enumerate()
            .filter(|&(_, &assumed)| assumed)
            .map(|(i, _)| self.level[i])
            .max()
            .unwrap_or(0)
    }

    /// The decision level at which `var` currently holds a value, or `None` if it
    /// is unassigned. Compare against [`base_level`](Search::base_level) to tell a
    /// proven assignment from a hypothetical one.
    #[must_use]
    pub fn level_of(&self, var: Var) -> Option<u32> {
        let i = var.idx();
        self.value.get(i).copied().flatten().map(|_| self.level[i])
    }

    /// The assignment stack in the order literals were set — the tentative
    /// puzzle state mid-search (plan §1), for the trail overlay.
    #[must_use]
    pub fn trail(&self) -> &[Lit] {
        &self.trail
    }

    /// The truth value currently assigned to `var`, or `None` if unassigned.
    #[must_use]
    pub fn value_of(&self, var: Var) -> Option<bool> {
        self.value.get(var.idx()).copied().flatten()
    }

    /// The literals of clause `cref` (a reason or learned clause), for decoding an
    /// [`Event`] back to puzzle terms.
    #[must_use]
    pub fn clause_lits(&self, cref: ClauseRef) -> &[Lit] {
        &self.clauses[cref]
    }

    /// Whether `cref` names a learned clause (rather than an original rule).
    #[must_use]
    pub fn is_learned(&self, cref: ClauseRef) -> bool {
        cref >= self.n_rules
    }

    /// Read-only failed-literal probe over the **current** partial assignment:
    /// would asserting `lit` now force an immediate BCP conflict?
    ///
    /// Does not mutate the search — it copies the current values, tentatively sets
    /// `lit`, and runs unit propagation to a fixpoint. Because it starts from the
    /// live trail (branching guesses included), a `true` result means `lit` is
    /// refuted *given the current tentative state*, not necessarily in every
    /// solution — the mid-search analogue of [`Propagator::probe`], which the
    /// [`SolverView`](crate::view::SolverView) surfaces as its `probe` hook. An
    /// already-assigned literal is refuted exactly when it disagrees with its
    /// current value.
    #[must_use]
    pub fn probe(&self, lit: Lit) -> bool {
        let mut values = self.value.clone();
        let idx = lit.var().idx();
        match values[idx] {
            Some(v) => return v != lit.is_pos(),
            None => values[idx] = Some(lit.is_pos()),
        }
        let mut worklist: Vec<ClauseRef> = self.occ[idx].clone();
        while let Some(ci) = worklist.pop() {
            let mut unit = None;
            let mut unassigned = 0u32;
            let mut satisfied = false;
            for &l in &self.clauses[ci] {
                match values[l.var().idx()] {
                    Some(v) if v == l.is_pos() => {
                        satisfied = true;
                        break;
                    }
                    Some(_) => {}
                    None => {
                        unassigned += 1;
                        unit = Some(l);
                    }
                }
            }
            if satisfied {
                continue;
            }
            match unit {
                Some(u) if unassigned == 1 => {
                    values[u.var().idx()] = Some(u.is_pos());
                    worklist.extend(self.occ[u.var().idx()].iter().copied());
                }
                // Every literal false (unassigned == 0), or ≥2 unassigned: the
                // latter cannot be a conflict, so only the former ends the probe.
                _ if unassigned == 0 => return true,
                _ => {}
            }
        }
        false
    }

    /// Whether the search has reached a terminal SAT/UNSAT state.
    #[must_use]
    pub fn is_done(&self) -> bool {
        matches!(self.stage, Stage::Done(_))
    }

    /// The current (possibly partial) assignment as an [`Assignment`], for
    /// projecting the trail or a completed model through the registry.
    #[must_use]
    pub fn assignment(&self) -> Assignment {
        let mut model = Assignment::with_capacity(self.n_vars);
        for (i, value) in self.value.iter().enumerate() {
            if let Some(b) = value {
                model.set(var_of(i), *b);
            }
        }
        model
    }

    /// Advance the search by exactly one [`Event`] and return it.
    ///
    /// Never blocks: pump it in a loop (see [`run`](Search::run)) for a full
    /// solve, or one event per frame for the stepping UI. Once terminal, every
    /// call re-emits the same `Sat`/`Unsat`.
    pub fn step(&mut self) -> Event {
        match &self.stage {
            Stage::Done(Final::Sat) => return Event::Sat,
            Stage::Done(Final::Unsat(core)) => return Event::Unsat { core: core.clone() },
            _ => {}
        }
        match self.stage {
            Stage::Propagate => self.step_propagate(),
            Stage::Analyze(cref) => self.step_analyze(cref),
            Stage::Backtrack { learned, to_level } => self.step_backtrack(learned, to_level),
            Stage::Done(_) => unreachable!("terminal stages handled above"),
        }
    }

    /// Drive the search to completion and return its outcome.
    #[must_use]
    pub fn run(&mut self) -> SolveOutcome {
        loop {
            match self.step() {
                Event::Sat => return SolveOutcome::Sat(self.assignment()),
                Event::Unsat { core } => return SolveOutcome::Unsat(core),
                _ => {}
            }
        }
    }

    /// The truth value of `lit` under the current assignment.
    fn lit_value(&self, lit: Lit) -> Option<bool> {
        self.value[lit.var().idx()].map(|b| if lit.is_pos() { b } else { !b })
    }

    /// Record `lit` true, at the current level, with the given reason, and queue
    /// the clauses its variable touches for re-examination.
    fn assign(&mut self, lit: Lit, reason: Option<ClauseRef>, is_assumption: bool) {
        let i = lit.var().idx();
        self.value[i] = Some(lit.is_pos());
        self.level[i] = self.decision_level();
        self.reason[i] = reason;
        self.is_assumption[i] = is_assumption;
        self.trail.push(lit);
        for k in 0..self.occ[i].len() {
            let ci = self.occ[i][k];
            self.dirty.push(ci);
        }
    }

    /// Classify clause `ci` under the current assignment.
    fn examine(&self, ci: ClauseRef) -> ClauseStatus {
        let mut unit = None;
        let mut unassigned = 0u32;
        for &lit in &self.clauses[ci] {
            match self.value[lit.var().idx()] {
                Some(v) if v == lit.is_pos() => return ClauseStatus::Idle, // satisfied
                Some(_) => {}                                              // falsified
                None => {
                    unassigned += 1;
                    if unassigned > 1 {
                        return ClauseStatus::Idle;
                    }
                    unit = Some(lit);
                }
            }
        }
        match unit {
            Some(lit) if unassigned == 1 => ClauseStatus::Unit(lit),
            _ => ClauseStatus::Conflict,
        }
    }

    /// Drain the worklist for the next unit or conflict; on fixpoint, decide.
    fn step_propagate(&mut self) -> Event {
        while let Some(ci) = self.dirty.pop() {
            match self.examine(ci) {
                ClauseStatus::Idle => {}
                ClauseStatus::Conflict => {
                    self.stage = Stage::Analyze(ci);
                    return Event::Conflict { clause: ci };
                }
                ClauseStatus::Unit(lit) => {
                    self.assign(lit, Some(ci), false);
                    return Event::Propagate { lit, reason: ci };
                }
            }
        }
        self.decide()
    }

    /// Propagation reached a fixpoint: place the next assumption, branch on a free
    /// variable, or declare SAT.
    fn decide(&mut self) -> Event {
        // Assumptions are the first decisions, in order (plan §4). Re-scanned from
        // the start each time so a backtrack that unwinds an assumption simply
        // re-places it on the next decision.
        for idx in 0..self.assumptions.len() {
            let a = self.assumptions[idx];
            match self.lit_value(a) {
                Some(true) => {} // already satisfied
                Some(false) => {
                    // The givens are inconsistent: `a` is provably false.
                    let core = self.core_for_failed_assumption(a);
                    self.stage = Stage::Done(Final::Unsat(core.clone()));
                    return Event::Unsat { core };
                }
                None => {
                    self.trail_lim.push(self.trail.len());
                    self.assign(a, None, true);
                    return Event::Decide { lit: a };
                }
            }
        }
        // All assumptions hold; branch on the lowest-index free variable.
        if let Some(lit) = self.pick_branch() {
            self.trail_lim.push(self.trail.len());
            self.assign(lit, None, false);
            Event::Decide { lit }
        } else {
            self.stage = Stage::Done(Final::Sat);
            Event::Sat
        }
    }

    /// The lowest-index unassigned variable as a positive literal, or `None` when
    /// every variable is assigned.
    fn pick_branch(&self) -> Option<Lit> {
        self.value
            .iter()
            .position(Option::is_none)
            .map(|i| var_of(i).pos_lit())
    }

    /// Analyze the reported conflict: at level 0 it is terminal UNSAT; otherwise
    /// derive a first-UIP clause, add it, and queue a backtrack.
    fn step_analyze(&mut self, cref: ClauseRef) -> Event {
        if self.decision_level() == 0 {
            let seed: Vec<Lit> = self.clauses[cref].clone();
            let core = self.analyze_final(&seed);
            self.stage = Stage::Done(Final::Unsat(core.clone()));
            return Event::Unsat { core };
        }
        let (learned, to_level) = self.analyze(cref);
        let learned_ref = self.add_learned(&learned);
        self.stage = Stage::Backtrack {
            learned: learned_ref,
            to_level,
        };
        Event::Learn {
            clause: clause(learned.iter().copied()),
        }
    }

    /// Unwind to `to_level` and re-arm propagation from the asserting clause.
    fn step_backtrack(&mut self, learned: ClauseRef, to_level: u32) -> Event {
        self.backtrack_to(to_level);
        // After a conflict-driven backtrack the learned clause is the only clause
        // guaranteed newly unit; seed the worklist with just it, and its asserting
        // literal propagates on the next step.
        self.dirty.clear();
        self.dirty.push(learned);
        self.stage = Stage::Propagate;
        Event::Backtrack { to_level }
    }

    /// First-UIP conflict analysis: returns the learned clause (asserting literal
    /// first) and the level to backtrack to.
    fn analyze(&self, conflict: ClauseRef) -> (Vec<Lit>, u32) {
        let dl = self.decision_level();
        let mut seen = vec![false; self.n_vars];
        // Literals of the learned clause that sit below the current level.
        let mut others: Vec<Lit> = Vec::new();
        // Count of not-yet-resolved literals at the current level.
        let mut pending = 0i32;
        let mut pivot: Option<Lit> = None;
        let mut cref = conflict;
        let mut index = self.trail.len();

        loop {
            for &q in &self.clauses[cref] {
                if let Some(p) = pivot {
                    if q.var() == p.var() {
                        continue;
                    }
                }
                let j = q.var().idx();
                if !seen[j] && self.level[j] > 0 {
                    seen[j] = true;
                    if self.level[j] >= dl {
                        pending += 1;
                    } else {
                        others.push(q);
                    }
                }
            }
            // Walk down the trail to the next resolved (seen) literal.
            loop {
                index -= 1;
                if seen[self.trail[index].var().idx()] {
                    break;
                }
            }
            let resolved = self.trail[index];
            pivot = Some(resolved);
            seen[resolved.var().idx()] = false;
            pending -= 1;
            if pending <= 0 {
                break;
            }
            cref = self.reason[resolved.var().idx()]
                .expect("a resolved non-UIP literal was propagated, so has a reason");
        }

        let uip = pivot.expect("analysis resolves to a unique implication point");
        let mut learned = Vec::with_capacity(others.len() + 1);
        learned.push(!uip);
        learned.extend(others);
        let to_level = learned[1..]
            .iter()
            .map(|lit| self.level[lit.var().idx()])
            .max()
            .unwrap_or(0);
        (learned, to_level)
    }

    /// Append a learned clause and index its variables' occurrences.
    fn add_learned(&mut self, lits: &[Lit]) -> ClauseRef {
        let cref = self.clauses.len();
        for lit in lits {
            self.occ[lit.var().idx()].push(cref);
        }
        self.clauses.push(lits.to_vec());
        cref
    }

    /// Undo every assignment made above decision level `level`.
    fn backtrack_to(&mut self, level: u32) {
        while self.decision_level() > level {
            let lim = self.trail_lim.pop().expect("a level to unwind");
            while self.trail.len() > lim {
                let lit = self.trail.pop().expect("a literal to unwind");
                let i = lit.var().idx();
                self.value[i] = None;
                self.level[i] = 0;
                self.reason[i] = None;
                self.is_assumption[i] = false;
            }
        }
    }

    /// Collect the assumption literals responsible for a failed assumption `a`
    /// (assigned false by earlier assumptions), plus `a` itself.
    fn core_for_failed_assumption(&self, a: Lit) -> Vec<Lit> {
        let mut core = self.analyze_final(&[a]);
        if !core.iter().any(|lit| lit.var() == a.var()) {
            core.push(a);
        }
        core
    }

    /// Trace the reason graph from `seeds` back to the assumption literals that
    /// entail them — the UNSAT core over the assumptions (plan §4).
    ///
    /// Best-effort: correct for the common cases (a directly contradicted given, a
    /// shallow conflict among givens); it can under-report in deep conflicts whose
    /// assumptions were already unwound, where it returns whatever assumptions
    /// remain reachable. The verdict is always sound; the app uses BatSat's core
    /// when a minimal core matters.
    fn analyze_final(&self, seeds: &[Lit]) -> Vec<Lit> {
        let mut seen = vec![false; self.n_vars];
        let mut stack: Vec<usize> = Vec::new();
        for &lit in seeds {
            let i = lit.var().idx();
            if self.value[i].is_some() && !seen[i] {
                seen[i] = true;
                stack.push(i);
            }
        }
        let mut core = Vec::new();
        while let Some(i) = stack.pop() {
            if self.is_assumption[i] {
                let value = self.value[i].expect("an assumption variable is assigned");
                core.push(if value {
                    var_of(i).pos_lit()
                } else {
                    var_of(i).neg_lit()
                });
            } else if let Some(cref) = self.reason[i] {
                for &q in &self.clauses[cref] {
                    let j = q.var().idx();
                    if j != i && self.value[j].is_some() && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        core
    }
}

#[cfg(test)]
mod tests {
    use super::{Cdcl, Event, Search};
    use crate::backend_batsat::BatSatBackend;
    use crate::cnf::{clause, Cnf, Lit, Var};
    use crate::solver::{SolveOutcome, Solver};

    /// Whether an assignment satisfies every clause of a CNF given as literal
    /// lists.
    fn satisfies(model: &crate::solver::Assignment, clauses: &[Vec<Lit>]) -> bool {
        clauses
            .iter()
            .all(|cl| cl.iter().any(|&lit| model.lit_value(lit) == Some(true)))
    }

    /// A tiny deterministic PRNG (xorshift) so the fuzz test needs no rng crate.
    struct Rng(u64);
    impl Rng {
        fn bits(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        /// A value in `0..n`.
        fn below(&mut self, n: usize) -> usize {
            let n = u64::try_from(n).expect("bound fits in u64");
            usize::try_from(self.bits() % n).expect("remainder fits in usize")
        }
        /// A fair coin.
        fn coin(&mut self) -> bool {
            self.bits() & 1 == 0
        }
        /// A random variable in `0..n_vars`.
        fn var(&mut self, n_vars: usize) -> Var {
            Var::new(u32::try_from(self.below(n_vars)).expect("variable index fits in u32"))
        }
        /// A random literal over `0..n_vars`.
        fn lit(&mut self, n_vars: usize) -> Lit {
            let var = self.var(n_vars);
            if self.coin() {
                var.pos_lit()
            } else {
                var.neg_lit()
            }
        }
    }

    #[test]
    fn solves_a_unit_chain() {
        // (a) ∧ (¬a ∨ b) ∧ (¬b ∨ c): a, b, c all forced true.
        let (a, b, c) = (Var::new(0), Var::new(1), Var::new(2));
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.pos_lit()]));
        cnf.add_clause(clause([a.neg_lit(), b.pos_lit()]));
        cnf.add_clause(clause([b.neg_lit(), c.pos_lit()]));
        let mut solver = Cdcl::from_cnf(&cnf);
        let SolveOutcome::Sat(model) = solver.solve(&[]) else {
            panic!("chain is satisfiable");
        };
        assert_eq!(model.var_value(a), Some(true));
        assert_eq!(model.var_value(b), Some(true));
        assert_eq!(model.var_value(c), Some(true));
    }

    #[test]
    fn detects_a_direct_contradiction() {
        // (a) ∧ (¬a) is UNSAT with no assumptions, so an empty core.
        let a = Var::new(0);
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.pos_lit()]));
        cnf.add_clause(clause([a.neg_lit()]));
        let mut solver = Cdcl::from_cnf(&cnf);
        let SolveOutcome::Unsat(core) = solver.solve(&[]) else {
            panic!("directly contradictory");
        };
        assert!(
            core.is_empty(),
            "rules alone are UNSAT; no assumptions needed"
        );
    }

    #[test]
    fn unsat_core_pins_the_conflicting_assumptions() {
        // (¬a ∨ ¬b): satisfiable, but asserting both a and b is not.
        let (a, b) = (Var::new(0), Var::new(1));
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.neg_lit(), b.neg_lit()]));
        let mut solver = Cdcl::from_cnf(&cnf);
        let SolveOutcome::Unsat(core) = solver.solve(&[a.pos_lit(), b.pos_lit()]) else {
            panic!("a ∧ b contradicts the clause");
        };
        // The core is a subset of the assumptions and names both culprits.
        assert!(core.iter().all(|l| *l == a.pos_lit() || *l == b.pos_lit()));
        assert!(core.contains(&a.pos_lit()));
        assert!(core.contains(&b.pos_lit()));
        // Dropping either assumption makes it satisfiable again.
        assert!(matches!(solver.solve(&[a.pos_lit()]), SolveOutcome::Sat(_)));
    }

    #[test]
    fn agrees_with_batsat_on_random_instances() {
        // The soundness anchor: over many random 3-CNFs the hand-written CDCL and
        // BatSat must agree on SAT/UNSAT, and every model we report must hold.
        let mut rng = Rng(0x5EED);
        let mut sat = 0;
        let mut unsat = 0;
        for _ in 0..600 {
            let n_vars = 3 + rng.below(6); // 3..=8 variables
            let n_clauses = 3 + rng.below(20);
            let mut clauses: Vec<Vec<Lit>> = Vec::new();
            let mut cnf = Cnf::new();
            for _ in 0..n_clauses {
                let cl: Vec<Lit> = (0..3).map(|_| rng.lit(n_vars)).collect();
                cnf.add_clause(clause(cl.iter().copied()));
                clauses.push(cl);
            }

            let mut solver = Cdcl::from_cnf(&cnf);
            let ours = solver.solve(&[]);
            let theirs = BatSatBackend::solve_cnf(&cnf, &[]);
            match (&ours, &theirs) {
                (SolveOutcome::Sat(model), SolveOutcome::Sat(_)) => {
                    assert!(satisfies(model, &clauses), "our model must satisfy the CNF");
                    sat += 1;
                }
                (SolveOutcome::Unsat(_), SolveOutcome::Unsat(_)) => unsat += 1,
                _ => panic!("CDCL and BatSat disagree on satisfiability"),
            }
        }
        // The generator should exercise both verdicts.
        assert!(
            sat > 0 && unsat > 0,
            "fuzz saw only one verdict (sat={sat}, unsat={unsat})"
        );
    }

    #[test]
    fn agrees_with_batsat_under_random_assumptions() {
        // Same anchor, now exercising the assumption path and core extraction.
        let mut rng = Rng(0x00C0_FFEE);
        for _ in 0..400 {
            let n_vars = 3 + rng.below(5); // 3..=7
            let n_clauses = 2 + rng.below(12);
            let mut clauses: Vec<Vec<Lit>> = Vec::new();
            let mut cnf = Cnf::new();
            for _ in 0..n_clauses {
                let cl: Vec<Lit> = (0..3).map(|_| rng.lit(n_vars)).collect();
                cnf.add_clause(clause(cl.iter().copied()));
                clauses.push(cl);
            }
            // A random handful of assumptions.
            let mut assumptions = Vec::new();
            for v in 0..n_vars {
                if rng.below(3) == 0 {
                    let var = Var::new(u32::try_from(v).expect("index fits"));
                    assumptions.push(if rng.coin() {
                        var.pos_lit()
                    } else {
                        var.neg_lit()
                    });
                }
            }

            let mut solver = Cdcl::from_cnf(&cnf);
            let ours = solver.solve(&assumptions);
            let theirs = BatSatBackend::solve_cnf(&cnf, &assumptions);
            match (&ours, &theirs) {
                (SolveOutcome::Sat(model), SolveOutcome::Sat(_)) => {
                    assert!(satisfies(model, &clauses));
                    for a in &assumptions {
                        assert_eq!(
                            model.lit_value(*a),
                            Some(true),
                            "assumptions hold in the model"
                        );
                    }
                }
                (SolveOutcome::Unsat(core), SolveOutcome::Unsat(_)) => {
                    // The core is a subset of the assumptions (plan §4).
                    for lit in core {
                        assert!(assumptions.contains(lit), "core literal is an assumption");
                    }
                }
                _ => panic!("CDCL and BatSat disagree under assumptions"),
            }
        }
    }

    #[test]
    fn proven_and_hypothetical_split_at_the_last_assumption() {
        // (¬a ∨ b): assuming `a` forces `b` at the assumption's level (proven),
        // and any later branch opens a deeper, hypothetical level. `c` is a free
        // variable to branch on once the assumption is satisfied.
        let (a, b, c) = (Var::new(0), Var::new(1), Var::new(2));
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.neg_lit(), b.pos_lit()]));
        // A tautology that never propagates, just to register `c` as branchable.
        cnf.add_clause(clause([c.pos_lit(), c.neg_lit()]));
        let cdcl = Cdcl::from_cnf(&cnf);
        let mut search: Search = cdcl.search(&[a.pos_lit()]);

        // First: the assumption `a` is placed as the base decision.
        assert!(matches!(search.step(), Event::Decide { .. }));
        assert_eq!(search.base_level(), 1);
        assert_eq!(search.level_of(a), Some(1));

        // Then `b` is forced by propagation at the base level — a proven fact.
        assert!(matches!(search.step(), Event::Propagate { .. }));
        assert_eq!(search.level_of(b), Some(1));
        assert!(search.level_of(b).unwrap() <= search.base_level());

        // With the assumption satisfied, the search branches on `c` above the
        // base level — a hypothesis.
        assert!(matches!(search.step(), Event::Decide { .. }));
        assert!(search.level_of(c).unwrap() > search.base_level());
    }

    #[test]
    fn search_probe_matches_bcp_before_branching() {
        // Before any branch (only assumptions on the trail), a mid-search probe
        // must agree with the search-free `Propagator::probe`: both start from the
        // givens alone. (¬a ∨ ¬b) with assumption a: probing b conflicts, a does not.
        use crate::propagate::Propagator;
        let (a, b) = (Var::new(0), Var::new(1));
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.neg_lit(), b.neg_lit()]));
        let cdcl = Cdcl::from_cnf(&cnf);
        let mut search = cdcl.search(&[a.pos_lit()]);
        // Step until the assumption `a` is placed but before any free-variable branch.
        assert!(matches!(search.step(), Event::Decide { .. }));
        let prop = Propagator::from_cnf(&cnf);
        assert_eq!(
            search.probe(b.pos_lit()),
            prop.probe(&[a.pos_lit()], b.pos_lit())
        );
        assert!(search.probe(b.pos_lit()), "b is refuted given a");
        // An already-true literal never conflicts; its negation always does.
        assert!(!search.probe(a.pos_lit()));
        assert!(search.probe(a.neg_lit()));
    }

    #[test]
    fn step_stream_is_well_formed() {
        // Drive the state machine one event at a time and check the invariants the
        // UI relies on: it terminates, and every propagated literal ends up true.
        let (a, b, c) = (Var::new(0), Var::new(1), Var::new(2));
        let mut cnf = Cnf::new();
        cnf.add_clause(clause([a.pos_lit()]));
        cnf.add_clause(clause([a.neg_lit(), b.pos_lit()]));
        cnf.add_clause(clause([b.neg_lit(), c.pos_lit()]));
        let solver = Cdcl::from_cnf(&cnf);
        let mut search: Search = solver.search(&[]);

        let mut propagated = Vec::new();
        let mut steps = 0;
        loop {
            steps += 1;
            assert!(steps < 10_000, "search must terminate");
            match search.step() {
                Event::Propagate { lit, reason } => {
                    // The reason clause names a real clause in the database.
                    assert!(!search.clause_lits(reason).is_empty());
                    propagated.push(lit);
                }
                Event::Sat => break,
                Event::Unsat { .. } => panic!("the chain is satisfiable"),
                _ => {}
            }
        }
        let model = search.assignment();
        for lit in propagated {
            assert_eq!(model.lit_value(lit), Some(true));
        }
    }
}
