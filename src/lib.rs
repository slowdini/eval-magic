//! `eval-magic` — a CLI for running skill evals: it measures whether an agent
//! skill actually shifts behavior.
//!
//! The crate is organized into seven submodules, ordered roughly by dependency:
//! `core` (domain types) underpins everything; `validation`, `adapters`,
//! `sandbox`, `pipeline`, and `workspace` each own one concern; `cli` dispatches
//! the subcommands and orchestrates `run`.

pub mod adapters;
pub mod cli;
pub mod core;
pub mod pipeline;
pub mod sandbox;
pub mod validation;
pub mod workspace;
