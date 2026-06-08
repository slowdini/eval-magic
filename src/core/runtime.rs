//! Runtime & path helpers.
//!
//! TODO(port): Phase 1 — port `src/core/runtime.ts`. The Bun-vs-Node
//! portability shims have no Rust equivalent; the git-spawning and path
//! resolution helpers carry over (`std::process::Command`, `std::path`).
