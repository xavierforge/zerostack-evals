//! Domain modules: zerostack-subsystem-specific knowledge, quarantined.
//!
//! The harness core (scenario/backend/seed/asserts/verdict) is generic.
//! Anything that encodes how a *particular* zerostack subsystem lays out
//! files or derives identifiers lives here, one module per subsystem, and
//! only ever surfaces as scenario-TOML sugar that expands into generic
//! `seed::Placement`s. Adding eval support for another subsystem (subagents,
//! chains, …) that needs seeding sugar = one new module here, zero changes to
//! the core.
//!
//! Because this knowledge is a snapshot of zerostack internals, every module
//! here pairs its layout knowledge with a runtime drift check the runner
//! calls after driving the agent — a stale snapshot grades Indeterminate,
//! never a silent Fail (see `memory::verify_layout`).

pub mod memory;
