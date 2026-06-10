//! Subcommand handlers, grouped by concern. Each handler maps a parsed
//! [`super::args`] command to its owning module and reports the user-facing
//! result. Dispatch ([`super::dispatch`]) routes to these via the re-exports
//! below; the handlers lean on the shared context/iteration helpers in
//! [`super`] (`crate::cli`).

mod guard;
mod pipeline;
mod run;
mod validate;
mod workspace;

pub(crate) use guard::{run_guard, run_guard_codex, run_teardown_guard};
pub(crate) use pipeline::{
    run_aggregate, run_detect_stray_writes, run_fill_transcripts, run_finalize, run_grade,
    run_ingest, run_record_runs,
};
pub(crate) use run::run_run;
pub(crate) use validate::run_validate;
pub(crate) use workspace::{run_promote_baseline, run_snapshot, run_teardown};
