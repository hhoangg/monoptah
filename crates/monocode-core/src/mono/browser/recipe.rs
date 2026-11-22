//! Browser recipes: a reusable, read-only task on one origin that Monoptah replays without a
//! model call per step. Design and measurements:
//! `docs/superpowers/specs/2026-11-22-browser-recipes-design.md` (Phase 1).
//!
//! * [`model`]: the recipe file format, its validation and its content hash.
//! * [`store`]: JSON files (private to the user) and the table of approved hashes.
//! * [`snapshot`]: a parser for the accessibility snapshot text of `@playwright/mcp`.
//! * [`runner`]: replays a recipe through the gate's policy, locates elements with scoped probes
//!   and returns a compact result.
//! * [`repair`]: self-healing of one failed step with a validated patch, versions and rollback.
//! * [`risk`]: what a step can do (read, write, send, money), and the credentials a recipe never types.
//! * [`record`]: keeps a trace of the Mono's browser calls and compiles a recipe from it.
//! * [`share`]: the export file, importing it, and the shared library (Phase 4).
//! * [`manage`]: what the Browser tab does: list, approve, dry run, rollback, delete, promote.
//! * [`tools`]: the `recipe_*` tools the gate offers.
pub mod manage;
pub mod model;
pub mod record;
pub mod repair;
pub mod risk;
pub mod runner;
pub mod share;
pub mod snapshot;
pub mod store;
pub mod tools;

#[cfg(test)]
mod tests;
