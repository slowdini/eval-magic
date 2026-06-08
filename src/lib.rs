//! `eval-magic` — the Rust rewrite of `@slowdini/eval-runner`.
//!
//! A CLI for running skill evals: it measures whether an agent skill actually
//! shifts behavior. The crate is organized into the same seven submodules as
//! the TypeScript original so the port can proceed one module at a time. See
//! `rewrite-roadmap.md` at the repo root for the phased plan.
//!
//! Each module below begins as a documented stub and is filled in during its
//! roadmap phase.

pub mod adapters;
pub mod cli;
pub mod core;
pub mod pipeline;
pub mod sandbox;
pub mod validation;
pub mod workspace;
