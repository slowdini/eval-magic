//! The post-dispatch processing chain: six stateless JSON-in/JSON-out stages.
//!
//! Mirrors `src/pipeline/` in eval-runner. Chain order:
//! `record-runs` → `fill-transcripts` → `detect-stray-writes` → `grade` →
//! `aggregate`.
//!
//! TODO(port): Phase 5 — port each stage one at a time against shared fixtures;
//! decompose `grade.ts` (the largest pipeline file) while porting it.
