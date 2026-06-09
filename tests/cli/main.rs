//! Integration tests for the CLI surface, driving the built `skill-eval`
//! binary. Mirrors the subprocess-style integration tests in eval-runner
//! (`cli.test.ts`). These pin the command tree and dispatch behavior of the
//! Phase-0 scaffold; per-command behavior is tested as each module is ported.
//!
//! Split by subcommand into submodules (file-length guideline); shared helpers
//! ([`skill_eval`](helpers::skill_eval), [`canonical_root`](helpers::canonical_root))
//! live in [`helpers`], single-use helpers stay with their group.

mod helpers;

mod aggregate;
mod basics;
mod grade;
mod guard;
mod stray_writes;
mod workspace;
