//! Browser recipes: a reusable, read-only task on one origin that Monoptah replays without a
//! model call per step. Design and measurements:
//! `docs/superpowers/specs/2026-11-22-browser-recipes-design.md` (Phase 1).
//!
//! * [`model`]: the recipe file format, its validation and its content hash.
//! * [`store`]: JSON files (private to the user) and the table of approved hashes.
//! * [`snapshot`]: a parser for the accessibility snapshot text of `@playwright/mcp`.
//! * [`runner`]: replays a recipe through the gate's policy, locates elements with scoped probes
//!   and returns a compact result.
//! * [`record`]: keeps a trace of the Mono's browser calls and compiles a recipe from it.
//! * [`tools`]: the `recipe_*` tools the gate offers.
pub mod model;
pub mod record;
pub mod runner;
pub mod snapshot;
pub mod store;
pub mod tools;

#[cfg(test)]
mod tests;
