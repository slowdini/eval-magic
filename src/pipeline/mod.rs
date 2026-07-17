//! The post-dispatch processing chain: stateless JSON-in/JSON-out stages.
//!
//! Chain order:
//! `record-runs` → `fill-transcripts` → `detect-stray-writes` → `grade` →
//! `aggregate`. Each stage reads JSON/JSONL artifacts from an iteration directory
//! and writes JSON back; no stage pipes to another in-memory, so any stage can
//! be run (and re-run) standalone.

pub mod aggregate;
pub mod detect_stray_writes;
pub mod error;
pub mod fill_transcripts;
pub mod grade;
pub mod io;
pub mod record_runs;
pub mod slots;

pub use aggregate::{Benchmark, aggregate};
pub use detect_stray_writes::{
    StrayFinding, StrayWritesReport, detect_live_source_reads, detect_stray_writes,
    detect_stray_writes_report,
};
pub use error::PipelineError;
pub use fill_transcripts::{FillTranscriptsResult, fill_transcripts};
pub use grade::{GradeContext, emit_judge_tasks, finalize};
pub use record_runs::{RecordRunsResult, record_runs};
pub use slots::{RunSlot, run_slots};
