//! Integration tests for the CLI surface, driving the built `eval-magic`
//! binary. These pin the command tree and dispatch behavior; per-command
//! behavior lives with each subcommand's submodule.
//!
//! Split by subcommand into submodules (file-length guideline); shared helpers
//! ([`skill_eval`](helpers::skill_eval), [`canonical_root`](helpers::canonical_root))
//! live in [`helpers`], single-use helpers stay with their group.

mod helpers;

mod aggregate;
mod basics;
mod grade;
mod guard;
mod init;
mod stray_writes;
mod workspace;
