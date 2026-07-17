//! Per-harness run-option capabilities.
//!
//! Every dispatch rides a single mechanism: each task is delivered through a
//! one-shot harness CLI subprocess. What still varies by harness is which *run
//! options* the generic `run` preflight may accept — captured as this narrow
//! capability struct, reported per harness by
//! `HarnessAdapter::run_capabilities` and independent of the comparison
//! [`Mode`](crate::core::Mode) (`new-skill` / `revision`), which selects the
//! two conditions being compared.

/// Run-option support for a harness's dispatch path.
///
/// This is intentionally narrower than the full enhancement surface: it only
/// describes options the generic `run` preflight must accept or reject before
/// the build sequence starts. Harness-specific behavior still lives behind the
/// adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessRunCapabilities {
    pub supports_guard: bool,
    pub supports_bootstrap_with_no_stage: bool,
    pub supports_stage_name_with_no_stage: bool,
}
