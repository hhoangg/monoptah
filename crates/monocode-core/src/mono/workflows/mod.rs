//! Mono workflows: a graph of session, command, Ask Mono, If, For each and Report nodes.
//! The definition, lenient parsing and validation are pure; files and running come later.
pub mod board_edit;
#[cfg(test)]
mod board_edit_tests;
pub mod board_model;
#[cfg(test)]
mod board_model_tests;
pub mod cli;
#[cfg(test)]
mod cli_tests;
pub mod command;
pub mod condition;
pub mod engine;
#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod fixtures;
pub mod guide;
pub mod model;
pub mod models;
pub mod ordered;
pub mod output;
pub mod parse;
pub mod report;
#[cfg(test)]
mod report_tests;
pub mod run_graph_model;
#[cfg(test)]
mod run_graph_model_tests;
pub mod schedule;
pub mod session_context;
pub mod store;
#[cfg(test)]
mod store_tests;
pub mod template;
#[cfg(test)]
mod tests;
pub mod tick;
#[cfg(test)]
mod tick_tests;
#[cfg(test)]
mod ui_fixtures;
pub mod validate;
pub mod yaml;
mod yaml_emit;
mod yaml_read;
#[cfg(test)]
mod yaml_tests;
