//! JSON-Schema validation of `evals.json` and pipeline artifacts.
//!
//! Mirrors `src/validation/` in eval-runner. The AJV validator of the original
//! is replaced by the `jsonschema` crate, validating against the bundled
//! `schema/*.json` (embedded at compile time via `include_str!`).

pub mod batch;
pub mod error;
pub mod evals;
pub mod schema;

pub use batch::{FileOutcome, ValidationReport, validate_all};
pub use error::ValidationError;
pub use evals::validate_evals_config;
pub use schema::{SchemaName, validate_against_schema};
