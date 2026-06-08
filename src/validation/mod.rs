//! JSON-Schema validation of `evals.json` and pipeline artifacts.
//!
//! Mirrors `src/validation/` in eval-runner. The AJV validator of the original
//! is replaced by the `jsonschema` crate, validating against the bundled
//! `schema/*.json`.
//!
//! TODO(port): Phase 2 — port `validate-schema.ts`, `validate.ts`,
//! `validate-all.ts`.
