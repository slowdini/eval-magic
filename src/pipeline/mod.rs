//! The post-dispatch processing chain: stateless JSON-in/JSON-out stages.
//!
//! Mirrors `src/pipeline/` in eval-runner. Chain order:
//! `record-runs` → `fill-transcripts` → `detect-stray-writes` → `grade` →
//! `aggregate`. Each stage reads JSON/JSONL artifacts from an iteration directory
//! and writes JSON back; no stage pipes to another in-memory. Stages are ported
//! one at a time against the same fixtures the TypeScript suite uses.

pub mod error;
pub mod io;
pub mod record_runs;

pub use error::PipelineError;
pub use record_runs::{RecordRunsResult, record_runs};
